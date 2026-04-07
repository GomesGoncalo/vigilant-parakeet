use mac_address::MacAddress;
use node_lib::crypto::{
    CryptoConfig, DhGroup, ML_KEM_768_CT_LEN, ML_KEM_768_EK_LEN, ML_KEM_768_SEED_LEN,
};
use rand_core::OsRng;
use std::collections::HashMap;
use tokio::time::Instant;
use x25519_dalek::{EphemeralSecret, PublicKey};

/// Private: stores the OBU's side of a pending key exchange.
enum PendingKeyMaterial {
    /// X25519 ephemeral keypair. `EphemeralSecret` enforces single-use at the
    /// type level — `diffie_hellman` consumes it, preventing accidental reuse.
    X25519(EphemeralSecret, PublicKey),
    /// ML-KEM-768 decapsulation key seed (64 bytes). The encapsulation key
    /// (1184 bytes) is sent in the wire message; we only keep the seed to
    /// reconstruct the decap key when the server's ciphertext arrives.
    /// Wrapped in `Zeroizing` so the seed bytes are wiped when the pending
    /// exchange is removed.
    MlKem768Seed(zeroize::Zeroizing<[u8; ML_KEM_768_SEED_LEN]>),
}

/// State of a pending key exchange initiated by this node.
pub struct PendingExchange {
    key_material: PendingKeyMaterial,
    pub key_id: u32,
    pub initiated_at: Instant,
    pub retries: u32,
}

/// An established DH-derived key for a peer.
pub struct EstablishedKey {
    pub key: Vec<u8>,
    pub key_id: u32,
    pub established_at: Instant,
}

/// Per-peer DH key store managing pending exchanges and established keys.
pub struct DhKeyStore {
    /// Pending outgoing key exchanges, keyed by peer MAC.
    pending: HashMap<[u8; 6], PendingExchange>,
    /// Established DH-derived keys, keyed by peer MAC.
    established: HashMap<[u8; 6], EstablishedKey>,
    /// Counter for generating unique key IDs.
    next_key_id: u32,
    /// Crypto configuration for key derivation.
    crypto_config: CryptoConfig,
}

impl Default for DhKeyStore {
    fn default() -> Self {
        Self::new(CryptoConfig::default())
    }
}

impl DhKeyStore {
    pub fn new(crypto_config: CryptoConfig) -> Self {
        Self {
            pending: HashMap::new(),
            established: HashMap::new(),
            next_key_id: 1,
            crypto_config,
        }
    }

    /// Start a new key exchange with a peer.
    ///
    /// Returns the key_id and the public key material to send in the wire message:
    /// - X25519: 32-byte ephemeral public key
    /// - ML-KEM-768: 1184-byte encapsulation key
    pub fn initiate_exchange(&mut self, peer: MacAddress) -> (u32, Vec<u8>) {
        let key_id = self.next_key_id;
        self.next_key_id = self.next_key_id.wrapping_add(1);

        let (key_material, public_bytes) = Self::generate_key_material(self.crypto_config.dh_group);

        self.pending.insert(
            peer.bytes(),
            PendingExchange {
                key_material,
                key_id,
                initiated_at: Instant::now(),
                retries: 0,
            },
        );

        (key_id, public_bytes)
    }

    /// Re-initiate a timed-out key exchange, preserving the retry count.
    pub fn reinitiate_exchange(&mut self, peer: MacAddress) -> (u32, Vec<u8>) {
        let prev_retries = self
            .pending
            .get(&peer.bytes())
            .map(|p| p.retries)
            .unwrap_or(0);

        let key_id = self.next_key_id;
        self.next_key_id = self.next_key_id.wrapping_add(1);

        let (key_material, public_bytes) = Self::generate_key_material(self.crypto_config.dh_group);

        self.pending.insert(
            peer.bytes(),
            PendingExchange {
                key_material,
                key_id,
                initiated_at: Instant::now(),
                retries: prev_retries + 1,
            },
        );

        (key_id, public_bytes)
    }

