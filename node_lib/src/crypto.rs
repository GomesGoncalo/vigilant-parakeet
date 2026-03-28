use crate::error::NodeError;
use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng},
    Aes128Gcm, Aes256Gcm, Key, Nonce,
};
use chacha20poly1305::ChaCha20Poly1305;
use hkdf::Hkdf;
use sha2::{Sha256, Sha384, Sha512};
use x25519_dalek::{EphemeralSecret, PublicKey, SharedSecret, StaticSecret};

/// Fixed key for backward compatibility when DH is not enabled.
pub const FIXED_KEY: &[u8; 32] = b"vigilant_parakeet_fixed_key_256!";

/// HKDF info string for deriving keys from DH shared secrets.
const HKDF_INFO: &[u8] = b"vigilant-parakeet-dh-aes256gcm";

// ── Configurable cipher suite enums ────────────────────────────────

/// Symmetric cipher algorithm for payload encryption.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum SymmetricCipher {
    /// AES-256-GCM (default) — 32-byte key, 12-byte nonce, 16-byte tag.
    #[default]
    Aes256Gcm,
    /// AES-128-GCM — 16-byte key, 12-byte nonce, 16-byte tag.
    Aes128Gcm,
    /// ChaCha20-Poly1305 — 32-byte key, 12-byte nonce, 16-byte tag.
    /// Better performance on hardware without AES-NI.
    ChaCha20Poly1305,
}

impl std::fmt::Display for SymmetricCipher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Aes256Gcm => write!(f, "aes-256-gcm"),
            Self::Aes128Gcm => write!(f, "aes-128-gcm"),
            Self::ChaCha20Poly1305 => write!(f, "chacha20-poly1305"),
        }
    }
}

impl std::str::FromStr for SymmetricCipher {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "aes-256-gcm" | "aes256gcm" => Ok(Self::Aes256Gcm),
            "aes-128-gcm" | "aes128gcm" => Ok(Self::Aes128Gcm),
            "chacha20-poly1305" | "chacha20poly1305" => Ok(Self::ChaCha20Poly1305),
            _ => Err(format!(
                "unknown cipher '{}', expected: aes-256-gcm, aes-128-gcm, chacha20-poly1305",
                s
            )),
        }
    }
}

impl SymmetricCipher {
    /// Key length in bytes required by this cipher.
    pub fn key_len(self) -> usize {
        match self {
            Self::Aes256Gcm | Self::ChaCha20Poly1305 => 32,
            Self::Aes128Gcm => 16,
        }
    }
}

/// Key derivation function for deriving symmetric keys from DH shared secrets.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum KdfAlgorithm {
    /// HKDF-SHA256 (default).
    #[default]
    HkdfSha256,
    /// HKDF-SHA384.
    HkdfSha384,
    /// HKDF-SHA512.
    HkdfSha512,
}

impl std::fmt::Display for KdfAlgorithm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HkdfSha256 => write!(f, "hkdf-sha256"),
            Self::HkdfSha384 => write!(f, "hkdf-sha384"),
            Self::HkdfSha512 => write!(f, "hkdf-sha512"),
        }
    }
}

impl std::str::FromStr for KdfAlgorithm {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "hkdf-sha256" | "hkdfsha256" | "sha256" => Ok(Self::HkdfSha256),
            "hkdf-sha384" | "hkdfsha384" | "sha384" => Ok(Self::HkdfSha384),
            "hkdf-sha512" | "hkdfsha512" | "sha512" => Ok(Self::HkdfSha512),
            _ => Err(format!(
                "unknown KDF '{}', expected: hkdf-sha256, hkdf-sha384, hkdf-sha512",
                s
            )),
        }
    }
}

/// DH group for key exchange.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum DhGroup {
    /// X25519 (Curve25519, default) — 32-byte keys.
    #[default]
    X25519,
}

impl std::fmt::Display for DhGroup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::X25519 => write!(f, "x25519"),
        }
    }
}

impl std::str::FromStr for DhGroup {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "x25519" | "curve25519" => Ok(Self::X25519),
            _ => Err(format!("unknown DH group '{}', expected: x25519", s)),
        }
    }
}

/// Complete crypto configuration for a node.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CryptoConfig {
    pub cipher: SymmetricCipher,
    pub kdf: KdfAlgorithm,
    pub dh_group: DhGroup,
}

