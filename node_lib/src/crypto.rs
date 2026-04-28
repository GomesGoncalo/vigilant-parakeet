use crate::error::NodeError;
use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng},
    Aes128Gcm, Aes256Gcm, Key, Nonce,
};
use chacha20poly1305::ChaCha20Poly1305;
use ed25519_dalek::{Signature, Signer, Verifier, VerifyingKey};
use hkdf::Hkdf;
use ml_dsa::{KeyGen, KeyPair, MlDsa65};
use ml_kem::{Decapsulate, Encapsulate, EncapsulationKey, FromSeed, Kem, KeyExport, MlKem768};
use sha2::{Sha256, Sha384, Sha512};
use x25519_dalek::{EphemeralSecret, PublicKey, SharedSecret, StaticSecret};
use zeroize::Zeroizing;

/// HKDF info string for deriving keys from DH shared secrets.
const HKDF_INFO: &[u8] = b"vigilant-parakeet-dh";

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

/// DH group / KEM algorithm for key exchange.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum DhGroup {
    /// X25519 (Curve25519, default) — classical ECDH, 32-byte keys.
    #[default]
    X25519,
    /// ML-KEM-768 (NIST FIPS 203) — quantum-resistant KEM.
    /// Encapsulation key: 1184 bytes; ciphertext: 1088 bytes; shared secret: 32 bytes.
    MlKem768,
}

impl DhGroup {
    /// Returns the wire-format byte identifier for this DH group.
    pub fn wire_id(self) -> u8 {
        match self {
            Self::X25519 => 0x01,
            Self::MlKem768 => 0x02,
        }
    }

    /// Maps a wire-format byte back to a [`DhGroup`].
    ///
    /// Returns `None` for unrecognised IDs.
    pub fn from_wire_id(id: u8) -> Option<Self> {
        match id {
            0x01 => Some(Self::X25519),
            0x02 => Some(Self::MlKem768),
            _ => None,
        }
    }
}

impl std::fmt::Display for DhGroup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::X25519 => write!(f, "x25519"),
            Self::MlKem768 => write!(f, "ml-kem-768"),
        }
    }
}

impl std::str::FromStr for DhGroup {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "x25519" | "curve25519" => Ok(Self::X25519),
            "ml-kem-768" | "mlkem768" | "kyber768" => Ok(Self::MlKem768),
            _ => Err(format!(
                "unknown DH group '{}', expected: x25519, ml-kem-768",
                s
            )),
        }
    }
}

/// Signing algorithm for DH key exchange authentication.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum SigningAlgorithm {
    /// Ed25519 (default) — classical EdDSA, 32-byte verifying key, 64-byte signature.
    #[default]
    Ed25519,
    /// ML-DSA-65 (NIST FIPS 204) — quantum-resistant digital signature.
    /// Verifying key: 1952 bytes; signature: 3309 bytes.
    MlDsa65,
}

impl SigningAlgorithm {
    /// Returns the wire-format byte identifier for this signing algorithm.
    pub fn wire_id(self) -> u8 {
        match self {
            Self::Ed25519 => 0x01,
            Self::MlDsa65 => 0x02,
        }
    }

    /// Maps a wire-format byte back to a [`SigningAlgorithm`].
    ///
    /// Returns `None` for unrecognised IDs — callers should drop the message.
    pub fn from_wire_id(id: u8) -> Option<Self> {
        match id {
            0x01 => Some(Self::Ed25519),
            0x02 => Some(Self::MlDsa65),
            _ => None,
        }
    }
}

impl std::fmt::Display for SigningAlgorithm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ed25519 => write!(f, "ed25519"),
            Self::MlDsa65 => write!(f, "ml-dsa-65"),
        }
    }
}

impl std::str::FromStr for SigningAlgorithm {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "ed25519" => Ok(Self::Ed25519),
            "ml-dsa-65" | "mldsa65" | "dilithium3" => Ok(Self::MlDsa65),
            _ => Err(format!(
                "unknown signing algorithm '{}', expected: ed25519, ml-dsa-65",
                s
            )),
        }
    }
}

/// Complete crypto configuration for a node.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CryptoConfig {
    pub cipher: SymmetricCipher,
    pub kdf: KdfAlgorithm,
    pub dh_group: DhGroup,
    /// Signing algorithm used when `enable_dh_signatures` is set.
    pub signing_algorithm: SigningAlgorithm,
}