    /// Generate fresh key material for an outgoing key exchange.
    fn generate_key_material(dh_group: DhGroup) -> (PendingKeyMaterial, Vec<u8>) {
        match dh_group {
            DhGroup::X25519 => {
                let secret = EphemeralSecret::random_from_rng(OsRng);
                let public = PublicKey::from(&secret);
                let pub_bytes = public.as_bytes().to_vec();
                (PendingKeyMaterial::X25519(secret, public), pub_bytes)
            }
            DhGroup::MlKem768 => {
                let (seed, ek) = node_lib::crypto::kem_768_generate();
                (
                    PendingKeyMaterial::MlKem768Seed(zeroize::Zeroizing::new(seed)),
                    ek.to_vec(),
                )
            }
        }
    }

    /// Complete a key exchange when we receive a reply.
    ///
    /// `peer_response` is algorithm-dependent:
    /// - X25519: 32-byte DH public key from the responder
    /// - ML-KEM-768: 1088-byte KEM ciphertext from the responder
    ///
    /// Returns the derived key and session establishment duration on success.
    pub fn complete_exchange(
        &mut self,
        peer: MacAddress,
        key_id: u32,
        peer_response: &[u8],
    ) -> Option<(Vec<u8>, std::time::Duration)> {
        // Check key_id before removing so we don't consume the pending exchange on mismatch.
        {
            let pending = self.pending.get(&peer.bytes())?;
            if pending.key_id != key_id {
                tracing::warn!(
                    expected = pending.key_id,
                    received = key_id,
                    peer = %peer,
                    "Key exchange key_id mismatch, ignoring reply"
                );
                return None;
            }
        }

        // Remove (and take ownership of) the pending exchange. For X25519,
        // EphemeralSecret::diffie_hellman consumes the secret, enforcing single-use.
        let pending = self.pending.remove(&peer.bytes())?;

        let session_duration = pending.initiated_at.elapsed();
        let retries = pending.retries;

        let shared_secret_bytes: Vec<u8> = match pending.key_material {
            PendingKeyMaterial::X25519(secret, _public) => {
                let peer_pub_bytes: [u8; 32] = peer_response.try_into().ok()?;
                let peer_public = PublicKey::from(peer_pub_bytes);
                secret.diffie_hellman(&peer_public).as_bytes().to_vec()
            }
            PendingKeyMaterial::MlKem768Seed(seed) => {
                let ct_bytes: &[u8; ML_KEM_768_CT_LEN] = peer_response.try_into().ok()?;
                match node_lib::crypto::kem_768_decapsulate(&seed, ct_bytes) {
                    Ok(ss) => ss.to_vec(),
                    Err(e) => {
                        tracing::error!(error = %e, "ML-KEM-768 decapsulation failed");
                        return None;
                    }
                }
            }
        };

        let derived_key = match node_lib::crypto::derive_key(
            self.crypto_config.kdf,
            &shared_secret_bytes,
            key_id,
            self.crypto_config.cipher.key_len(),
        ) {
            Ok(k) => k,
            Err(e) => {
                tracing::error!(error = %e, "Failed to derive session key");
                return None;
            }
        };

        tracing::info!(
            peer = %peer,
            key_id = key_id,
            retries = retries,
            elapsed_ms = session_duration.as_millis() as u64,
            dh_group = %self.crypto_config.dh_group,
            "Key exchange session established"
        );
        self.established.insert(
            peer.bytes(),
            EstablishedKey {
                key: derived_key.clone(),
                key_id,
                established_at: Instant::now(),
            },
        );

        Some((derived_key, session_duration))
    }

