use mac_address::MacAddress;
use node_lib::crypto::{CryptoConfig, DhKeypair};
use std::collections::HashMap;
use tokio::time::Instant;

/// State of a pending DH key exchange initiated by this node.
pub struct PendingExchange {
    pub keypair: DhKeypair,
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

    /// Start a new DH exchange with a peer. Returns the key_id and public key bytes.
    pub fn initiate_exchange(&mut self, peer: MacAddress) -> (u32, [u8; 32]) {
        let key_id = self.next_key_id;
        self.next_key_id = self.next_key_id.wrapping_add(1);

        let keypair = DhKeypair::generate();
        let public_bytes = *keypair.public.as_bytes();

        self.pending.insert(
            peer.bytes(),
            PendingExchange {
                keypair,
                key_id,
                initiated_at: Instant::now(),
                retries: 0,
            },
        );

        (key_id, public_bytes)
    }

    /// Re-initiate a timed-out DH exchange, preserving the retry count.
    /// Returns the new key_id and public key bytes.
    pub fn reinitiate_exchange(&mut self, peer: MacAddress) -> (u32, [u8; 32]) {
        let prev_retries = self
            .pending
            .get(&peer.bytes())
            .map(|p| p.retries)
            .unwrap_or(0);

        let key_id = self.next_key_id;
        self.next_key_id = self.next_key_id.wrapping_add(1);

        let keypair = DhKeypair::generate();
        let public_bytes = *keypair.public.as_bytes();

        self.pending.insert(
            peer.bytes(),
            PendingExchange {
                keypair,
                key_id,
                initiated_at: Instant::now(),
                retries: prev_retries + 1,
            },
        );

        (key_id, public_bytes)
    }

    /// Complete a DH exchange when we receive a reply.
    /// Returns the derived key on success.
    pub fn complete_exchange(
        &mut self,
        peer: MacAddress,
        key_id: u32,
        peer_public_bytes: &[u8; 32],
    ) -> Option<Vec<u8>> {
        let pending = self.pending.get(&peer.bytes())?;
        if pending.key_id != key_id {
            tracing::warn!(
                expected = pending.key_id,
                received = key_id,
                peer = %peer,
                "DH key_id mismatch, ignoring reply"
            );
            return None;
        }

        let peer_public = x25519_dalek::PublicKey::from(*peer_public_bytes);
        let shared_secret = pending.keypair.diffie_hellman(&peer_public);
        let derived_key = match node_lib::crypto::derive_key(
            self.crypto_config.kdf,
            shared_secret.as_bytes(),
            key_id,
            self.crypto_config.cipher.key_len(),
        ) {
            Ok(k) => k,
            Err(e) => {
                tracing::error!(error = %e, "Failed to derive DH key");
                return None;
            }
        };

        self.pending.remove(&peer.bytes());
        self.established.insert(
            peer.bytes(),
            EstablishedKey {
                key: derived_key.clone(),
                key_id,
                established_at: Instant::now(),
            },
        );

        Some(derived_key)
    }

    /// Handle an incoming DH init from a peer. Generates our keypair, computes
    /// the shared secret, stores the established key, and returns our public key bytes.
    ///
    /// Returns `None` if key derivation fails.
    pub fn handle_incoming_init(
        &mut self,
        peer: MacAddress,
        key_id: u32,
        peer_public_bytes: &[u8; 32],
    ) -> Option<[u8; 32]> {
        let our_keypair = DhKeypair::generate();
        let our_public = *our_keypair.public.as_bytes();

        let peer_public = x25519_dalek::PublicKey::from(*peer_public_bytes);
        let shared_secret = our_keypair.diffie_hellman(&peer_public);
        let derived_key = match node_lib::crypto::derive_key(
            self.crypto_config.kdf,
            shared_secret.as_bytes(),
            key_id,
            self.crypto_config.cipher.key_len(),
        ) {
            Ok(k) => k,
            Err(e) => {
                tracing::error!(error = %e, "Failed to derive DH key in handle_incoming_init");
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

        Some(our_public)
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initiate_and_complete_exchange() {
        let cfg = CryptoConfig::default();
        let mut store_a = DhKeyStore::new(cfg);
        let mut store_b = DhKeyStore::new(cfg);

        let mac_a: MacAddress = [1u8; 6].into();
        let mac_b: MacAddress = [2u8; 6].into();

        let (key_id, pub_a) = store_a.initiate_exchange(mac_b);
        let pub_b = store_b
            .handle_incoming_init(mac_a, key_id, &pub_a)
            .expect("should derive key");
        let key_a = store_a
            .complete_exchange(mac_b, key_id, &pub_b)
            .expect("should complete");

        let key_b = store_b.get_key(mac_a).expect("should have key");
        assert_eq!(key_a, key_b);
    }

    #[test]
    fn wrong_key_id_rejected() {
        let mut store = DhKeyStore::new(CryptoConfig::default());
        let peer: MacAddress = [3u8; 6].into();

        let (_key_id, _pub_key) = store.initiate_exchange(peer);
        let fake_pub = [0u8; 32];
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

        let fake_peer_pub = [42u8; 32];
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