// ── ML-KEM-768 constants ───────────────────────────────────────────────

/// Encapsulation key size (public key sent in KeyExchangeInit).
pub const ML_KEM_768_EK_LEN: usize = 1184;
/// Ciphertext size (returned in KeyExchangeReply).
pub const ML_KEM_768_CT_LEN: usize = 1088;
/// Decapsulation key seed size (stored locally, never transmitted).
pub const ML_KEM_768_SEED_LEN: usize = 64;
/// Shared secret size (32 bytes, same as X25519).
pub const ML_KEM_768_SS_LEN: usize = 32;

// ── Ed25519 constants ──────────────────────────────────────────────────

/// Ed25519 verifying key size.
pub const ED25519_VK_LEN: usize = 32;
/// Ed25519 signature size.
pub const ED25519_SIG_LEN: usize = 64;

// ── ML-DSA-65 constants ────────────────────────────────────────────────

/// ML-DSA-65 verifying key size (sent in signed messages).
pub const ML_DSA_65_VK_LEN: usize = 1952;
/// ML-DSA-65 signature size.
pub const ML_DSA_65_SIG_LEN: usize = 3309;

// ── ML-KEM-768 key encapsulation mechanism ─────────────────────────────

/// Generate an ML-KEM-768 keypair.
///
/// Returns `(dk_seed, ek_bytes)`:
/// - `dk_seed`: 64-byte seed that reconstructs the decapsulation (private) key.
/// - `ek_bytes`: 1184-byte encapsulation (public) key to send in `KeyExchangeInit`.
pub fn kem_768_generate() -> ([u8; ML_KEM_768_SEED_LEN], [u8; ML_KEM_768_EK_LEN]) {
    let (dk, ek) = MlKem768::generate_keypair();
    let dk_seed: [u8; ML_KEM_768_SEED_LEN] = dk.to_bytes().into();
    let ek_bytes: [u8; ML_KEM_768_EK_LEN] = ek.to_bytes().into();
    (dk_seed, ek_bytes)
}

/// Encapsulate a shared secret using the peer's ML-KEM-768 encapsulation key.
///
/// Returns `(ct_bytes, ss_bytes)`:
/// - `ct_bytes`: 1088-byte ciphertext to send in `KeyExchangeReply`.
/// - `ss_bytes`: 32-byte shared secret — feed into HKDF to derive the symmetric key.
pub fn kem_768_encapsulate(
    ek_bytes: &[u8; ML_KEM_768_EK_LEN],
) -> Result<([u8; ML_KEM_768_CT_LEN], [u8; ML_KEM_768_SS_LEN]), NodeError> {
    let ek_key_arr: ml_kem::Key<EncapsulationKey<MlKem768>> = (*ek_bytes).into();
    let ek = EncapsulationKey::<MlKem768>::new(&ek_key_arr)
        .map_err(|e| NodeError::EncryptionError(format!("invalid ML-KEM-768 encap key: {e}")))?;
    let (ct, ss) = ek.encapsulate();
    let ct_bytes: [u8; ML_KEM_768_CT_LEN] = ct.into();
    let ss_bytes: [u8; ML_KEM_768_SS_LEN] = ss.into();
    Ok((ct_bytes, ss_bytes))
}

