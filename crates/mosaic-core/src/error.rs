#[derive(thiserror::Error, Debug)]
pub enum CryptoError {
    #[error("Key derivation failed")]
    KeyDerivationFailed,
    #[error("Encryption failed")]
    EncryptionFailed,
    #[error("Decryption failed")]
    DecryptionFailed,
    #[error("Invalid ciphertext length")]
    InvalidCiphertextLength,
}

#[derive(thiserror::Error, Debug)]
pub enum HeaderError {
    #[error("Invalid magic bytes — not a Mosaic vault")]
    InvalidMagic,
    #[error("Decryption failed — wrong password or corrupted header")]
    DecryptionFailed,
    #[error("Header integrity check failed")]
    IntegrityFailed,
    #[error("Unsupported vault version: {0}")]
    UnsupportedVersion(u16),
    #[error("Vault name too long (max 64 characters)")]
    NameTooLong,
    #[error("Serialization error: {0}")]
    Serialization(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(thiserror::Error, Debug)]
pub enum PoolError {
    #[error("Pool {0} not found")]
    PoolNotFound(u32),
    #[error("Offset {offset} + size {size} exceeds pool capacity")]
    OutOfBounds { offset: u64, size: u64 },
    #[error("Encryption error in pool")]
    EncryptionError,
    #[error("Decryption error in pool")]
    DecryptionError,
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(thiserror::Error, Debug)]
pub enum IndexError {
    #[error("File not found: {0}")]
    FileNotFound(String),
    #[error("File already exists: {0}")]
    FileAlreadyExists(String),
    #[error("Not a directory: {0}")]
    NotADirectory(String),
}