    /// Handle an incoming key exchange init from a peer (OBU-to-OBU or test usage).
    ///
    /// `peer_key_material` is algorithm-dependent:
    /// - X25519: 32-byte DH public key
    /// - ML-KEM-768: 1184-byte encapsulation key
    ///
    /// Returns our response to send back:
    /// - X25519: 32-byte DH public key
    /// - ML-KEM-768: 1088-byte KEM ciphertext
    ///
    /// Returns `None` if key derivation fails.
    pub fn handle_incoming_init(
        &mut self,
        peer: MacAddress,
        key_id: u32,
        peer_key_material: &[u8],
    ) -> Option<Vec<u8>> {
        let (our_response, shared_secret_bytes) = match self.crypto_config.dh_group {
            DhGroup::X25519 => {
                let our_secret = EphemeralSecret::random_from_rng(OsRng);
                let our_public = PublicKey::from(&our_secret);
                let our_public_bytes = our_public.as_bytes().to_vec();
                let peer_pub_bytes: [u8; 32] = peer_key_material.try_into().ok()?;
                let peer_public = PublicKey::from(peer_pub_bytes);
                let ss = our_secret.diffie_hellman(&peer_public).as_bytes().to_vec();
                (our_public_bytes, ss)
            }
            DhGroup::MlKem768 => {
                let ek_bytes: &[u8; ML_KEM_768_EK_LEN] = peer_key_material.try_into().ok()?;
                match node_lib::crypto::kem_768_encapsulate(ek_bytes) {
                    Ok((ct, ss)) => (ct.to_vec(), ss.to_vec()),
                    Err(e) => {
                        tracing::error!(
                            error = %e,
                            "ML-KEM-768 encapsulation failed in handle_incoming_init"
                        );
                        return None;
                    }
                }
            }
        };

        let derived_key = match node_lib::crypto::derive_key(
            self.crypto_config.kdf,
            &shared_secret_bytes,
            key_id,
            self.crypto_config.cipher.key_len(),
        ) {
            Ok(k) => k,
            Err(e) => {
                tracing::error!(error = %e, "Failed to derive key in handle_incoming_init");
                return None;
            }
        };

        self.established.insert(
            peer.bytes(),
            EstablishedKey {
                key: derived_key,
                key_id,
                established_at: Instant::now(),
            },
        );

        Some(our_response)
    }

    /// Get the established key for a peer, if any.
    pub fn get_key(&self, peer: MacAddress) -> Option<&[u8]> {
        self.established
            .get(&peer.bytes())
            .map(|e| e.key.as_slice())
    }

    /// Check if a pending exchange for a peer has timed out.
    pub fn is_pending_timed_out(&self, peer: MacAddress, timeout_ms: u64) -> bool {
        self.pending.get(&peer.bytes()).is_some_and(|p| {
            p.initiated_at.elapsed() >= std::time::Duration::from_millis(timeout_ms)
        })
    }

    /// Get the retry count for a pending exchange.
    pub fn pending_retries(&self, peer: MacAddress) -> Option<u32> {
        self.pending.get(&peer.bytes()).map(|p| p.retries)
    }

    /// Increment the retry counter for a pending exchange.
    pub fn increment_retries(&mut self, peer: MacAddress) {
        if let Some(p) = self.pending.get_mut(&peer.bytes()) {
            p.retries += 1;
        }
    }

    /// Remove a pending exchange (e.g. after max retries).
    pub fn remove_pending(&mut self, peer: MacAddress) {
        self.pending.remove(&peer.bytes());
    }

    /// Check if a key for a peer has expired based on the configured lifetime.
    pub fn is_key_expired(&self, peer: MacAddress, lifetime_ms: u64) -> bool {
        self.established.get(&peer.bytes()).is_some_and(|e| {
            e.established_at.elapsed() >= std::time::Duration::from_millis(lifetime_ms)
        })
    }

    /// Check if there is a pending exchange for a peer.
    pub fn has_pending(&self, peer: MacAddress) -> bool {
        self.pending.contains_key(&peer.bytes())
    }

    /// Check if there is an established key for a peer.
    pub fn has_established_key(&self, peer: MacAddress) -> bool {
        self.established.contains_key(&peer.bytes())
    }

    /// Return `(key_id, age_secs)` for the established key of a peer, if any.
    pub fn get_session_info(&self, peer: MacAddress) -> Option<(u32, u64)> {
        let key = self.established.get(&peer.bytes())?;
        Some((key.key_id, key.established_at.elapsed().as_secs()))
    }

