use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng},
    Aes256Gcm, Key, Nonce,
};
use anyhow::{anyhow, Result};

/// Fixed key for initial implementation. In production, this would be exchanged securely.
const FIXED_KEY: &[u8; 32] = b"vigilant_parakeet_fixed_key_256!";

/// Encrypt plaintext using AES-256-GCM with a random nonce.
/// Returns encrypted data with nonce prepended (12 bytes nonce + ciphertext).
///
/// Note: Encryption adds 28 bytes of overhead (12-byte nonce + 16-byte auth tag).
/// To ensure encrypted packets don't exceed MTU, input should be limited to MTU - 28 bytes.
pub fn encrypt_payload(plaintext: &[u8]) -> Result<Vec<u8>> {
    // AES-GCM adds 28 bytes overhead: 12-byte nonce + 16-byte authentication tag
    const ENCRYPTION_OVERHEAD: usize = 12 + 16;
    const MAX_MTU: usize = 1500;
    const MAX_PLAINTEXT_SIZE: usize = MAX_MTU - ENCRYPTION_OVERHEAD;

    if plaintext.len() > MAX_PLAINTEXT_SIZE {
        return Err(anyhow!(
            "Plaintext too large for encryption: {} bytes exceeds maximum {} bytes (MTU {} - overhead {})",
            plaintext.len(),
            MAX_PLAINTEXT_SIZE,
            MAX_MTU,
            ENCRYPTION_OVERHEAD
        ));
    }

    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(FIXED_KEY));
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);

    let ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .map_err(|e| anyhow!("Encryption failed: {}", e))?;

    // Prepend nonce to ciphertext for transmission
    let mut result = nonce.to_vec();
    result.extend_from_slice(&ciphertext);
    Ok(result)
}

/// Decrypt data that was encrypted with encrypt_payload.
/// Expects nonce (12 bytes) + ciphertext.
pub fn decrypt_payload(encrypted_data: &[u8]) -> Result<Vec<u8>> {
    if encrypted_data.len() < 12 {
        return Err(anyhow!("Encrypted data too short (missing nonce)"));
    }

    let (nonce_bytes, ciphertext) = encrypted_data.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);

    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(FIXED_KEY));
    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| anyhow!("Decryption failed: {}", e))?;

    Ok(plaintext)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let plaintext = b"test payload data";
        let encrypted = encrypt_payload(plaintext).expect("encryption failed");
        let decrypted = decrypt_payload(&encrypted).expect("decryption failed");
        assert_eq!(plaintext, decrypted.as_slice());
    }

    #[test]
    fn encrypt_produces_different_ciphertext() {
        let plaintext = b"test payload data";
        let encrypted1 = encrypt_payload(plaintext).expect("encryption failed");
        let encrypted2 = encrypt_payload(plaintext).expect("encryption failed");
        // Should be different due to random nonce
        assert_ne!(encrypted1, encrypted2);
    }

    #[test]
    fn decrypt_invalid_data_fails() {
        let invalid_data = b"too short";
        assert!(decrypt_payload(invalid_data).is_err());
    }

    #[test]
    fn decrypt_wrong_key_fails() {
        let plaintext = b"test data";
        let encrypted = encrypt_payload(plaintext).expect("encryption failed");

        // Modify the ciphertext to simulate wrong key/corruption
        let mut corrupted = encrypted;
        corrupted[15] ^= 0x01; // Flip a bit in the ciphertext

        assert!(decrypt_payload(&corrupted).is_err());
    }

    #[test]
    fn encrypt_large_payload_fails() {
        // Create a payload that would exceed MTU after encryption
        // Max plaintext size is 1500 - 28 = 1472 bytes
        let large_plaintext = vec![0u8; 1473]; // 1 byte over the limit
        let result = encrypt_payload(&large_plaintext);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Plaintext too large"));
    }

    #[test]
    fn encrypt_max_size_payload_succeeds() {
        // Create a payload at the maximum allowed size
        // Max plaintext size is 1500 - 28 = 1472 bytes
        let max_plaintext = vec![0u8; 1472];
        let encrypted = encrypt_payload(&max_plaintext).expect("encryption should succeed");

        // Verify the encrypted size doesn't exceed MTU
        assert!(encrypted.len() <= 1500);

        // Verify round-trip decryption works
        let decrypted = decrypt_payload(&encrypted).expect("decryption should succeed");
        assert_eq!(max_plaintext, decrypted);
    }
}