/// Decapsulate the shared secret from an ML-KEM-768 ciphertext.
///
/// `dk_seed` is the 64-byte seed stored in `PendingExchange`.
/// Returns the 32-byte shared secret — feed into HKDF to derive the symmetric key.
pub fn kem_768_decapsulate(
    dk_seed: &[u8; ML_KEM_768_SEED_LEN],
    ct_bytes: &[u8; ML_KEM_768_CT_LEN],
) -> Result<[u8; ML_KEM_768_SS_LEN], NodeError> {
    let seed_array: ml_kem::Seed = (*dk_seed).into();
    let (dk, _) = MlKem768::from_seed(&seed_array);
    let ct_array: ml_kem::Ciphertext<MlKem768> = (*ct_bytes).into();
    let ss = dk.decapsulate(&ct_array);
    let ss_bytes: [u8; ML_KEM_768_SS_LEN] = ss.into();
    Ok(ss_bytes)
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

// ── Signing keypair (Ed25519 or ML-DSA-65) ────────────────────────────

/// Signing keypair for authenticating DH key exchange messages.
///
/// Supports both classical Ed25519 and quantum-resistant ML-DSA-65.
/// The algorithm is chosen at construction time via `SigningAlgorithm`.
/// Both algorithms derive the keypair deterministically from a 32-byte seed,
/// enabling persistent identities via configuration.
///
/// The seed field is automatically zeroed when the keypair is dropped.
pub struct SigningKeypair {
    /// Raw 32-byte seed — zeroized on drop to prevent key material lingering in memory.
    seed: Zeroizing<[u8; 32]>,
    inner: SigningKeypairInner,
}

enum SigningKeypairInner {
    Ed25519(Box<ed25519_dalek::SigningKey>),
    MlDsa65(Box<KeyPair<MlDsa65>>),
}

impl SigningKeypair {
    /// Generate a new random signing keypair using the given algorithm.
    pub fn generate(algo: SigningAlgorithm) -> Self {
        let seed = ed25519_dalek::SigningKey::generate(&mut OsRng).to_bytes();
        Self::from_seed(algo, seed)
    }

    /// Reconstruct a `SigningKeypair` from a 32-byte seed and algorithm.
    /// The same `(algo, seed)` pair always produces the same keypair.
    pub fn from_seed(algo: SigningAlgorithm, seed: [u8; 32]) -> Self {
        let inner = match algo {
            SigningAlgorithm::Ed25519 => {
                SigningKeypairInner::Ed25519(Box::new(ed25519_dalek::SigningKey::from_bytes(&seed)))
            }
            SigningAlgorithm::MlDsa65 => {
                SigningKeypairInner::MlDsa65(Box::new(MlDsa65::key_gen_internal((&seed).into())))
            }
        };
        Self {
            seed: Zeroizing::new(seed),
            inner,
        }
    }

    /// Return the 32-byte seed for reconstructing this keypair via `from_seed`.
    pub fn seed_bytes(&self) -> [u8; 32] {
        *self.seed
    }

    /// Return the signing algorithm used by this keypair.
    pub fn signing_algorithm(&self) -> SigningAlgorithm {
        match &self.inner {
            SigningKeypairInner::Ed25519(_) => SigningAlgorithm::Ed25519,
            SigningKeypairInner::MlDsa65(_) => SigningAlgorithm::MlDsa65,
        }
    }

    /// Sign `message` and return the signature bytes.
    ///
    /// - Ed25519: 64 bytes.
    /// - ML-DSA-65: 3309 bytes.
    pub fn sign(&self, message: &[u8]) -> Vec<u8> {
        match &self.inner {
            SigningKeypairInner::Ed25519(sk) => sk.sign(message).to_bytes().to_vec(),
            SigningKeypairInner::MlDsa65(sk) => {
                use ml_dsa::signature::Signer;
                sk.sign(message).encode().to_vec()
            }
        }
    }

    /// Return the verifying (public) key bytes.
    ///
    /// - Ed25519: 32 bytes.
    /// - ML-DSA-65: 1952 bytes.
    pub fn verifying_key_bytes(&self) -> Vec<u8> {
        match &self.inner {
            SigningKeypairInner::Ed25519(sk) => sk.verifying_key().to_bytes().to_vec(),
            SigningKeypairInner::MlDsa65(sk) => sk.verifying_key().encode().to_vec(),
        }
    }
}

/// Decode a 64-hex-character string into a 32-byte array.
///
/// Returns `None` if the input is not exactly 64 hex characters.
pub fn decode_hex_32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..32)
        .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok())
        .collect();
    bytes.and_then(|b| b.try_into().ok())
}

