use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use zeroize::Zeroize;

use crate::crypto;
use crate::error::HeaderError;
use crate::index::{FileIndex, FileIndexLegacy};

/// Total header file size: 10 MB
const HEADER_SIZE: usize = 10 * 1024 * 1024;
/// Prelude (unencrypted) size: 128 bytes
const PRELUDE_SIZE: usize = 128;
/// Size of encrypted data length field (u64, 8 bytes, right after prelude)
const LENGTH_FIELD_SIZE: usize = 8;
/// HMAC size at the end: 32 bytes
const HMAC_SIZE: usize = 32;
/// Magic bytes: "MOSC"
const MAGIC: [u8; 4] = [0x4D, 0x4F, 0x53, 0x43];
/// Current format version
const FORMAT_VERSION: u16 = 1;

/// Unencrypted prelude (first 128 bytes of vault.header)
#[derive(Clone, Debug)]
pub struct VaultPrelude {
    pub magic: [u8; 4],
    pub version: u16,
    pub argon2_salt: [u8; 32],
    pub argon2_m_cost: u32,
    pub argon2_t_cost: u32,
    pub argon2_p_cost: u32,
    pub header_nonce: [u8; 12],
    pub _reserved: [u8; 38],
}

impl VaultPrelude {
    fn to_bytes(&self) -> Result<[u8; PRELUDE_SIZE], HeaderError> {
        let mut buf = [0u8; PRELUDE_SIZE];
        buf[0..4].copy_from_slice(&self.magic);
        buf[4..6].copy_from_slice(&self.version.to_le_bytes());
        buf[6..38].copy_from_slice(&self.argon2_salt);
        buf[38..42].copy_from_slice(&self.argon2_m_cost.to_le_bytes());
        buf[42..46].copy_from_slice(&self.argon2_t_cost.to_le_bytes());
        buf[46..50].copy_from_slice(&self.argon2_p_cost.to_le_bytes());
        buf[50..62].copy_from_slice(&self.header_nonce);
        // _reserved stays zero
        Ok(buf)
    }

    fn from_bytes(buf: &[u8; PRELUDE_SIZE]) -> Result<Self, HeaderError> {
        let mut magic = [0u8; 4];
        magic.copy_from_slice(&buf[0..4]);
        if magic != MAGIC {
            return Err(HeaderError::InvalidMagic);
        }

        let version = u16::from_le_bytes([buf[4], buf[5]]);
        if version != FORMAT_VERSION {
            return Err(HeaderError::UnsupportedVersion(version));
        }

        let mut argon2_salt = [0u8; 32];
        argon2_salt.copy_from_slice(&buf[6..38]);

        let argon2_m_cost = u32::from_le_bytes([buf[38], buf[39], buf[40], buf[41]]);
        let argon2_t_cost = u32::from_le_bytes([buf[42], buf[43], buf[44], buf[45]]);
        let argon2_p_cost = u32::from_le_bytes([buf[46], buf[47], buf[48], buf[49]]);

        let mut header_nonce = [0u8; 12];
        header_nonce.copy_from_slice(&buf[50..62]);

        let mut _reserved = [0u8; 38];
        _reserved.copy_from_slice(&buf[62..100]);

        Ok(Self {
            magic,
            version,
            argon2_salt,
            argon2_m_cost,
            argon2_t_cost,
            argon2_p_cost,
            header_nonce,
            _reserved,
        })
    }
}

/// Encrypted part of the header
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct VaultHeader {
    pub metadata: VaultMetadata,
    pub pool_index: Vec<PoolEntry>,
    pub file_index: FileIndex,
}

impl Zeroize for VaultHeader {
    fn zeroize(&mut self) {
        self.metadata.zeroize();
        self.pool_index.clear();
        self.file_index.zeroize();
    }
}