// ── DH key exchange ────────────────────────────────────────────────

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

// ── Key derivation ─────────────────────────────────────────────────

/// Derive a symmetric key from a DH shared secret using the configured KDF.
///
/// The `key_id` is mixed into the HKDF salt to bind the derived key to a
/// specific exchange round, ensuring each re-key produces a distinct key.
/// `key_len` determines the output size (16 for AES-128, 32 for AES-256/ChaCha20).
pub fn derive_key(
    kdf: KdfAlgorithm,
    shared_secret: &[u8; 32],
    key_id: u32,
    key_len: usize,
) -> Vec<u8> {
    let mut salt = Vec::with_capacity(36);
    salt.extend_from_slice(b"vigilant-parakeet-salt-");
    salt.extend_from_slice(&key_id.to_be_bytes());

    let mut okm = vec![0u8; key_len];

    match kdf {
        KdfAlgorithm::HkdfSha256 => {
            let hk = Hkdf::<Sha256>::new(Some(&salt), shared_secret);
            hk.expand(HKDF_INFO, &mut okm)
                .expect("HKDF-SHA256 expand failed");
        }
        KdfAlgorithm::HkdfSha384 => {
            let hk = Hkdf::<Sha384>::new(Some(&salt), shared_secret);
            hk.expand(HKDF_INFO, &mut okm)
                .expect("HKDF-SHA384 expand failed");
        }
        KdfAlgorithm::HkdfSha512 => {
            let hk = Hkdf::<Sha512>::new(Some(&salt), shared_secret);
            hk.expand(HKDF_INFO, &mut okm)
                .expect("HKDF-SHA512 expand failed");
        }
    }

    okm
}

/// Convenience wrapper: derive a 32-byte key with HKDF-SHA256 (backward compat).
pub fn derive_key_from_shared_secret(shared_secret: &[u8; 32], key_id: u32) -> [u8; 32] {
    let v = derive_key(KdfAlgorithm::HkdfSha256, shared_secret, key_id, 32);
    let mut out = [0u8; 32];
    out.copy_from_slice(&v);
    out
}

// ── Configurable encrypt / decrypt ─────────────────────────────────

/// Encrypt plaintext with the given key using the specified cipher.
///
/// Returns `[nonce (12 bytes) ‖ ciphertext ‖ auth_tag (16 bytes)]`.
/// Total overhead: 28 bytes.
pub fn encrypt_with_config(
    cipher: SymmetricCipher,
    plaintext: &[u8],
    key: &[u8],
) -> Result<Vec<u8>, NodeError> {
    match cipher {
        SymmetricCipher::Aes256Gcm => {
            let c = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
            let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
            let ct = c
                .encrypt(&nonce, plaintext)
                .map_err(|e| NodeError::EncryptionError(e.to_string()))?;
            let mut out = nonce.to_vec();
            out.extend_from_slice(&ct);
            Ok(out)
        }
        SymmetricCipher::Aes128Gcm => {
            let c = Aes128Gcm::new(Key::<Aes128Gcm>::from_slice(key));
            let nonce = Aes128Gcm::generate_nonce(&mut OsRng);
            let ct = c
                .encrypt(&nonce, plaintext)
                .map_err(|e| NodeError::EncryptionError(e.to_string()))?;
            let mut out = nonce.to_vec();
            out.extend_from_slice(&ct);
            Ok(out)
        }
        SymmetricCipher::ChaCha20Poly1305 => {
            let c = ChaCha20Poly1305::new(Key::<ChaCha20Poly1305>::from_slice(key));
            let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
            let ct = c
                .encrypt(&nonce, plaintext)
                .map_err(|e| NodeError::EncryptionError(e.to_string()))?;
            let mut out = nonce.to_vec();
            out.extend_from_slice(&ct);
            Ok(out)
        }
    }
}