/// Verify a DH key exchange signature.
///
/// `algo` selects the algorithm that produced the signature.
/// `message` is the base KE payload bytes that were signed.
/// `signing_pubkey` — 32 bytes for Ed25519, 1952 bytes for ML-DSA-65.
/// `signature` — 64 bytes for Ed25519, 3309 bytes for ML-DSA-65.
pub fn verify_dh_signature(
    algo: SigningAlgorithm,
    message: &[u8],
    signing_pubkey: &[u8],
    signature: &[u8],
) -> Result<(), NodeError> {
    match algo {
        SigningAlgorithm::Ed25519 => {
            let pk_bytes: &[u8; 32] = signing_pubkey.try_into().map_err(|_| {
                NodeError::SignatureError(format!(
                    "Ed25519 verifying key must be 32 bytes, got {}",
                    signing_pubkey.len()
                ))
            })?;
            let sig_bytes: &[u8; 64] = signature.try_into().map_err(|_| {
                NodeError::SignatureError(format!(
                    "Ed25519 signature must be 64 bytes, got {}",
                    signature.len()
                ))
            })?;
            let verifying_key = VerifyingKey::from_bytes(pk_bytes).map_err(|e| {
                NodeError::SignatureError(format!("invalid Ed25519 verifying key: {e}"))
            })?;
            let sig = Signature::from_bytes(sig_bytes);
            verifying_key
                .verify(message, &sig)
                .map_err(|e| NodeError::SignatureError(format!("Ed25519 verification failed: {e}")))
        }
        SigningAlgorithm::MlDsa65 => {
            let enc_vk: [u8; ML_DSA_65_VK_LEN] = signing_pubkey.try_into().map_err(|_| {
                NodeError::SignatureError(format!(
                    "ML-DSA-65 verifying key must be {ML_DSA_65_VK_LEN} bytes, got {}",
                    signing_pubkey.len()
                ))
            })?;
            let enc_sig_arr: [u8; ML_DSA_65_SIG_LEN] = signature.try_into().map_err(|_| {
                NodeError::SignatureError(format!(
                    "ML-DSA-65 signature must be {ML_DSA_65_SIG_LEN} bytes, got {}",
                    signature.len()
                ))
            })?;
            let enc_vk_array: ml_dsa::EncodedVerifyingKey<MlDsa65> = enc_vk.into();
            let vk = ml_dsa::VerifyingKey::<MlDsa65>::decode(&enc_vk_array);
            let enc_sig: ml_dsa::EncodedSignature<MlDsa65> = enc_sig_arr.into();
            let sig = ml_dsa::Signature::<MlDsa65>::decode(&enc_sig).ok_or_else(|| {
                NodeError::SignatureError("ML-DSA-65 signature decoding failed".into())
            })?;
            use ml_dsa::signature::Verifier;
            vk.verify(message, &sig).map_err(|e| {
                NodeError::SignatureError(format!("ML-DSA-65 verification failed: {e}"))
            })
        }
    }
}

// ── Key derivation ─────────────────────────────────────────────────

/// Derive a symmetric key from a DH shared secret using the configured KDF.
///
/// The `key_id` is mixed into the HKDF salt to bind the derived key to a
/// specific exchange round, ensuring each re-key produces a distinct key.
/// `key_len` determines the output size (16 for AES-128, 32 for AES-256/ChaCha20).
pub fn derive_key(
    kdf: KdfAlgorithm,
    shared_secret: &[u8],
    key_id: u32,
    key_len: usize,
) -> Result<Vec<u8>, NodeError> {
    let mut salt = Vec::with_capacity(36);
    salt.extend_from_slice(b"vigilant-parakeet-salt-");
    salt.extend_from_slice(&key_id.to_be_bytes());

    let mut okm = vec![0u8; key_len];

    let expand_result = match kdf {
        KdfAlgorithm::HkdfSha256 => {
            let hk = Hkdf::<Sha256>::new(Some(&salt), shared_secret);
            hk.expand(HKDF_INFO, &mut okm)
        }
        KdfAlgorithm::HkdfSha384 => {
            let hk = Hkdf::<Sha384>::new(Some(&salt), shared_secret);
            hk.expand(HKDF_INFO, &mut okm)
        }
        KdfAlgorithm::HkdfSha512 => {
            let hk = Hkdf::<Sha512>::new(Some(&salt), shared_secret);
            hk.expand(HKDF_INFO, &mut okm)
        }
    };

    expand_result.map_err(|e| {
        NodeError::EncryptionError(format!("HKDF expand failed for {:?}: {}", kdf, e))
    })?;

    Ok(okm)
}

