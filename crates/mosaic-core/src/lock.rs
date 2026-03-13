use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// A file-based lock that prevents concurrent access to a vault.
/// Creates `vault.lock` next to the header file.
/// The lock file contains the PID of the locking process for diagnostics.
pub struct VaultLock {
    path: PathBuf,
}

impl VaultLock {
    /// Acquires a lock for the vault at `header_path`.
    /// Returns an error if another process already holds the lock.
    pub fn acquire(header_path: &Path) -> Result<Self, LockError> {
        let lock_path = Self::lock_path(header_path);

        // Check if a stale lock exists
        if lock_path.exists() {
            // Read the PID from the lock file
            if let Ok(contents) = fs::read_to_string(&lock_path) {
                if let Ok(pid) = contents.trim().parse::<u32>() {
                    if is_process_alive(pid) {
                        return Err(LockError::AlreadyLocked(pid));
                    }
                }
            }
            // Stale lock — remove it
            let _ = fs::remove_file(&lock_path);
        }

        // Create lock file with our PID
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::AlreadyExists {
                    LockError::AlreadyLocked(0)
                } else {
                    LockError::Io(e)
                }
            })?;

        let pid = std::process::id();
        writeln!(file, "{}", pid).map_err(LockError::Io)?;
        file.sync_all().map_err(LockError::Io)?;

        Ok(Self { path: lock_path })
    }

    /// Releases the lock by deleting the lock file.
    pub fn release(self) {
        // Drop will handle cleanup
        drop(self);
    }

    fn lock_path(header_path: &Path) -> PathBuf {
        let dir = header_path.parent().unwrap_or(Path::new("."));
        dir.join("vault.lock")
    }
}

impl Drop for VaultLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Checks if a process with the given PID is still alive.
fn is_process_alive(pid: u32) -> bool {
    // kill(pid, 0) checks if the process exists without sending a signal
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[derive(Debug)]
pub enum LockError {
    /// Another process (with the given PID) holds the lock.
    AlreadyLocked(u32),
    /// I/O error creating/reading the lock file.
    Io(std::io::Error),
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LockError::AlreadyLocked(pid) => {
                write!(f, "Vault is already locked by process {}", pid)
            }
            LockError::Io(e) => write!(f, "Lock I/O error: {}", e),
        }
    }
}

impl std::error::Error for LockError {}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_lock_acquire_release() {
        let dir = TempDir::new().unwrap();
        let header_path = dir.path().join("vault.header");
        File::create(&header_path).unwrap();

        let lock = VaultLock::acquire(&header_path).unwrap();
        assert!(dir.path().join("vault.lock").exists());

        drop(lock);
        assert!(!dir.path().join("vault.lock").exists());
    }

    #[test]
    fn test_lock_prevents_double_lock() {
        let dir = TempDir::new().unwrap();
        let header_path = dir.path().join("vault.header");
        File::create(&header_path).unwrap();

        let _lock1 = VaultLock::acquire(&header_path).unwrap();
        let result = VaultLock::acquire(&header_path);
        assert!(result.is_err());
    }
}