impl Drop for VaultHeader {
    fn drop(&mut self) {
        self.zeroize();
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct VaultMetadata {
    pub uuid: [u8; 16],
    pub name: String,
    pub created_at: u64,
    pub tile_size_bytes: u64,
    pub flags: u32,
}

impl Zeroize for VaultMetadata {
    fn zeroize(&mut self) {
        self.uuid.zeroize();
        self.name.zeroize();
        self.created_at.zeroize();
        self.tile_size_bytes.zeroize();
        self.flags.zeroize();
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PoolEntry {
    pub id: u32,
    pub filename: String,
    pub size_bytes: u64,
    pub checksum: [u8; 32],
    pub status: PoolStatus,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub enum PoolStatus {
    Full,
    Active,
    Pending,
}

impl VaultHeader {
    /// Reads and decrypts a vault.header from disk.
    /// Returns (header, derived_key).
    pub fn open(path: &Path, password: &[u8]) -> Result<(Self, [u8; 32]), HeaderError> {
        let data = std::fs::read(path)?;
        if data.len() < PRELUDE_SIZE + HMAC_SIZE {
            return Err(HeaderError::InvalidMagic);
        }

        let mut prelude_bytes = [0u8; PRELUDE_SIZE];
        prelude_bytes.copy_from_slice(&data[..PRELUDE_SIZE]);
        let prelude = VaultPrelude::from_bytes(&prelude_bytes)?;

        // Derive key
        let key = crypto::derive_key_with_params(
            password,
            &prelude.argon2_salt,
            prelude.argon2_m_cost,
            prelude.argon2_t_cost,
            prelude.argon2_p_cost,
        )
        .map_err(|_| HeaderError::DecryptionFailed)?;

        // Read encrypted data length
        let len_start = PRELUDE_SIZE;
        let len_end = PRELUDE_SIZE + LENGTH_FIELD_SIZE;
        let encrypted_len = u64::from_le_bytes(
            data[len_start..len_end].try_into().unwrap(),
        ) as usize;

        // Extract encrypted portion
        let enc_start = len_end;
        let enc_end = enc_start + encrypted_len;
        if enc_end + HMAC_SIZE > data.len() {
            return Err(HeaderError::DecryptionFailed);
        }
        let encrypted_data = &data[enc_start..enc_end];

        // Verify HMAC over everything except the last 32 bytes
        let hmac_start = data.len() - HMAC_SIZE;
        let mut expected_hmac = [0u8; 32];
        expected_hmac.copy_from_slice(&data[hmac_start..]);

        if !crypto::verify_hmac(&key, &data[..hmac_start], &expected_hmac) {
            return Err(HeaderError::IntegrityFailed);
        }

        // Decrypt
        let plaintext = crypto::decrypt_with_nonce(&key, &prelude.header_nonce, encrypted_data)
            .map_err(|_| HeaderError::DecryptionFailed)?;

        // Deserialize — try current format, fall back to legacy (no directories field)
        let header: VaultHeader = match bincode::deserialize::<VaultHeader>(&plaintext) {
            Ok(h) => h,
            Err(_) => {
                // Legacy vault: FileIndex had no `directories` field
                #[derive(Deserialize)]
                struct VaultHeaderLegacy {
                    metadata: VaultMetadata,
                    pool_index: Vec<PoolEntry>,
                    file_index: FileIndexLegacy,
                }
                let legacy: VaultHeaderLegacy = bincode::deserialize(&plaintext)
                    .map_err(|e| HeaderError::Serialization(e.to_string()))?;
                VaultHeader {
                    metadata: legacy.metadata,
                    pool_index: legacy.pool_index,
                    file_index: FileIndex::from_legacy(legacy.file_index),
                }
            }
        };

        Ok((header, key))
    }

    /// Encrypts and writes the header to disk (10 MB padded).
    pub fn save(
        &self,
        path: &Path,
        key: &[u8; 32],
        prelude: &VaultPrelude,
    ) -> Result<(), HeaderError> {
        let serialized =
            bincode::serialize(self).map_err(|e| HeaderError::Serialization(e.to_string()))?;

        // Encrypt
        let encrypted = crypto::encrypt_with_nonce(key, &prelude.header_nonce, &serialized)
            .map_err(|_| HeaderError::DecryptionFailed)?;

        // Build the full 10 MB buffer
        let prelude_bytes = prelude.to_bytes()?;
        let mut buf = vec![0u8; HEADER_SIZE];
        buf[..PRELUDE_SIZE].copy_from_slice(&prelude_bytes);

        // Write encrypted data length
        let len_start = PRELUDE_SIZE;
        let len_end = len_start + LENGTH_FIELD_SIZE;
        buf[len_start..len_end].copy_from_slice(&(encrypted.len() as u64).to_le_bytes());

        // Encrypted data goes after length field
        let enc_start = len_end;
        let enc_end = enc_start + encrypted.len();
        if enc_end + HMAC_SIZE > HEADER_SIZE {
            return Err(HeaderError::Serialization(
                "Header data too large for 10 MB limit".into(),
            ));
        }
        buf[enc_start..enc_end].copy_from_slice(&encrypted);
        // Rest is zero padding up to HEADER_SIZE - HMAC_SIZE

        // Compute HMAC over everything except the last 32 bytes
        let hmac = crypto::compute_hmac(key, &buf[..HEADER_SIZE - HMAC_SIZE]);
        buf[HEADER_SIZE - HMAC_SIZE..].copy_from_slice(&hmac);

        std::fs::write(path, &buf)?;
        Ok(())
    }

    /// Creates a new empty vault.
    pub fn init(
        path: &Path,
        password: &[u8],
        name: &str,
        tile_size_mb: u64,
    ) -> Result<(), HeaderError> {
        if name.len() > 64 {
            return Err(HeaderError::NameTooLong);
        }

        // Generate salt and nonce
        let mut salt = [0u8; 32];
        let mut nonce = [0u8; 12];
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut salt);
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut nonce);

        let prelude = VaultPrelude {
            magic: MAGIC,
            version: FORMAT_VERSION,
            argon2_salt: salt,
            argon2_m_cost: 65536,
            argon2_t_cost: 3,
            argon2_p_cost: 4,
            header_nonce: nonce,
            _reserved: [0u8; 38],
        };

        // Derive key
        let key = crypto::derive_key(password, &salt)
            .map_err(|_| HeaderError::DecryptionFailed)?;

        // Generate UUID
        let uuid_val = uuid::Uuid::new_v4();
        let mut uuid_bytes = [0u8; 16];
        uuid_bytes.copy_from_slice(uuid_val.as_bytes());

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let tile_size_bytes = tile_size_mb * 1024 * 1024;

        // Create first pool entry (Active, pending creation on disk)
        let first_pool = PoolEntry {
            id: 0,
            filename: "pool_000.bin".to_string(),
            size_bytes: 0,
            checksum: [0u8; 32],
            status: PoolStatus::Active,
        };

        let header = VaultHeader {
            metadata: VaultMetadata {
                uuid: uuid_bytes,
                name: name.to_string(),
                created_at: now,
                tile_size_bytes,
                flags: 0,
            },
            pool_index: vec![first_pool],
            file_index: FileIndex::new(),
        };

        header.save(path, &key, &prelude)?;

        // Create the first empty pool file
        let pool_path = path.parent().unwrap_or(Path::new(".")).join("pool_000.bin");
        std::fs::write(&pool_path, &[])?;

        Ok(())
    }

    /// Verifies the HMAC of the header.
    pub fn verify_integrity(&self, key: &[u8; 32], raw_data: &[u8]) -> bool {
        if raw_data.len() < HMAC_SIZE {
            return false;
        }
        let data_end = raw_data.len() - HMAC_SIZE;
        let mut expected = [0u8; 32];
        expected.copy_from_slice(&raw_data[data_end..]);
        crypto::verify_hmac(key, &raw_data[..data_end], &expected)
    }
}

/// Returns the VaultPrelude from a header file without decrypting.
pub fn read_prelude(path: &Path) -> Result<VaultPrelude, HeaderError> {
    let mut buf = [0u8; PRELUDE_SIZE];
    let data = std::fs::read(path)?;
    if data.len() < PRELUDE_SIZE {
        return Err(HeaderError::InvalidMagic);
    }
    buf.copy_from_slice(&data[..PRELUDE_SIZE]);
    VaultPrelude::from_bytes(&buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_header_init_and_open() {
        let dir = TempDir::new().unwrap();
        let header_path = dir.path().join("vault.header");
        let password = b"my-secure-password";

        VaultHeader::init(&header_path, password, "test-vault", 256).unwrap();

        assert!(header_path.exists());
        // Header should be exactly 10 MB
        let metadata = std::fs::metadata(&header_path).unwrap();
        assert_eq!(metadata.len(), HEADER_SIZE as u64);

        // Pool file should exist
        assert!(dir.path().join("pool_000.bin").exists());

        // Open with correct password
        let (header, _key) = VaultHeader::open(&header_path, password).unwrap();
        assert_eq!(header.metadata.name, "test-vault");
        assert_eq!(header.metadata.tile_size_bytes, 256 * 1024 * 1024);
        assert_eq!(header.pool_index.len(), 1);
        assert_eq!(header.pool_index[0].status, PoolStatus::Active);
    }

    #[test]
    fn test_header_wrong_password() {
        let dir = TempDir::new().unwrap();
        let header_path = dir.path().join("vault.header");

        VaultHeader::init(&header_path, b"correct-password", "vault", 256).unwrap();

        let result = VaultHeader::open(&header_path, b"wrong-password");
        assert!(result.is_err());
        match result {
            Err(HeaderError::IntegrityFailed) | Err(HeaderError::DecryptionFailed) => {}
            other => panic!("Expected DecryptionFailed or IntegrityFailed, got {:?}", other),
        }
    }

    #[test]
    fn test_header_roundtrip_with_data() {
        let dir = TempDir::new().unwrap();
        let header_path = dir.path().join("vault.header");
        let password = b"test-pass";

        VaultHeader::init(&header_path, password, "data-vault", 128).unwrap();

        // Open, modify, save, reopen
        let (mut header, key) = VaultHeader::open(&header_path, password).unwrap();
        let prelude = read_prelude(&header_path).unwrap();

        use crate::index::{FileEntry, FileSegment};
        header.file_index.insert(
            "test.txt",
            FileEntry {
                size: 1024,
                created_at: 1700000000,
                modified_at: 1700000000,
                segments: vec![FileSegment {
                    pool_id: 0,
                    offset: 0,
                    length: 1024,
                }],
            },
        );

        header.save(&header_path, &key, &prelude).unwrap();

        let (header2, _) = VaultHeader::open(&header_path, password).unwrap();
        assert!(header2.file_index.get("test.txt").is_some());
        assert_eq!(header2.file_index.get("test.txt").unwrap().size, 1024);
    }

    #[test]
    fn test_header_invalid_magic() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("bad.header");
        std::fs::write(&path, vec![0u8; HEADER_SIZE]).unwrap();

        let result = VaultHeader::open(&path, b"any");
        assert!(matches!(result, Err(HeaderError::InvalidMagic)));
    }

    #[test]
    fn test_name_too_long() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("vault.header");
        let long_name = "a".repeat(65);
        let result = VaultHeader::init(&path, b"pass", &long_name, 256);
        assert!(matches!(result, Err(HeaderError::NameTooLong)));
    }
}