/// Convenience wrapper: derive a 32-byte key with HKDF-SHA256.
pub fn derive_key_from_shared_secret(
    shared_secret: &[u8; 32],
    key_id: u32,
) -> Result<[u8; 32], NodeError> {
    let v = derive_key(KdfAlgorithm::HkdfSha256, shared_secret, key_id, 32)?;
    let mut out = [0u8; 32];
    out.copy_from_slice(&v);
    Ok(out)
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
    let expected = cipher.key_len();
    if key.len() != expected {
        return Err(NodeError::EncryptionError(format!(
            "key length mismatch: expected {} bytes for {:?}, got {}",
            expected,
            cipher,
            key.len()
        )));
    }
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
    let expected = cipher.key_len();
    if key.len() != expected {
        return Err(NodeError::DecryptionError(format!(
            "key length mismatch: expected {} bytes for {:?}, got {}",
            expected,
            cipher,
            key.len()
        )));
    }
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

// ── Key-based convenience wrappers ───────────────

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let key: [u8; 32] = [0x42; 32];
        let plaintext = b"test payload data";
        let encrypted = encrypt_payload_with_key(plaintext, &key).expect("encryption failed");
        let decrypted = decrypt_payload_with_key(&encrypted, &key).expect("decryption failed");
        assert_eq!(plaintext, decrypted.as_slice());
    }

    #[test]
    fn encrypt_produces_different_ciphertext() {
        let key: [u8; 32] = [0x42; 32];
        let plaintext = b"test payload data";
        let encrypted1 = encrypt_payload_with_key(plaintext, &key).expect("encryption failed");
        let encrypted2 = encrypt_payload_with_key(plaintext, &key).expect("encryption failed");
        assert_ne!(encrypted1, encrypted2);
    }

    #[test]
    fn decrypt_invalid_data_fails() {
        let key: [u8; 32] = [0x42; 32];
        let invalid_data = b"too short";
        assert!(decrypt_payload_with_key(invalid_data, &key).is_err());
    }

    #[test]
    fn decrypt_wrong_key_fails() {
        let key: [u8; 32] = [0x42; 32];
        let plaintext = b"test data";
        let encrypted = encrypt_payload_with_key(plaintext, &key).expect("encryption failed");
        let mut corrupted = encrypted;
        corrupted[15] ^= 0x01;
        assert!(decrypt_payload_with_key(&corrupted, &key).is_err());
    }

    #[test]
    fn encrypt_large_payload_succeeds() {
        let key: [u8; 32] = [0x42; 32];
        let large_plaintext = vec![0u8; 2000];
        let encrypted =
            encrypt_payload_with_key(&large_plaintext, &key).expect("encryption should succeed");
        let decrypted =
            decrypt_payload_with_key(&encrypted, &key).expect("decryption should succeed");
        assert_eq!(large_plaintext, decrypted);
    }

    #[test]
    fn encrypt_payload_adds_overhead() {
        let key: [u8; 32] = [0x42; 32];
        let plaintext = vec![0u8; 100];
        let encrypted =
            encrypt_payload_with_key(&plaintext, &key).expect("encryption should succeed");
        assert_eq!(encrypted.len(), plaintext.len() + 28);
        let decrypted =
            decrypt_payload_with_key(&encrypted, &key).expect("decryption should succeed");
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
        let key = derive_key_from_shared_secret(shared.as_bytes(), 1).expect("derive key");

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
        let key1 = derive_key_from_shared_secret(&shared_secret, 1).expect("derive key");
        let key2 = derive_key_from_shared_secret(&shared_secret, 2).expect("derive key");
        assert_ne!(key1, key2);
    }

    #[test]
    fn different_keys_cannot_decrypt_each_other() {
        let key_a: [u8; 32] = [0xAA; 32];
        let key_b: [u8; 32] = [0xBB; 32];
        let plaintext = b"test data";
        let encrypted_a = encrypt_payload_with_key(plaintext, &key_a).expect("encrypt with key_a");
        assert!(decrypt_payload_with_key(&encrypted_a, &key_b).is_err());
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
            let key = derive_key(kdf, &shared, 1, 32).expect("derive key");
            assert_eq!(key.len(), 32, "{kdf} key length mismatch");
            // Non-zero output
            assert!(key.iter().any(|&b| b != 0), "{kdf} produced zero key");
        }
    }

    #[test]
    fn different_kdfs_produce_different_keys() {
        let shared = [0x55u8; 32];
        let k1 = derive_key(KdfAlgorithm::HkdfSha256, &shared, 1, 32).unwrap();
        let k2 = derive_key(KdfAlgorithm::HkdfSha384, &shared, 1, 32).unwrap();
        let k3 = derive_key(KdfAlgorithm::HkdfSha512, &shared, 1, 32).unwrap();
        assert_ne!(k1, k2);
        assert_ne!(k2, k3);
    }

    #[test]
    fn derive_key_16_bytes_for_aes128() {
        let shared = [0x33u8; 32];
        let key = derive_key(KdfAlgorithm::HkdfSha256, &shared, 1, 16).unwrap();
        assert_eq!(key.len(), 16);
    }

    #[test]
    fn full_configurable_dh_roundtrip() {
        let config = CryptoConfig {
            cipher: SymmetricCipher::ChaCha20Poly1305,
            kdf: KdfAlgorithm::HkdfSha512,
            dh_group: DhGroup::X25519,
            signing_algorithm: SigningAlgorithm::Ed25519,
        };

        let alice = DhKeypair::generate();
        let bob = DhKeypair::generate();
        let shared = alice.diffie_hellman(&bob.public);
        let key = derive_key(config.kdf, shared.as_bytes(), 42, config.cipher.key_len()).unwrap();

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
        assert_eq!("ml-kem-768".parse::<DhGroup>().unwrap(), DhGroup::MlKem768);
        assert!("rsa".parse::<DhGroup>().is_err());
    }

    #[test]
    fn crypto_config_default() {
        let cfg = CryptoConfig::default();
        assert_eq!(cfg.cipher, SymmetricCipher::Aes256Gcm);
        assert_eq!(cfg.kdf, KdfAlgorithm::HkdfSha256);
        assert_eq!(cfg.dh_group, DhGroup::X25519);
        assert_eq!(cfg.signing_algorithm, SigningAlgorithm::Ed25519);
    }

    // ── Ed25519 signing tests ────────────────────────────────────────

    #[test]
    fn signing_keypair_generates_distinct_keys() {
        let kp1 = SigningKeypair::generate(SigningAlgorithm::Ed25519);
        let kp2 = SigningKeypair::generate(SigningAlgorithm::Ed25519);
        assert_ne!(kp1.verifying_key_bytes(), kp2.verifying_key_bytes());
    }

    #[test]
    fn sign_and_verify_roundtrip() {
        let kp = SigningKeypair::generate(SigningAlgorithm::Ed25519);
        let message = b"key_id + dh_public + sender";
        let sig = kp.sign(message);
        let pubkey = kp.verifying_key_bytes();
        assert!(verify_dh_signature(SigningAlgorithm::Ed25519, message, &pubkey, &sig).is_ok());
    }

    #[test]
    fn verify_wrong_key_fails() {
        let signer = SigningKeypair::generate(SigningAlgorithm::Ed25519);
        let other = SigningKeypair::generate(SigningAlgorithm::Ed25519);
        let message = b"some dh payload";
        let sig = signer.sign(message);
        let wrong_pubkey = other.verifying_key_bytes();
        assert!(
            verify_dh_signature(SigningAlgorithm::Ed25519, message, &wrong_pubkey, &sig).is_err()
        );
    }

    #[test]
    fn verify_tampered_message_fails() {
        let kp = SigningKeypair::generate(SigningAlgorithm::Ed25519);
        let message = b"original dh payload";
        let sig = kp.sign(message);
        let pubkey = kp.verifying_key_bytes();
        let tampered = b"tampered dh payload";
        assert!(verify_dh_signature(SigningAlgorithm::Ed25519, tampered, &pubkey, &sig).is_err());
    }

    #[test]
    fn verify_tampered_signature_fails() {
        let kp = SigningKeypair::generate(SigningAlgorithm::Ed25519);
        let message = b"dh payload bytes";
        let mut sig = kp.sign(message);
        sig[0] ^= 0xFF;
        let pubkey = kp.verifying_key_bytes();
        assert!(verify_dh_signature(SigningAlgorithm::Ed25519, message, &pubkey, &sig).is_err());
    }

    #[test]
    fn ed25519_seed_roundtrip() {
        let kp = SigningKeypair::generate(SigningAlgorithm::Ed25519);
        let seed = kp.seed_bytes();
        let kp2 = SigningKeypair::from_seed(SigningAlgorithm::Ed25519, seed);
        assert_eq!(kp.verifying_key_bytes(), kp2.verifying_key_bytes());
    }

    // ── ML-DSA-65 signing tests ──────────────────────────────────────

    #[test]
    fn ml_dsa_65_sign_verify_roundtrip() {
        let kp = SigningKeypair::generate(SigningAlgorithm::MlDsa65);
        assert_eq!(kp.verifying_key_bytes().len(), ML_DSA_65_VK_LEN);
        let message = b"quantum-resistant key exchange payload";
        let sig = kp.sign(message);
        assert_eq!(sig.len(), ML_DSA_65_SIG_LEN);
        let pubkey = kp.verifying_key_bytes();
        assert!(verify_dh_signature(SigningAlgorithm::MlDsa65, message, &pubkey, &sig).is_ok());
    }

    #[test]
    fn ml_dsa_65_wrong_key_fails() {
        let signer = SigningKeypair::generate(SigningAlgorithm::MlDsa65);
        let other = SigningKeypair::generate(SigningAlgorithm::MlDsa65);
        let message = b"some payload";
        let sig = signer.sign(message);
        let wrong_pubkey = other.verifying_key_bytes();
        assert!(
            verify_dh_signature(SigningAlgorithm::MlDsa65, message, &wrong_pubkey, &sig).is_err()
        );
    }

    #[test]
    fn ml_dsa_65_seed_roundtrip() {
        let kp = SigningKeypair::generate(SigningAlgorithm::MlDsa65);
        let seed = kp.seed_bytes();
        let kp2 = SigningKeypair::from_seed(SigningAlgorithm::MlDsa65, seed);
        assert_eq!(kp.verifying_key_bytes(), kp2.verifying_key_bytes());
    }

    // ── ML-KEM-768 tests ─────────────────────────────────────────────

    #[test]
    fn ml_kem_768_roundtrip() {
        let (dk_seed, ek_bytes) = kem_768_generate();
        assert_eq!(ek_bytes.len(), ML_KEM_768_EK_LEN);
        assert_eq!(dk_seed.len(), ML_KEM_768_SEED_LEN);

        let (ct_bytes, ss_enc) = kem_768_encapsulate(&ek_bytes).expect("encapsulate");
        assert_eq!(ct_bytes.len(), ML_KEM_768_CT_LEN);
        assert_eq!(ss_enc.len(), ML_KEM_768_SS_LEN);

        let ss_dec = kem_768_decapsulate(&dk_seed, &ct_bytes).expect("decapsulate");
        assert_eq!(ss_enc, ss_dec, "shared secrets must match");
    }

    #[test]
    fn ml_kem_768_different_pairs_dont_share_secret() {
        let (dk_seed1, ek_bytes1) = kem_768_generate();
        let (dk_seed2, _ek_bytes2) = kem_768_generate();
        assert_ne!(dk_seed1, dk_seed2);

        // Encapsulate with key1
        let (ct, ss_correct) = kem_768_encapsulate(&ek_bytes1).expect("encapsulate");
        // Correct decapsulation: dk1 decapsulates ciphertext addressed to ek1
        let ss_dk1 = kem_768_decapsulate(&dk_seed1, &ct).expect("decapsulate dk1");
        assert_eq!(
            ss_correct, ss_dk1,
            "correct keypair must recover the shared secret"
        );
        // Wrong decapsulation: dk2 decapsulates the same ciphertext — must produce a different secret
        let ss_dk2 = kem_768_decapsulate(&dk_seed2, &ct).expect("decapsulate dk2");
        assert_ne!(
            ss_dk1, ss_dk2,
            "wrong decapsulation key must produce a different secret"
        );
    }

    #[test]
    fn ml_kem_768_derive_key_roundtrip() {
        let (dk_seed, ek_bytes) = kem_768_generate();
        let (ct_bytes, ss_enc) = kem_768_encapsulate(&ek_bytes).expect("encapsulate");
        let ss_dec = kem_768_decapsulate(&dk_seed, &ct_bytes).expect("decapsulate");

        let key_enc = derive_key(KdfAlgorithm::HkdfSha256, &ss_enc, 1, 32).expect("derive enc");
        let key_dec = derive_key(KdfAlgorithm::HkdfSha256, &ss_dec, 1, 32).expect("derive dec");
        assert_eq!(key_enc, key_dec, "derived keys must match");
    }
}
