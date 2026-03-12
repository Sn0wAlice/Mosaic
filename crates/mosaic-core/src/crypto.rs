use argon2::{Argon2, Params, Version};
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Nonce,
};
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

use crate::error::CryptoError;

/// Nonce size for ChaCha20-Poly1305
pub const NONCE_SIZE: usize = 12;
/// Authentication tag size for ChaCha20-Poly1305
pub const TAG_SIZE: usize = 16;

/// Derives a 32-byte key from a password via Argon2id.
/// Parameters: m_cost=65536 KB, t_cost=3, p_cost=4
pub fn derive_key(password: &[u8], salt: &[u8; 32]) -> Result<[u8; 32], CryptoError> {
    derive_key_with_params(password, salt, 65536, 3, 4)
}

/// Derives a key with custom Argon2id parameters.
pub fn derive_key_with_params(
    password: &[u8],
    salt: &[u8; 32],
    m_cost: u32,
    t_cost: u32,
    p_cost: u32,
) -> Result<[u8; 32], CryptoError> {
    let params = Params::new(m_cost, t_cost, p_cost, Some(32))
        .map_err(|_| CryptoError::KeyDerivationFailed)?;
    let argon2 = Argon2::new(argon2::Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0u8; 32];
    argon2
        .hash_password_into(password, salt, &mut key)
        .map_err(|_| CryptoError::KeyDerivationFailed)?;
    Ok(key)
}

/// Encrypts a block of data with ChaCha20-Poly1305.
/// Returns: nonce (12 bytes) || ciphertext || tag (16 bytes)
pub fn encrypt(key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
    let cipher = ChaCha20Poly1305::new_from_slice(key)
        .map_err(|_| CryptoError::EncryptionFailed)?;

    let mut nonce_bytes = [0u8; NONCE_SIZE];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|_| CryptoError::EncryptionFailed)?;

    let mut result = Vec::with_capacity(NONCE_SIZE + ciphertext.len());
    result.extend_from_slice(&nonce_bytes);
    result.extend_from_slice(&ciphertext);
    Ok(result)
}

/// Encrypts with a specific nonce (for pool block encryption where nonce is deterministic).
pub fn encrypt_with_nonce(
    key: &[u8; 32],
    nonce_bytes: &[u8; NONCE_SIZE],
    plaintext: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    let cipher = ChaCha20Poly1305::new_from_slice(key)
        .map_err(|_| CryptoError::EncryptionFailed)?;
    let nonce = Nonce::from_slice(nonce_bytes);
    cipher
        .encrypt(nonce, plaintext)
        .map_err(|_| CryptoError::EncryptionFailed)
}

/// Decrypts a block. The nonce is extracted from the first 12 bytes.
pub fn decrypt(key: &[u8; 32], ciphertext: &[u8]) -> Result<Vec<u8>, CryptoError> {
    if ciphertext.len() < NONCE_SIZE + TAG_SIZE {
        return Err(CryptoError::InvalidCiphertextLength);
    }

    let (nonce_bytes, ct) = ciphertext.split_at(NONCE_SIZE);
    let cipher = ChaCha20Poly1305::new_from_slice(key)
        .map_err(|_| CryptoError::DecryptionFailed)?;
    let nonce = Nonce::from_slice(nonce_bytes);

    let plaintext = cipher
        .decrypt(nonce, ct)
        .map_err(|_| CryptoError::DecryptionFailed)?;

    // The plaintext will be zeroized by the caller if needed
    Ok(plaintext)
}

/// Decrypts with a specific nonce (for pool block decryption).
pub fn decrypt_with_nonce(
    key: &[u8; 32],
    nonce_bytes: &[u8; NONCE_SIZE],
    ciphertext: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    let cipher = ChaCha20Poly1305::new_from_slice(key)
        .map_err(|_| CryptoError::DecryptionFailed)?;
    let nonce = Nonce::from_slice(nonce_bytes);
    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| CryptoError::DecryptionFailed)
}

