use std::collections::hash_map::DefaultHasher;
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fuser::{
    FileAttr, FileType, Filesystem, MountOption, ReplyAttr, ReplyCreate, ReplyData,
    ReplyDirectory, ReplyEntry, ReplyOpen, ReplyStatfs, ReplyWrite, ReplyXattr, Request,
};
use zeroize::Zeroize;

use mosaic_core::header::{VaultHeader, VaultPrelude};
use mosaic_core::index::{FileEntry, FileSegment};
use mosaic_core::pool::PoolManager;

const TTL: Duration = Duration::from_secs(1);
const ROOT_INO: u64 = 1;
const BLOCK_SIZE: u32 = 4096;

/// FUSE filesystem exposing a Mosaic vault as a mounted directory.
pub struct MosaicFS {
    header: Arc<RwLock<VaultHeader>>,
    prelude: VaultPrelude,
    pools: Arc<RwLock<PoolManager>>,
    header_path: PathBuf,
    key: [u8; 32],
}

impl Drop for MosaicFS {
    fn drop(&mut self) {
        // Save header on unmount
        if let Ok(header) = self.header.read() {
            if let Ok(pools) = self.pools.read() {
                let mut h = header.clone();
                h.pool_index = pools.pool_index().to_vec();
                if let Err(e) = h.save(&self.header_path, &self.key, &self.prelude) {
                    tracing::error!("Failed to save header on unmount: {}", e);
                }
            }
        }
        self.key.zeroize();
    }
}

impl MosaicFS {
    pub fn new(
        header: VaultHeader,
        prelude: VaultPrelude,
        pools: PoolManager,
        header_path: PathBuf,
        key: [u8; 32],
    ) -> Self {
        Self {
            header: Arc::new(RwLock::new(header)),
            prelude,
            pools: Arc::new(RwLock::new(pools)),
            header_path,
            key,
        }
    }

    fn path_from_ino_and_name(parent_path: &str, name: &str) -> String {
        if parent_path.is_empty() || parent_path == "/" {
            name.to_string()
        } else {
            format!("{}/{}", parent_path, name)
        }
    }
}

/// Hash-based inode from virtual path.
fn path_to_ino(path: &str) -> u64 {
    if path.is_empty() || path == "/" {
        return ROOT_INO;
    }
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    let h = hasher.finish();
    if h == ROOT_INO { h.wrapping_add(1) } else { h }
}

/// Reverse lookup: inode -> path by scanning the file index.
fn ino_to_path(header: &VaultHeader, ino: u64) -> Option<String> {
    if ino == ROOT_INO {
        return Some(String::new());
    }
    for key in header.file_index.entries.keys() {
        if path_to_ino(key) == ino {
            return Some(key.clone());
        }
    }
    for dir in collect_dirs(header) {
        if path_to_ino(&dir) == ino {
            return Some(dir);
        }
    }
    None
}

fn collect_dirs(header: &VaultHeader) -> Vec<String> {
    let mut dirs = BTreeSet::new();
    for key in header.file_index.entries.keys() {
        let parts: Vec<&str> = key.split('/').collect();
        let mut path = String::new();
        for (i, part) in parts.iter().enumerate() {
            if i == parts.len() - 1 {
                break;
            }
            if !path.is_empty() {
                path.push('/');
            }
            path.push_str(part);
            dirs.insert(path.clone());
        }
    }
    dirs.into_iter().collect()
}

fn file_attr(ino: u64, entry: &FileEntry) -> FileAttr {
    FileAttr {
        ino,
        size: entry.size,
        blocks: (entry.size + 511) / 512,
        atime: system_time(entry.modified_at),
        mtime: system_time(entry.modified_at),
        ctime: system_time(entry.created_at),
        crtime: system_time(entry.created_at),
        kind: FileType::RegularFile,
        perm: 0o644,
        nlink: 1,
        uid: unsafe { libc::getuid() },
        gid: unsafe { libc::getgid() },
        rdev: 0,
        blksize: BLOCK_SIZE,
        flags: 0,
    }
}

fn dir_attr(ino: u64) -> FileAttr {
    let now = now_secs();
    FileAttr {
        ino,
        size: 0,
        blocks: 0,
        atime: system_time(now),
        mtime: system_time(now),
        ctime: system_time(now),
        crtime: system_time(now),
        kind: FileType::Directory,
        perm: 0o755,
        nlink: 2,
        uid: unsafe { libc::getuid() },
        gid: unsafe { libc::getgid() },
        rdev: 0,
        blksize: BLOCK_SIZE,
        flags: 0,
    }
}

