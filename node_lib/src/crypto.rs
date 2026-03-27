use crate::error::NodeError;
use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng},
    Aes256Gcm, Key, Nonce,
};
use hkdf::Hkdf;
use sha2::Sha256;
use x25519_dalek::{EphemeralSecret, PublicKey, SharedSecret, StaticSecret};

/// Fixed key for backward compatibility when DH is not enabled.
const FIXED_KEY: &[u8; 32] = b"vigilant_parakeet_fixed_key_256!";

/// HKDF info string for deriving AES-256 keys from DH shared secrets.
const HKDF_INFO: &[u8] = b"vigilant-parakeet-dh-aes256gcm";

/// A Diffie-Hellman keypair for key exchange.
pub struct DhKeypair {
    pub secret: StaticSecret,
    pub public: PublicKey,
}

impl DhKeypair {
    /// Generate a new random DH keypair.
    pub fn generate() -> Self {
        let secret = StaticSecret::random_from_rng(OsRng);
        let public = PublicKey::from(&secret);
        Self { secret, public }
    }

    /// Compute the shared secret with a peer's public key.
    pub fn diffie_hellman(&self, peer_public: &PublicKey) -> SharedSecret {
        self.secret.diffie_hellman(peer_public)
    }
}

/// Generate an ephemeral DH keypair (single use, consumed on DH).
pub fn generate_ephemeral_keypair() -> (EphemeralSecret, PublicKey) {
    let secret = EphemeralSecret::random_from_rng(OsRng);
    let public = PublicKey::from(&secret);
    (secret, public)
}

/// Derive an AES-256-GCM key from a DH shared secret using HKDF-SHA256.
///
/// The `key_id` is mixed into the HKDF salt to bind the derived key to a
/// specific exchange round, ensuring each re-key produces a distinct key.
pub fn derive_key_from_shared_secret(shared_secret: &[u8; 32], key_id: u32) -> [u8; 32] {
    let mut salt = Vec::with_capacity(36);
    salt.extend_from_slice(b"vigilant-parakeet-salt-");
    salt.extend_from_slice(&key_id.to_be_bytes());

    let hk = Hkdf::<Sha256>::new(Some(&salt), shared_secret);
    let mut okm = [0u8; 32];
    hk.expand(HKDF_INFO, &mut okm)
        .expect("HKDF expand should not fail for 32-byte output");
    okm
}

/// Encrypt plaintext using AES-256-GCM with a random nonce and the given key.
/// Returns encrypted data with nonce prepended (12 bytes nonce + ciphertext).
///
/// Note: Encryption adds 28 bytes of overhead (12-byte nonce + 16-byte auth tag).
/// MTU is set to 1400 bytes at the interface level to account for this overhead
/// plus the cloud protocol (15B) and UDP/IP (28B) wrapping on the RSU->Server path.
pub fn encrypt_payload_with_key(plaintext: &[u8], key: &[u8; 32]) -> Result<Vec<u8>, NodeError> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);

    let ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .map_err(|e| NodeError::EncryptionError(e.to_string()))?;

    let mut result = nonce.to_vec();
    result.extend_from_slice(&ciphertext);
    Ok(result)
}

/// Decrypt data that was encrypted with encrypt_payload_with_key.
/// Expects nonce (12 bytes) + ciphertext.
pub fn decrypt_payload_with_key(
    encrypted_data: &[u8],
    key: &[u8; 32],
) -> Result<Vec<u8>, NodeError> {
    if encrypted_data.len() < 12 {
        return Err(NodeError::EncryptedDataTooShort(encrypted_data.len()));
    }

    let (nonce_bytes, ciphertext) = encrypted_data.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);

    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| NodeError::DecryptionError(e.to_string()))?;

    Ok(plaintext)
}

/// Encrypt plaintext using AES-256-GCM with the fixed key.
/// Returns encrypted data with nonce prepended (12 bytes nonce + ciphertext).
pub fn encrypt_payload(plaintext: &[u8]) -> Result<Vec<u8>, NodeError> {
    encrypt_payload_with_key(plaintext, FIXED_KEY)
}

/// Decrypt data that was encrypted with encrypt_payload (fixed key).
/// Expects nonce (12 bytes) + ciphertext.
pub fn decrypt_payload(encrypted_data: &[u8]) -> Result<Vec<u8>, NodeError> {
    decrypt_payload_with_key(encrypted_data, FIXED_KEY)
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

    #[test]
    fn dh_keypair_generation_produces_different_keys() {
        let kp1 = DhKeypair::generate();
        let kp2 = DhKeypair::generate();
        assert_ne!(kp1.public.as_bytes(), kp2.public.as_bytes());
    }

    #[test]
    fn dh_shared_secret_is_symmetric() {
        let alice = DhKeypair::generate();
        let bob = DhKeypair::generate();

        let secret_ab = alice.diffie_hellman(&bob.public);
        let secret_ba = bob.diffie_hellman(&alice.public);

        assert_eq!(secret_ab.as_bytes(), secret_ba.as_bytes());
    }

    #[test]
    fn dh_derived_key_encrypt_decrypt_roundtrip() {
        let alice = DhKeypair::generate();
        let bob = DhKeypair::generate();

        let shared = alice.diffie_hellman(&bob.public);
        let key = derive_key_from_shared_secret(shared.as_bytes(), 1);

        let plaintext = b"secret vehicular data";
        let encrypted =
            encrypt_payload_with_key(plaintext, &key).expect("encryption should succeed");
        let decrypted =
            decrypt_payload_with_key(&encrypted, &key).expect("decryption should succeed");

        assert_eq!(plaintext, decrypted.as_slice());
    }

    #[test]
    fn different_key_ids_produce_different_derived_keys() {
        let shared_secret = [42u8; 32];
        let key1 = derive_key_from_shared_secret(&shared_secret, 1);
        let key2 = derive_key_from_shared_secret(&shared_secret, 2);
        assert_ne!(key1, key2);
    }

    #[test]
    fn dh_key_cannot_decrypt_fixed_key_ciphertext() {
        let alice = DhKeypair::generate();
        let bob = DhKeypair::generate();
        let shared = alice.diffie_hellman(&bob.public);
        let dh_key = derive_key_from_shared_secret(shared.as_bytes(), 1);

        let plaintext = b"test data";
        let encrypted_fixed = encrypt_payload(plaintext).expect("encrypt with fixed key");
        assert!(decrypt_payload_with_key(&encrypted_fixed, &dh_key).is_err());
    }

    #[test]
    fn ephemeral_keypair_dh_roundtrip() {
        let (alice_secret, alice_public) = generate_ephemeral_keypair();
        let bob = DhKeypair::generate();

        let shared_a = alice_secret.diffie_hellman(&bob.public);
        let shared_b = bob.diffie_hellman(&alice_public);
        assert_eq!(shared_a.as_bytes(), shared_b.as_bytes());
    }
}