    /// Clear any established key and pending exchange for a peer.
    ///
    /// Called when a `SessionTerminated` notice is received from the server so
    /// the OBU immediately stops sending encrypted traffic and re-initiates the
    /// DH handshake on the next opportunity.
    pub fn clear_session(&mut self, peer: MacAddress) {
        self.established.remove(&peer.bytes());
        self.pending.remove(&peer.bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initiate_and_complete_exchange_x25519() {
        let cfg = CryptoConfig::default();
        let mut store_a = DhKeyStore::new(cfg);
        let mut store_b = DhKeyStore::new(cfg);

        let mac_a: MacAddress = [1u8; 6].into();
        let mac_b: MacAddress = [2u8; 6].into();

        let (key_id, pub_a) = store_a.initiate_exchange(mac_b);
        let pub_b = store_b
            .handle_incoming_init(mac_a, key_id, &pub_a)
            .expect("should derive key");
        let (key_a, elapsed) = store_a
            .complete_exchange(mac_b, key_id, &pub_b)
            .expect("should complete");
        assert!(elapsed.as_millis() < 1000, "exchange should be fast");

        let key_b = store_b.get_key(mac_a).expect("should have key");
        assert_eq!(key_a, key_b);
    }

    #[test]
    fn initiate_and_complete_exchange_mlkem768() {
        use node_lib::crypto::{DhGroup, KdfAlgorithm, SigningAlgorithm, SymmetricCipher};
        let cfg = CryptoConfig {
            cipher: SymmetricCipher::default(),
            kdf: KdfAlgorithm::default(),
            dh_group: DhGroup::MlKem768,
            signing_algorithm: SigningAlgorithm::default(),
        };
        let mut store_a = DhKeyStore::new(cfg); // initiator (OBU)
        let mut store_b = DhKeyStore::new(cfg); // responder (server-side sim)

        let mac_a: MacAddress = [1u8; 6].into();
        let mac_b: MacAddress = [2u8; 6].into();

        let (key_id, ek_bytes) = store_a.initiate_exchange(mac_b);
        assert_eq!(
            ek_bytes.len(),
            ML_KEM_768_EK_LEN,
            "encap key should be 1184 bytes"
        );

        // store_b encapsulates (simulates server)
        let ct_bytes = store_b
            .handle_incoming_init(mac_a, key_id, &ek_bytes)
            .expect("should encapsulate");
        assert_eq!(
            ct_bytes.len(),
            ML_KEM_768_CT_LEN,
            "ciphertext should be 1088 bytes"
        );

        // store_a decapsulates
        let (key_a, elapsed) = store_a
            .complete_exchange(mac_b, key_id, &ct_bytes)
            .expect("should decapsulate");
        assert!(elapsed.as_millis() < 5000, "exchange should be fast");

        let key_b = store_b.get_key(mac_a).expect("should have key");
        assert_eq!(key_a, key_b, "both sides should derive the same key");
    }

    #[test]
    fn wrong_key_id_rejected() {
        let mut store = DhKeyStore::new(CryptoConfig::default());
        let peer: MacAddress = [3u8; 6].into();

        let (_key_id, _pub_key) = store.initiate_exchange(peer);
        let fake_pub = vec![0u8; 32];
        assert!(store.complete_exchange(peer, 999, &fake_pub).is_none());
    }

    #[test]
    fn has_pending_and_established() {
        let mut store = DhKeyStore::new(CryptoConfig::default());
        let peer: MacAddress = [4u8; 6].into();

        assert!(!store.has_pending(peer));
        assert!(!store.has_established_key(peer));

        let (key_id, _pub_key) = store.initiate_exchange(peer);
        assert!(store.has_pending(peer));

        let fake_peer_pub = vec![42u8; 32];
        store.complete_exchange(peer, key_id, &fake_peer_pub);
        assert!(!store.has_pending(peer));
        assert!(store.has_established_key(peer));
        assert!(store.get_key(peer).is_some());
    }

    #[test]
    fn unique_key_ids() {
        let mut store = DhKeyStore::new(CryptoConfig::default());
        let peer1: MacAddress = [5u8; 6].into();
        let peer2: MacAddress = [6u8; 6].into();

        let (id1, _) = store.initiate_exchange(peer1);
        let (id2, _) = store.initiate_exchange(peer2);
        assert_ne!(id1, id2);
    }

    #[test]
    fn configurable_cipher_produces_correct_key_len() {
        use node_lib::crypto::SymmetricCipher;

        let cfg_128 = CryptoConfig {
            cipher: SymmetricCipher::Aes128Gcm,
            ..CryptoConfig::default()
        };
        let mut store_a = DhKeyStore::new(cfg_128);
        let mut store_b = DhKeyStore::new(cfg_128);

        let mac_a: MacAddress = [10u8; 6].into();
        let mac_b: MacAddress = [20u8; 6].into();

        let (key_id, pub_a) = store_a.initiate_exchange(mac_b);
        let _pub_b = store_b
            .handle_incoming_init(mac_a, key_id, &pub_a)
            .expect("should derive key");

        let key_b = store_b.get_key(mac_a).expect("key");
        assert_eq!(key_b.len(), 16, "AES-128-GCM key should be 16 bytes");
    }
}