fn system_time(secs: u64) -> SystemTime {
    // Guard against any timestamp that could cause overflow on macOS
    let safe_secs = if secs == 0 || secs > 4_102_444_800 {
        946_684_800 // 2000-01-01
    } else {
        secs
    };
    UNIX_EPOCH
        .checked_add(Duration::from_secs(safe_secs))
        .unwrap_or(UNIX_EPOCH + Duration::from_secs(946_684_800))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(946_684_800)
}

fn time_or_now_to_secs(t: fuser::TimeOrNow) -> u64 {
    match t {
        fuser::TimeOrNow::SpecificTime(st) => st
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_else(|_| now_secs()),
        fuser::TimeOrNow::Now => now_secs(),
    }
}

impl Filesystem for MosaicFS {
    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let header = self.header.read().unwrap();
        let parent_path = match ino_to_path(&header, parent) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };
        let name_str = match name.to_str() {
            Some(n) => n,
            None => {
                reply.error(libc::EINVAL);
                return;
            }
        };
        let full_path = MosaicFS::path_from_ino_and_name(&parent_path, name_str);
        let ino = path_to_ino(&full_path);

        if let Some(entry) = header.file_index.get(&full_path) {
            reply.entry(&TTL, &file_attr(ino, entry), 0);
            return;
        }

        if header.file_index.is_dir(&full_path) {
            reply.entry(&TTL, &dir_attr(ino), 0);
            return;
        }

        reply.error(libc::ENOENT);
    }

    fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
        if ino == ROOT_INO {
            reply.attr(&TTL, &dir_attr(ROOT_INO));
            return;
        }

        let header = self.header.read().unwrap();
        let path = match ino_to_path(&header, ino) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        if let Some(entry) = header.file_index.get(&path) {
            reply.attr(&TTL, &file_attr(ino, entry));
            return;
        }

        if header.file_index.is_dir(&path) {
            reply.attr(&TTL, &dir_attr(ino));
            return;
        }

        reply.error(libc::ENOENT);
    }

    fn readdir(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        let header = self.header.read().unwrap();
        let dir_path = match ino_to_path(&header, ino) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let children = header.file_index.list_dir(&dir_path);
        let mut entries = Vec::new();

        entries.push((ino, FileType::Directory, ".".to_string()));
        entries.push((ROOT_INO, FileType::Directory, "..".to_string()));

        for child_name in &children {
            let child_path = MosaicFS::path_from_ino_and_name(&dir_path, child_name);
            let child_ino = path_to_ino(&child_path);
            let kind = if header.file_index.get(&child_path).is_some() {
                FileType::RegularFile
            } else {
                FileType::Directory
            };
            entries.push((child_ino, kind, child_name.clone()));
        }

        for (i, (ino, kind, name)) in entries.into_iter().enumerate().skip(offset as usize) {
            if reply.add(ino, (i + 1) as i64, kind, name) {
                break;
            }
        }
        reply.ok();
    }

    fn open(&mut self, _req: &Request, _ino: u64, _flags: i32, reply: ReplyOpen) {
        reply.opened(0, 0);
    }

    fn read(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyData,
    ) {
        let header = self.header.read().unwrap();
        let path = match ino_to_path(&header, ino) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };
        let entry = match header.file_index.get(&path) {
            Some(e) => e,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let offset = offset as u64;
        if offset >= entry.size {
            reply.data(&[]);
            return;
        }

        let read_size = std::cmp::min(size as u64, entry.size - offset);
        let pools = self.pools.read().unwrap();

        let mut result = Vec::new();
        let mut remaining = read_size;
        let mut file_offset = offset;

        for seg in &entry.segments {
            if remaining == 0 {
                break;
            }
            if file_offset >= seg.length {
                file_offset -= seg.length;
                continue;
            }

            let seg_read_offset = seg.offset + file_offset;
            let seg_read_size = std::cmp::min(remaining, seg.length - file_offset);

            match pools.read(seg.pool_id, seg_read_offset, seg_read_size) {
                Ok(data) => {
                    result.extend_from_slice(&data);
                    remaining -= seg_read_size;
                    file_offset = 0;
                }
                Err(_) => {
                    reply.error(libc::EIO);
                    return;
                }
            }
        }

        reply.data(&result);
    }

    fn write(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        data: &[u8],
        _write_flags: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyWrite,
    ) {
        let mut header = self.header.write().unwrap();
        let path = match ino_to_path(&header, ino) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let mut pools = self.pools.write().unwrap();

        let (pool_id, pool_offset) = match pools.allocate(data.len() as u64) {
            Ok(a) => a,
            Err(_) => {
                reply.error(libc::ENOSPC);
                return;
            }
        };

        if pools.write(pool_id, pool_offset, data).is_err() {
            reply.error(libc::EIO);
            return;
        }

        let now = now_secs();

        if let Some(entry) = header.file_index.get_mut(&path) {
            entry.segments.push(FileSegment {
                pool_id,
                offset: pool_offset,
                length: data.len() as u64,
            });
            entry.size = offset as u64 + data.len() as u64;
            entry.modified_at = now;
        }

        reply.written(data.len() as u32);
    }

    fn create(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        _flags: i32,
        reply: ReplyCreate,
    ) {
        let mut header = self.header.write().unwrap();
        let parent_path = match ino_to_path(&header, parent) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };
        let name_str = match name.to_str() {
            Some(n) => n,
            None => {
                reply.error(libc::EINVAL);
                return;
            }
        };
        let full_path = MosaicFS::path_from_ino_and_name(&parent_path, name_str);
        let ino = path_to_ino(&full_path);

        let now = now_secs();

        let entry = FileEntry {
            size: 0,
            created_at: now,
            modified_at: now,
            segments: Vec::new(),
        };

        header.file_index.insert(&full_path, entry.clone());
        reply.created(&TTL, &file_attr(ino, &entry), 0, 0, 0);
    }

    fn unlink(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: fuser::ReplyEmpty) {
        let mut header = self.header.write().unwrap();
        let parent_path = match ino_to_path(&header, parent) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };
        let name_str = match name.to_str() {
            Some(n) => n,
            None => {
                reply.error(libc::EINVAL);
                return;
            }
        };
        let full_path = MosaicFS::path_from_ino_and_name(&parent_path, name_str);

        match header.file_index.remove(&full_path) {
            Some(_) => reply.ok(),
            None => reply.error(libc::ENOENT),
        }
    }

    fn mkdir(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        let header = self.header.read().unwrap();
        let parent_path = match ino_to_path(&header, parent) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };
        let name_str = match name.to_str() {
            Some(n) => n,
            None => {
                reply.error(libc::EINVAL);
                return;
            }
        };
        let full_path = MosaicFS::path_from_ino_and_name(&parent_path, name_str);
        let ino = path_to_ino(&full_path);

        reply.entry(&TTL, &dir_attr(ino), 0);
    }

    fn rmdir(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: fuser::ReplyEmpty) {
        let header = self.header.read().unwrap();
        let parent_path = match ino_to_path(&header, parent) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };
        let name_str = match name.to_str() {
            Some(n) => n,
            None => {
                reply.error(libc::EINVAL);
                return;
            }
        };
        let full_path = MosaicFS::path_from_ino_and_name(&parent_path, name_str);

        let children = header.file_index.list_dir(&full_path);
        if !children.is_empty() {
            reply.error(libc::ENOTEMPTY);
            return;
        }

        reply.ok();
    }

    fn setattr(
        &mut self,
        _req: &Request,
        ino: u64,
        _mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        size: Option<u64>,
        atime: Option<fuser::TimeOrNow>,
        mtime: Option<fuser::TimeOrNow>,
        _ctime: Option<SystemTime>,
        _fh: Option<u64>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        if ino == ROOT_INO {
            reply.attr(&TTL, &dir_attr(ROOT_INO));
            return;
        }

        let mut header = self.header.write().unwrap();
        let path = match ino_to_path(&header, ino) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        if header.file_index.is_dir(&path) {
            reply.attr(&TTL, &dir_attr(ino));
            return;
        }

        if let Some(entry) = header.file_index.get_mut(&path) {
            if let Some(new_size) = size {
                entry.size = new_size;
            }
            if let Some(t) = mtime {
                entry.modified_at = time_or_now_to_secs(t);
            }
            if let Some(t) = atime {
                // We store mtime only, but accept atime silently
                let _ = t;
            }
            reply.attr(&TTL, &file_attr(ino, entry));
        } else {
            reply.error(libc::ENOENT);
        }
    }

    fn flush(
        &mut self,
        _req: &Request,
        _ino: u64,
        _fh: u64,
        _lock_owner: u64,
        reply: fuser::ReplyEmpty,
    ) {
        reply.ok();
    }

    fn release(
        &mut self,
        _req: &Request,
        _ino: u64,
        _fh: u64,
        _flags: i32,
        _lock_owner: Option<u64>,
        _flush: bool,
        reply: fuser::ReplyEmpty,
    ) {
        reply.ok();
    }

    fn access(&mut self, _req: &Request, _ino: u64, _mask: i32, reply: fuser::ReplyEmpty) {
        // DefaultPermissions handles checks; always grant here
        reply.ok();
    }

    fn statfs(&mut self, _req: &Request, _ino: u64, reply: ReplyStatfs) {
        let header = self.header.read().unwrap();
        let tile_size = header.metadata.tile_size_bytes;
        let total_blocks = if tile_size > 0 {
            let pool_count = header.pool_index.len() as u64;
            (pool_count * tile_size) / BLOCK_SIZE as u64
        } else {
            0
        };
        let used: u64 = header
            .pool_index
            .iter()
            .map(|p| p.size_bytes / BLOCK_SIZE as u64)
            .sum();
        let free = total_blocks.saturating_sub(used);
        let files = header.file_index.entries.len() as u64;

        reply.statfs(
            total_blocks, // total blocks
            free,         // free blocks
            free,         // available blocks
            files,        // total inodes
            u64::MAX,     // free inodes
            BLOCK_SIZE,   // block size
            255,          // max name length
            BLOCK_SIZE,   // fragment size
        );
    }

    fn getxattr(
        &mut self,
        _req: &Request,
        _ino: u64,
        _name: &OsStr,
        _size: u32,
        reply: ReplyXattr,
    ) {
        // No extended attributes supported
        // macOS uses ENOATTR, Linux uses ENODATA
        #[cfg(target_os = "macos")]
        { reply.error(93); } // ENOATTR on macOS
        #[cfg(not(target_os = "macos"))]
        { reply.error(libc::ENODATA); }
    }

    fn listxattr(&mut self, _req: &Request, _ino: u64, _size: u32, reply: ReplyXattr) {
        // No extended attributes — return empty size
        reply.size(0);
    }

    fn setxattr(
        &mut self,
        _req: &Request,
        _ino: u64,
        _name: &OsStr,
        _value: &[u8],
        _flags: i32,
        _position: u32,
        reply: fuser::ReplyEmpty,
    ) {
        reply.error(libc::ENOSYS);
    }

    fn rename(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &OsStr,
        new_parent: u64,
        new_name: &OsStr,
        _flags: u32,
        reply: fuser::ReplyEmpty,
    ) {
        let mut header = self.header.write().unwrap();
        let parent_path = match ino_to_path(&header, parent) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };
        let new_parent_path = match ino_to_path(&header, new_parent) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };
        let name_str = match name.to_str() {
            Some(n) => n,
            None => {
                reply.error(libc::EINVAL);
                return;
            }
        };
        let new_name_str = match new_name.to_str() {
            Some(n) => n,
            None => {
                reply.error(libc::EINVAL);
                return;
            }
        };

        let old_path = MosaicFS::path_from_ino_and_name(&parent_path, name_str);
        let new_path = MosaicFS::path_from_ino_and_name(&new_parent_path, new_name_str);

        if let Some(entry) = header.file_index.remove(&old_path) {
            header.file_index.insert(&new_path, entry);
            reply.ok();
        } else {
            reply.error(libc::ENOENT);
        }
    }
}

/// Mounts the filesystem in a background thread.
/// Returns a BackgroundSession that unmounts on drop.
pub fn mount(
    header: VaultHeader,
    prelude: VaultPrelude,
    pools: PoolManager,
    header_path: PathBuf,
    key: [u8; 32],
    mountpoint: &Path,
) -> Result<fuser::BackgroundSession, crate::FuseError> {
    let fs = MosaicFS::new(header, prelude, pools, header_path, key);

    let options = vec![
        MountOption::FSName("mosaic".to_string()),
        MountOption::AutoUnmount,
        MountOption::DefaultPermissions,
        MountOption::NoDev,
        MountOption::NoSuid,
        MountOption::CUSTOM("volname=Mosaic".to_string()),
    ];

    let session = fuser::spawn_mount2(fs, mountpoint, &options)
        .map_err(|e| crate::FuseError::MountFailed(e.to_string()))?;

    Ok(session)
}