/// Decrypt data produced by `encrypt_with_config`.
/// Expects `[nonce (12 bytes) ‖ ciphertext ‖ auth_tag]`.
pub fn decrypt_with_config(
    cipher: SymmetricCipher,
    encrypted_data: &[u8],
    key: &[u8],
) -> Result<Vec<u8>, NodeError> {
    if encrypted_data.len() < 12 {
        return Err(NodeError::EncryptedDataTooShort(encrypted_data.len()));
    }
    let (nonce_bytes, ciphertext) = encrypted_data.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);

    match cipher {
        SymmetricCipher::Aes256Gcm => {
            let c = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
            c.decrypt(nonce, ciphertext)
                .map_err(|e| NodeError::DecryptionError(e.to_string()))
        }
        SymmetricCipher::Aes128Gcm => {
            let c = Aes128Gcm::new(Key::<Aes128Gcm>::from_slice(key));
            c.decrypt(nonce, ciphertext)
                .map_err(|e| NodeError::DecryptionError(e.to_string()))
        }
        SymmetricCipher::ChaCha20Poly1305 => {
            let c = ChaCha20Poly1305::new(Key::<ChaCha20Poly1305>::from_slice(key));
            c.decrypt(nonce, ciphertext)
                .map_err(|e| NodeError::DecryptionError(e.to_string()))
        }
    }
}

// ── Fixed-key convenience wrappers (backward compat) ───────────────

/// Encrypt with AES-256-GCM and the given 32-byte key.
pub fn encrypt_payload_with_key(plaintext: &[u8], key: &[u8; 32]) -> Result<Vec<u8>, NodeError> {
    encrypt_with_config(SymmetricCipher::Aes256Gcm, plaintext, key)
}

/// Decrypt with AES-256-GCM and the given 32-byte key.
pub fn decrypt_payload_with_key(
    encrypted_data: &[u8],
    key: &[u8; 32],
) -> Result<Vec<u8>, NodeError> {
    decrypt_with_config(SymmetricCipher::Aes256Gcm, encrypted_data, key)
}

/// Encrypt plaintext using AES-256-GCM with the fixed key.
pub fn encrypt_payload(plaintext: &[u8]) -> Result<Vec<u8>, NodeError> {
    encrypt_payload_with_key(plaintext, FIXED_KEY)
}

