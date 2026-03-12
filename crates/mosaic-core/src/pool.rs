use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::crypto;
use crate::error::PoolError;
use crate::header::{PoolEntry, PoolStatus};

/// Block size for encryption within pools: 64 KB
const BLOCK_SIZE: u64 = 64 * 1024;
/// ChaCha20-Poly1305 tag overhead per block
const TAG_OVERHEAD: u64 = 16;
/// Encrypted block size on disk: 64 KB plaintext + 16 bytes tag
const ENCRYPTED_BLOCK_SIZE: u64 = BLOCK_SIZE + TAG_OVERHEAD;

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct PoolManager {
    #[zeroize(skip)]
    header_dir: PathBuf,
    #[zeroize(skip)]
    tile_size: u64,
    key: [u8; 32],
    #[zeroize(skip)]
    pool_index: Vec<PoolEntry>,
}

impl PoolManager {
    pub fn new(
        header_dir: PathBuf,
        tile_size: u64,
        key: [u8; 32],
        pool_index: Vec<PoolEntry>,
    ) -> Self {
        Self {
            header_dir,
            tile_size,
            key,
            pool_index,
        }
    }

    /// Returns a snapshot of the current pool index for saving to header.
    pub fn pool_index(&self) -> &[PoolEntry] {
        &self.pool_index
    }

    /// Allocates space for writing `size` bytes.
    /// Creates a new tile if necessary.
    /// Returns: (pool_id, offset) in logical (plaintext) space.
    pub fn allocate(&mut self, size: u64) -> Result<(u32, u64), PoolError> {
        // Find active pool with space
        for entry in &mut self.pool_index {
            if entry.status == PoolStatus::Active {
                let available = self.tile_size.saturating_sub(entry.size_bytes);
                if available >= size {
                    let offset = entry.size_bytes;
                    entry.size_bytes += size;
                    return Ok((entry.id, offset));
                } else {
                    // Mark as full, continue to create new one
                    entry.status = PoolStatus::Full;
                }
            }
        }

        // Need a new pool
        let new_id = self.pool_index.len() as u32;
        self.create_pool(new_id)?;

        let new_entry = PoolEntry {
            id: new_id,
            filename: Self::format_pool_name(new_id),
            size_bytes: size,
            checksum: [0u8; 32],
            status: PoolStatus::Active,
        };
        self.pool_index.push(new_entry);

        Ok((new_id, 0))
    }

    /// Reads `size` bytes from pool_id at the given logical offset.
    /// Data is decrypted on the fly (ChaCha20-Poly1305 per 64KB block).
    pub fn read(&self, pool_id: u32, offset: u64, size: u64) -> Result<Vec<u8>, PoolError> {
        let path = self.pool_path(pool_id);
        if !path.exists() {
            return Err(PoolError::PoolNotFound(pool_id));
        }

        let mut file = std::fs::File::open(&path)?;
        let mut result = Vec::with_capacity(size as usize);

        let start_block = offset / BLOCK_SIZE;
        let end_block = (offset + size).saturating_sub(1) / BLOCK_SIZE;
        let offset_in_first_block = (offset % BLOCK_SIZE) as usize;

        for block_idx in start_block..=end_block {
            let disk_offset = block_idx * ENCRYPTED_BLOCK_SIZE;
            file.seek(SeekFrom::Start(disk_offset))?;

            // Read the encrypted block
            let mut encrypted_buf = vec![0u8; ENCRYPTED_BLOCK_SIZE as usize];
            let bytes_read = file.read(&mut encrypted_buf)?;
            if bytes_read == 0 {
                break;
            }
            encrypted_buf.truncate(bytes_read);

            // Decrypt
            let nonce = crypto::derive_block_nonce(pool_id, block_idx);
            let plaintext = crypto::decrypt_with_nonce(&self.key, &nonce, &encrypted_buf)
                .map_err(|_| PoolError::DecryptionError)?;

            // Extract the relevant portion
            let start = if block_idx == start_block {
                offset_in_first_block
            } else {
                0
            };
            let remaining = size as usize - result.len();
            let end = std::cmp::min(plaintext.len(), start + remaining);

            if start < end {
                result.extend_from_slice(&plaintext[start..end]);
            }
        }

        Ok(result)
    }

    /// Writes data into a pool at the given logical offset.
    /// Data is encrypted on the fly.
    pub fn write(&mut self, pool_id: u32, offset: u64, data: &[u8]) -> Result<(), PoolError> {
        let path = self.pool_path(pool_id);

        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&path)?;

        let start_block = offset / BLOCK_SIZE;
        let total_blocks = if data.is_empty() {
            0
        } else {
            let end = offset + data.len() as u64;
            (end.saturating_sub(1) / BLOCK_SIZE) - start_block + 1
        };

        let offset_in_first_block = (offset % BLOCK_SIZE) as usize;
        let mut data_pos = 0usize;