/// Computes HMAC-SHA256 for header integrity.
pub fn compute_hmac(key: &[u8; 32], data: &[u8]) -> [u8; 32] {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(key).expect("HMAC accepts any key size");
    mac.update(data);
    let result = mac.finalize();
    let mut output = [0u8; 32];
    output.copy_from_slice(&result.into_bytes());
    output
}

/// Verifies HMAC-SHA256.
pub fn verify_hmac(key: &[u8; 32], data: &[u8], expected: &[u8; 32]) -> bool {
    let _computed = compute_hmac(key, data);
    // Constant-time comparison via hmac crate
    let mut mac = <HmacSha256 as Mac>::new_from_slice(key).expect("HMAC accepts any key size");
    mac.update(data);
    mac.verify_slice(expected).is_ok()
}

/// Derives a deterministic nonce from pool_id and block_index.
/// Used for random-access encryption/decryption within pools.
pub fn derive_block_nonce(pool_id: u32, block_index: u64) -> [u8; NONCE_SIZE] {
    let mut nonce = [0u8; NONCE_SIZE];
    nonce[0..4].copy_from_slice(&pool_id.to_le_bytes());
    nonce[4..12].copy_from_slice(&block_index.to_le_bytes());
    nonce
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kdf_deterministic() {
        let password = b"test-password-123";
        let salt = [42u8; 32];
        let key1 = derive_key(password, &salt).unwrap();
        let key2 = derive_key(password, &salt).unwrap();
        assert_eq!(key1, key2);
    }

    #[test]
    fn test_kdf_different_passwords() {
        let salt = [42u8; 32];
        let key1 = derive_key(b"password1", &salt).unwrap();
        let key2 = derive_key(b"password2", &salt).unwrap();
        assert_ne!(key1, key2);
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let key = [1u8; 32];
        let plaintext = b"Hello, Mosaic! This is secret data.";
        let ciphertext = encrypt(&key, plaintext).unwrap();
        assert_ne!(&ciphertext[NONCE_SIZE..], plaintext.as_slice());
        let decrypted = decrypt(&key, &ciphertext).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_encrypt_decrypt_empty() {
        let key = [2u8; 32];
        let plaintext = b"";
        let ciphertext = encrypt(&key, plaintext).unwrap();
        let decrypted = decrypt(&key, &ciphertext).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_decrypt_wrong_key() {
        let key1 = [1u8; 32];
        let key2 = [2u8; 32];
        let plaintext = b"secret";
        let ciphertext = encrypt(&key1, plaintext).unwrap();
        assert!(decrypt(&key2, &ciphertext).is_err());
    }

    #[test]
    fn test_decrypt_invalid_length() {
        let key = [1u8; 32];
        assert!(decrypt(&key, &[0u8; 10]).is_err());
    }

    #[test]
    fn test_hmac_tamper_detection() {
        let key = [3u8; 32];
        let data = b"important header data";
        let mac = compute_hmac(&key, data);
        assert!(verify_hmac(&key, data, &mac));

        // Tamper with one byte
        let mut tampered = data.to_vec();
        tampered[0] ^= 0x01;
        assert!(!verify_hmac(&key, &tampered, &mac));

        // Tamper with MAC
        let mut bad_mac = mac;
        bad_mac[0] ^= 0x01;
        assert!(!verify_hmac(&key, data, &bad_mac));
    }

    #[test]
    fn test_encrypt_decrypt_with_nonce() {
        let key = [4u8; 32];
        let nonce = derive_block_nonce(0, 0);
        let plaintext = b"block data";
        let ct = encrypt_with_nonce(&key, &nonce, plaintext).unwrap();
        let pt = decrypt_with_nonce(&key, &nonce, &ct).unwrap();
        assert_eq!(pt, plaintext);
    }

    #[test]
    fn test_block_nonce_uniqueness() {
        let n1 = derive_block_nonce(0, 0);
        let n2 = derive_block_nonce(0, 1);
        let n3 = derive_block_nonce(1, 0);
        assert_ne!(n1, n2);
        assert_ne!(n1, n3);
        assert_ne!(n2, n3);
    }
}