/// Decrypt data that was encrypted with encrypt_payload (fixed key).
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
        let mut corrupted = encrypted;
        corrupted[15] ^= 0x01;
        assert!(decrypt_payload(&corrupted).is_err());
    }

    #[test]
    fn encrypt_large_payload_succeeds() {
        let large_plaintext = vec![0u8; 2000];
        let encrypted = encrypt_payload(&large_plaintext).expect("encryption should succeed");
        let decrypted = decrypt_payload(&encrypted).expect("decryption should succeed");
        assert_eq!(large_plaintext, decrypted);
    }

    #[test]
    fn encrypt_payload_adds_overhead() {
        let plaintext = vec![0u8; 100];
        let encrypted = encrypt_payload(&plaintext).expect("encryption should succeed");
        assert_eq!(encrypted.len(), plaintext.len() + 28);
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

    // ── Configurable cipher tests ──────────────────────────────────

    #[test]
    fn aes128gcm_roundtrip() {
        let key = [0xABu8; 16];
        let plaintext = b"aes-128-gcm test data";
        let encrypted =
            encrypt_with_config(SymmetricCipher::Aes128Gcm, plaintext, &key).expect("encrypt");
        let decrypted =
            decrypt_with_config(SymmetricCipher::Aes128Gcm, &encrypted, &key).expect("decrypt");
        assert_eq!(plaintext, decrypted.as_slice());
    }

    #[test]
    fn chacha20poly1305_roundtrip() {
        let key = [0xCDu8; 32];
        let plaintext = b"chacha20-poly1305 test data";
        let encrypted = encrypt_with_config(SymmetricCipher::ChaCha20Poly1305, plaintext, &key)
            .expect("encrypt");
        let decrypted = decrypt_with_config(SymmetricCipher::ChaCha20Poly1305, &encrypted, &key)
            .expect("decrypt");
        assert_eq!(plaintext, decrypted.as_slice());
    }

    #[test]
    fn cross_cipher_decrypt_fails() {
        let key = [0xFFu8; 32];
        let plaintext = b"cross-cipher";
        let encrypted =
            encrypt_with_config(SymmetricCipher::Aes256Gcm, plaintext, &key).expect("encrypt");
        assert!(decrypt_with_config(SymmetricCipher::ChaCha20Poly1305, &encrypted, &key).is_err());
    }

    #[test]
    fn all_ciphers_add_28_bytes_overhead() {
        let plaintext = vec![0u8; 50];
        for cipher in [
            SymmetricCipher::Aes256Gcm,
            SymmetricCipher::Aes128Gcm,
            SymmetricCipher::ChaCha20Poly1305,
        ] {
            let key = vec![0x42u8; cipher.key_len()];
            let encrypted = encrypt_with_config(cipher, &plaintext, &key).expect("encrypt");
            assert_eq!(
                encrypted.len(),
                plaintext.len() + 28,
                "{cipher} overhead mismatch"
            );
        }
    }

    #[test]
    fn configurable_kdf_all_variants_produce_valid_keys() {
        let shared = [0x77u8; 32];
        for kdf in [
            KdfAlgorithm::HkdfSha256,
            KdfAlgorithm::HkdfSha384,
            KdfAlgorithm::HkdfSha512,
        ] {
            let key = derive_key(kdf, &shared, 1, 32);
            assert_eq!(key.len(), 32, "{kdf} key length mismatch");
            // Non-zero output
            assert!(key.iter().any(|&b| b != 0), "{kdf} produced zero key");
        }
    }

    #[test]
    fn different_kdfs_produce_different_keys() {
        let shared = [0x55u8; 32];
        let k1 = derive_key(KdfAlgorithm::HkdfSha256, &shared, 1, 32);
        let k2 = derive_key(KdfAlgorithm::HkdfSha384, &shared, 1, 32);
        let k3 = derive_key(KdfAlgorithm::HkdfSha512, &shared, 1, 32);
        assert_ne!(k1, k2);
        assert_ne!(k2, k3);
    }

    #[test]
    fn derive_key_16_bytes_for_aes128() {
        let shared = [0x33u8; 32];
        let key = derive_key(KdfAlgorithm::HkdfSha256, &shared, 1, 16);
        assert_eq!(key.len(), 16);
    }

    #[test]
    fn full_configurable_dh_roundtrip() {
        let config = CryptoConfig {
            cipher: SymmetricCipher::ChaCha20Poly1305,
            kdf: KdfAlgorithm::HkdfSha512,
            dh_group: DhGroup::X25519,
        };

        let alice = DhKeypair::generate();
        let bob = DhKeypair::generate();
        let shared = alice.diffie_hellman(&bob.public);
        let key = derive_key(config.kdf, shared.as_bytes(), 42, config.cipher.key_len());

        let plaintext = b"full configurable roundtrip";
        let encrypted = encrypt_with_config(config.cipher, plaintext, &key).expect("encrypt");
        let decrypted = decrypt_with_config(config.cipher, &encrypted, &key).expect("decrypt");
        assert_eq!(plaintext, decrypted.as_slice());
    }

    #[test]
    fn symmetric_cipher_from_str() {
        assert_eq!(
            "aes-256-gcm".parse::<SymmetricCipher>().unwrap(),
            SymmetricCipher::Aes256Gcm
        );
        assert_eq!(
            "aes-128-gcm".parse::<SymmetricCipher>().unwrap(),
            SymmetricCipher::Aes128Gcm
        );
        assert_eq!(
            "chacha20-poly1305".parse::<SymmetricCipher>().unwrap(),
            SymmetricCipher::ChaCha20Poly1305
        );
        assert!("unknown".parse::<SymmetricCipher>().is_err());
    }

    #[test]
    fn kdf_from_str() {
        assert_eq!(
            "hkdf-sha256".parse::<KdfAlgorithm>().unwrap(),
            KdfAlgorithm::HkdfSha256
        );
        assert_eq!(
            "hkdf-sha512".parse::<KdfAlgorithm>().unwrap(),
            KdfAlgorithm::HkdfSha512
        );
        assert!("unknown".parse::<KdfAlgorithm>().is_err());
    }

    #[test]
    fn dh_group_from_str() {
        assert_eq!("x25519".parse::<DhGroup>().unwrap(), DhGroup::X25519);
        assert!("rsa".parse::<DhGroup>().is_err());
    }

    #[test]
    fn crypto_config_default() {
        let cfg = CryptoConfig::default();
        assert_eq!(cfg.cipher, SymmetricCipher::Aes256Gcm);
        assert_eq!(cfg.kdf, KdfAlgorithm::HkdfSha256);
        assert_eq!(cfg.dh_group, DhGroup::X25519);
    }
}