        for i in 0..total_blocks {
            let block_idx = start_block + i;
            let disk_offset = block_idx * ENCRYPTED_BLOCK_SIZE;

            // Try to read existing block to handle partial writes
            let mut plaintext = vec![0u8; BLOCK_SIZE as usize];
            file.seek(SeekFrom::Start(disk_offset))?;
            let mut existing_encrypted = vec![0u8; ENCRYPTED_BLOCK_SIZE as usize];
            let bytes_read = file.read(&mut existing_encrypted)?;

            if bytes_read > 0 {
                existing_encrypted.truncate(bytes_read);
                let nonce = crypto::derive_block_nonce(pool_id, block_idx);
                if let Ok(existing) =
                    crypto::decrypt_with_nonce(&self.key, &nonce, &existing_encrypted)
                {
                    let copy_len = std::cmp::min(existing.len(), plaintext.len());
                    plaintext[..copy_len].copy_from_slice(&existing[..copy_len]);
                }
            }

            // Write new data into the plaintext block
            let block_start = if i == 0 { offset_in_first_block } else { 0 };
            let remaining_data = data.len() - data_pos;
            let space_in_block = BLOCK_SIZE as usize - block_start;
            let to_copy = std::cmp::min(remaining_data, space_in_block);

            plaintext[block_start..block_start + to_copy]
                .copy_from_slice(&data[data_pos..data_pos + to_copy]);
            data_pos += to_copy;

            // Determine actual block length (don't write trailing zeros for last block)
            let block_data_end = block_start + to_copy;
            let block_len = if bytes_read > 0 {
                // Existing block: keep full block size
                BLOCK_SIZE as usize
            } else {
                block_data_end
            };

            // Encrypt and write
            let nonce = crypto::derive_block_nonce(pool_id, block_idx);
            let encrypted =
                crypto::encrypt_with_nonce(&self.key, &nonce, &plaintext[..block_len])
                    .map_err(|_| PoolError::EncryptionError)?;

            file.seek(SeekFrom::Start(disk_offset))?;
            std::io::Write::write_all(&mut file, &encrypted)?;
        }

        file.sync_all()?;
        Ok(())
    }

    /// Creates a new empty pool file on disk.
    fn create_pool(&self, pool_id: u32) -> Result<(), PoolError> {
        let path = self.pool_path(pool_id);
        std::fs::File::create(&path)?;
        Ok(())
    }

    /// Computes the path of a pool from its id.
    fn pool_path(&self, pool_id: u32) -> PathBuf {
        self.header_dir.join(Self::format_pool_name(pool_id))
    }

    fn format_pool_name(pool_id: u32) -> String {
        format!("pool_{:03}.bin", pool_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_manager(dir: &std::path::Path, tile_size_mb: u64) -> PoolManager {
        let key = [0xABu8; 32];
        let tile_size = tile_size_mb * 1024 * 1024;
        let pool_entry = PoolEntry {
            id: 0,
            filename: "pool_000.bin".to_string(),
            size_bytes: 0,
            checksum: [0u8; 32],
            status: PoolStatus::Active,
        };
        // Create the pool file
        std::fs::File::create(dir.join("pool_000.bin")).unwrap();

        PoolManager::new(dir.to_path_buf(), tile_size, key, vec![pool_entry])
    }

    #[test]
    fn test_pool_write_and_read() {
        let dir = TempDir::new().unwrap();
        let mut mgr = make_manager(dir.path(), 256);

        let data = b"Hello, Mosaic tiles!";
        let (pool_id, offset) = mgr.allocate(data.len() as u64).unwrap();
        assert_eq!(pool_id, 0);
        assert_eq!(offset, 0);

        mgr.write(pool_id, offset, data).unwrap();
        let read_back = mgr.read(pool_id, offset, data.len() as u64).unwrap();
        assert_eq!(read_back, data);
    }

    #[test]
    fn test_pool_multiple_writes() {
        let dir = TempDir::new().unwrap();
        let mut mgr = make_manager(dir.path(), 256);

        let data1 = vec![0xAAu8; 1000];
        let data2 = vec![0xBBu8; 2000];

        let (id1, off1) = mgr.allocate(data1.len() as u64).unwrap();
        mgr.write(id1, off1, &data1).unwrap();

        let (id2, off2) = mgr.allocate(data2.len() as u64).unwrap();
        mgr.write(id2, off2, &data2).unwrap();

        let r1 = mgr.read(id1, off1, data1.len() as u64).unwrap();
        assert_eq!(r1, data1);

        let r2 = mgr.read(id2, off2, data2.len() as u64).unwrap();
        assert_eq!(r2, data2);
    }

    #[test]
    fn test_pool_large_data_spanning_blocks() {
        let dir = TempDir::new().unwrap();
        let mut mgr = make_manager(dir.path(), 256);

        // Write data larger than one 64KB block
        let data = vec![0xCCu8; 100_000];
        let (pool_id, offset) = mgr.allocate(data.len() as u64).unwrap();
        mgr.write(pool_id, offset, &data).unwrap();

        let read_back = mgr.read(pool_id, offset, data.len() as u64).unwrap();
        assert_eq!(read_back, data);
    }

    #[test]
    fn test_pool_allocation_creates_new_tile() {
        let dir = TempDir::new().unwrap();
        // Very small tile: 1 MB
        let mut mgr = make_manager(dir.path(), 1);

        // Allocate more than 1 MB total
        let data = vec![0xDD; 600_000];
        let (id1, _) = mgr.allocate(data.len() as u64).unwrap();
        assert_eq!(id1, 0);

        // This should spill into pool_001
        let (id2, _) = mgr.allocate(data.len() as u64).unwrap();
        assert_eq!(id2, 1);
        assert!(dir.path().join("pool_001.bin").exists());
        assert_eq!(mgr.pool_index().len(), 2);
    }

    #[test]
    fn test_pool_not_found() {
        let dir = TempDir::new().unwrap();
        let mgr = make_manager(dir.path(), 256);
        let result = mgr.read(99, 0, 100);
        assert!(matches!(result, Err(PoolError::PoolNotFound(99))));
    }
}
