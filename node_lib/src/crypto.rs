use crate::error::NodeError;
use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng},
    Aes256Gcm, Key, Nonce,
};

/// Fixed key for initial implementation. In production, this would be exchanged securely.
const FIXED_KEY: &[u8; 32] = b"vigilant_parakeet_fixed_key_256!";

/// Encrypt plaintext using AES-256-GCM with a random nonce.
/// Returns encrypted data with nonce prepended (12 bytes nonce + ciphertext).
///
/// Note: Encryption adds 28 bytes of overhead (12-byte nonce + 16-byte auth tag).
/// MTU is set to 1436 bytes at the interface level to account for this overhead.
pub fn encrypt_payload(plaintext: &[u8]) -> Result<Vec<u8>, NodeError> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(FIXED_KEY));
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);

    let ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .map_err(|e| NodeError::EncryptionError(e.to_string()))?;

    // Prepend nonce to ciphertext for transmission
    let mut result = nonce.to_vec();
    result.extend_from_slice(&ciphertext);
    Ok(result)
}

/// Decrypt data that was encrypted with encrypt_payload.
/// Expects nonce (12 bytes) + ciphertext.
pub fn decrypt_payload(encrypted_data: &[u8]) -> Result<Vec<u8>, NodeError> {
    if encrypted_data.len() < 12 {
        return Err(NodeError::EncryptedDataTooShort(encrypted_data.len()));
    }

    let (nonce_bytes, ciphertext) = encrypted_data.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);

    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(FIXED_KEY));
    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| NodeError::DecryptionError(e.to_string()))?;

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
    fn encrypt_large_payload_succeeds() {
        // Test that we can encrypt large payloads now that interface MTU handles fragmentation
        let large_plaintext = vec![0u8; 2000]; // Larger than previous 1436 limit
        let encrypted = encrypt_payload(&large_plaintext).expect("encryption should succeed");

        // Verify round-trip decryption works
        let decrypted = decrypt_payload(&encrypted).expect("decryption should succeed");
        assert_eq!(large_plaintext, decrypted);
    }

    #[test]
    fn encrypt_payload_adds_overhead() {
        // Verify encryption adds exactly 28 bytes of overhead (12 nonce + 16 tag)
        let plaintext = vec![0u8; 100];
        let encrypted = encrypt_payload(&plaintext).expect("encryption should succeed");
        assert_eq!(encrypted.len(), plaintext.len() + 28);

        // Verify round-trip works
        let decrypted = decrypt_payload(&encrypted).expect("decryption should succeed");
        assert_eq!(plaintext, decrypted);
    }
}
