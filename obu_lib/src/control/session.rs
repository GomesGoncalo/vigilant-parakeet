use super::dh_key_store::DhKeyStore;
use super::node::{self, ReplyType};
use super::routing::Routing;
use anyhow::{anyhow, Result};
use common::network_interface::NetworkInterface;
use common::tun::Tun;
use futures::Future;
use mac_address::MacAddress;
use node_lib::control::client_cache::ClientCache;
use node_lib::crypto::{CryptoConfig, SigningKeypair};
use node_lib::messages::{
    auth::{
        key_exchange::{KeyExchangeInit, KeyExchangeReply},
        session_terminated::{SessionTerminated, CLOCK_SKEW_TOLERANCE_SECS, VALIDITY_SECS},
        Auth,
    },
    data::ToUpstream,
    message::Message,
    packet_type::PacketType,
};
use node_lib::{Shared, SharedDevice, SharedTun};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, RwLock};
use tokio::sync::Notify;
use tokio::time::Duration;
use tracing::Instrument;

use crate::args::ObuArgs;

// ── Type aliases ────────────────────────────────────────────────────────────

pub(super) type SharedKeyStore = Arc<RwLock<DhKeyStore>>;
type RevocationNonceCache = Arc<Mutex<VecDeque<([u8; 8], std::time::Instant)>>>;

// ── DH exchange action ───────────────────────────────────────────────────────

/// Action to take when a DH key exchange is needed.
#[derive(Debug, Clone, Copy)]
enum DhAction {
    /// No pending exchange exists — start a fresh one.
    Initiate,
    /// A pending exchange timed out — re-use its retry counter.
    Reinitiate,
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Decode a hex string of any length into bytes. Returns `None` on invalid hex.
fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok())
        .collect()
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Virtual MAC used to key the DH store for the server tunnel.
/// The OBU negotiates keys with the server (not peers), so we use a
/// fixed sentinel MAC as the store key.
pub(super) fn server_virtual_mac() -> MacAddress {
    MacAddress::new([0, 0, 0, 0, 0, 0])
}

// ── Session (TUN read) ────────────────────────────────────────────────────────

// Session support is under development - types are placeholders for future implementation
#[allow(dead_code)]
pub struct SessionParams {}

#[allow(dead_code)]
pub(crate) struct InnerSession {
    tun: Arc<Tun>,
}

#[allow(dead_code)]
pub enum Session {
    NoSession(Arc<Tun>),
    ValidSession(InnerSession),
}

impl Session {
    pub fn new(tun: Arc<Tun>) -> Self {
        Self::NoSession(tun)
    }

    pub async fn process<Fut>(
        &self,
        callable: impl FnOnce([u8; 1500], usize) -> Fut,
    ) -> Result<Option<Vec<ReplyType>>>
    where
        Fut: Future<Output = Result<Option<Vec<ReplyType>>>>,
    {
        match self {
            Self::NoSession(tun) => {
                // allocate a zeroed buffer and read into it safely
                let mut buf: [u8; 1500] = [0u8; 1500];
                let n = tun.recv(&mut buf).await?;
                callable(buf, n).await
            }
            Self::ValidSession(_session) => {
                // session handling not implemented yet
                todo!()
            }
        }
    }
}

// ── CryptoState ───────────────────────────────────────────────────────────────

/// All DH key-exchange and session-crypto state for an OBU.
///
/// Owns the key store, session flags, signing keypair, nonce replay-prevention
/// cache, and downstream client cache used for relay-mode key-exchange
/// forwarding.  Lock-order rule: always acquire `dh_key_store` before
/// `seen_revocation_nonces`; never hold either across an `.await`.
pub(super) struct CryptoState {
    pub(super) dh_key_store: SharedKeyStore,
    crypto_config: CryptoConfig,
    signing_keypair: Option<Arc<SigningKeypair>>,
    rekey_notify: Arc<Notify>,
    /// Time-bounded cache of recently-seen `SessionTerminated` nonces.
    seen_revocation_nonces: RevocationNonceCache,
    /// Downstream client cache for relay-mode operation.
    pub(super) downstream_client_cache: Arc<ClientCache>,
    // ── config values extracted from ObuArgs at construction ────────────────
    pub(super) enable_encryption: bool,
    enable_dh_signatures: bool,
    server_signing_pubkey: Option<String>,
    dh_rekey_interval_ms: u64,
    dh_key_lifetime_ms: u64,
    dh_reply_timeout_ms: u64,
}

impl CryptoState {
    /// Build crypto state from OBU args.  Generates or derives the signing
    /// keypair when `enable_dh_signatures` is set, and logs the public key.
    pub(super) fn new(args: &ObuArgs) -> Result<Self> {
        let crypto_config = CryptoConfig {
            cipher: args.obu_params.cipher,
            kdf: args.obu_params.kdf,
            dh_group: args.obu_params.dh_group,
            signing_algorithm: args.obu_params.signing_algorithm,
        };

        let signing_algo = args.obu_params.signing_algorithm;
        let signing_keypair = if args.obu_params.enable_dh_signatures {
            let kp = if let Some(ref hex_seed) = args.obu_params.signing_key_seed {
                let seed = node_lib::crypto::decode_hex_32(hex_seed).ok_or_else(|| {
                    anyhow!("signing_key_seed must be exactly 64 hex characters (32 bytes)")
                })?;
                SigningKeypair::from_seed(signing_algo, seed)
            } else {
                SigningKeypair::generate(signing_algo)
            };
            let pubkey_hex = encode_hex(&kp.verifying_key_bytes());
            tracing::info!(
                signing_pubkey = %pubkey_hex,
                "DH signing enabled — register this public key in the server's \
                 dh_signing_allowlist to enforce PKI authentication"
            );
            Some(Arc::new(kp))
        } else {
            None
        };

        tracing::info!(
            cipher = %crypto_config.cipher,
            kdf = %crypto_config.kdf,
            dh_group = %crypto_config.dh_group,
            encryption = args.obu_params.enable_encryption,
            dh_rekey_interval_ms = args.obu_params.dh_rekey_interval_ms,
            dh_key_lifetime_ms = args.obu_params.dh_key_lifetime_ms,
            dh_reply_timeout_ms = args.obu_params.dh_reply_timeout_ms,
            "CryptoState initialized"
        );

        Ok(Self {
            dh_key_store: Arc::new(RwLock::new(DhKeyStore::new(crypto_config))),
            crypto_config,
            signing_keypair,
            rekey_notify: Arc::new(Notify::new()),
            seen_revocation_nonces: Arc::new(Mutex::new(VecDeque::new())),
            downstream_client_cache: Arc::new(ClientCache::new()),
            enable_encryption: args.obu_params.enable_encryption,
            enable_dh_signatures: args.obu_params.enable_dh_signatures,
            server_signing_pubkey: args.obu_params.server_signing_pubkey.clone(),
            dh_rekey_interval_ms: args.obu_params.dh_rekey_interval_ms,
            dh_key_lifetime_ms: args.obu_params.dh_key_lifetime_ms,
            dh_reply_timeout_ms: args.obu_params.dh_reply_timeout_ms,
        })
    }

    // ── Key-store accessors ──────────────────────────────────────────────────

    /// Check whether a DH session with the server has been established.
    pub(super) fn has_dh_session(&self) -> bool {
        self.dh_key_store
            .read()
            .expect("dh key store read lock poisoned")
            .has_established_key(server_virtual_mac())
    }

    /// Return the established DH session info: `(key_id, age_secs)`.
    /// Returns `None` when no session has been established yet.
    pub(super) fn get_dh_session_info(&self) -> Option<(u32, u64)> {
        self.dh_key_store
            .read()
            .expect("dh key store read lock poisoned")
            .get_session_info(server_virtual_mac())
    }

    /// Immediately trigger a DH re-key exchange, bypassing the normal interval.
    ///
    /// Clears the current established session (if any) and wakes the re-keying
    /// task so it initiates a new key exchange on the next loop iteration.
    pub(super) fn trigger_rekey(&self) {
        {
            let mut store = self
                .dh_key_store
                .write()
                .expect("dh key store write lock poisoned");
            store.clear_session(server_virtual_mac());
        }
        self.rekey_notify.notify_one();
        tracing::info!("DH re-key triggered manually via admin interface");
    }

    /// Return whether a pending DH exchange is in progress.
    pub(super) fn has_dh_pending(&self) -> bool {
        self.dh_key_store
            .read()
            .expect("dh key store read lock poisoned")
            .has_pending(server_virtual_mac())
    }

    // ── Data-path helpers ────────────────────────────────────────────────────

    /// Decrypt an inbound downstream payload.
    ///
    /// Returns `Some(plaintext)` on success (or when encryption is disabled),
    /// and `None` when the frame must be dropped (no session, or decrypt error).
    pub(super) fn decrypt_downstream(&self, data: &[u8]) -> Option<Vec<u8>> {
        if !self.enable_encryption {
            return Some(data.to_vec());
        }
        let dh_store = self
            .dh_key_store
            .read()
            .expect("dh key store read lock poisoned");
        let dh_key = dh_store.get_key(server_virtual_mac());
        let cipher = self.crypto_config.cipher;
        let Some(key) = dh_key else {
            tracing::debug!(
                size = data.len(),
                cipher = %cipher,
                "No DH session established with server, dropping downstream frame"
            );
            return None;
        };
        match node_lib::crypto::decrypt_with_config(cipher, data, key) {
            Ok(decrypted) => Some(decrypted),
            Err(e) => {
                tracing::warn!(
                    size = data.len(),
                    cipher = %cipher,
                    error = %e,
                    "Failed to decrypt downstream frame, dropping"
                );
                None
            }
        }
    }

    // ── Background tasks ─────────────────────────────────────────────────────

    /// Spawn the periodic DH re-keying task.
    ///
    /// Normally wakes every `dh_rekey_interval_ms` to check whether a new key
    /// exchange is needed.  The `rekey_notify` Notify bypasses the sleep so the
    /// task reacts immediately when a `SessionTerminated` notice clears the key.
    pub(super) fn start_dh_rekey_task(
        crypto: Arc<Self>,
        device: SharedDevice,
        routing: Shared<Routing>,
        node_name: String,
    ) -> Result<()> {
        tracing::info!(
            cipher = %crypto.crypto_config.cipher,
            kdf = %crypto.crypto_config.kdf,
            dh_group = %crypto.crypto_config.dh_group,
            rekey_interval_ms = crypto.dh_rekey_interval_ms,
            "Starting DH re-keying task"
        );
        let rekey_interval = Duration::from_millis(crypto.dh_rekey_interval_ms);
        let key_lifetime_ms = crypto.dh_key_lifetime_ms;
        let reply_timeout_ms = crypto.dh_reply_timeout_ms;
        let rekey_notify = crypto.rekey_notify.clone();

        let span = tracing::info_span!("node", name = %node_name);
        tokio::task::spawn(
            async move {
                // Initial delay to allow routing to establish
                tokio::time::sleep(Duration::from_millis(500)).await;
                let cached_upstream = || {
                    routing
                        .read()
                        .expect("routing read lock poisoned")
                        .get_cached_upstream()
                };
                tracing::info!(
                    upstream = ?cached_upstream(),
                    "DH rekey task starting (initial delay elapsed)"
                );

                loop {
                    if let Some(mut upstream_mac) = cached_upstream() {
                        // Use server_virtual_mac() for key store lookups — the key
                        // is with the server, not with a specific RSU/peer.
                        let needs_exchange = {
                            let store = crypto
                                .dh_key_store
                                .read()
                                .expect("dh key store read lock poisoned");
                            let no_key = !store.has_established_key(server_virtual_mac());
                            let expired =
                                store.is_key_expired(server_virtual_mac(), key_lifetime_ms);
                            if expired {
                                tracing::debug!(
                                    via = %upstream_mac,
                                    lifetime_ms = key_lifetime_ms,
                                    "Server DH key expired, initiating re-key"
                                );
                            }
                            no_key || expired
                        };

                        if needs_exchange {
                            // Determine what action to take:
                            // - If no pending exchange, initiate a new one.
                            // - If pending exchange timed out, re-initiate (preserving retry count).
                            //   After 3 consecutive timeouts, failover to the next-best RSU candidate.
                            // - If pending exchange still in progress, wait.
                            let action: Option<DhAction> = {
                                let store = crypto
                                    .dh_key_store
                                    .read()
                                    .expect("dh key store read lock poisoned");
                                if !store.has_pending(server_virtual_mac()) {
                                    Some(DhAction::Initiate)
                                } else if store
                                    .is_pending_timed_out(server_virtual_mac(), reply_timeout_ms)
                                {
                                    let retries =
                                        store.pending_retries(server_virtual_mac()).unwrap_or(0);
                                    if retries >= 3 {
                                        if let Some(new_upstream) = routing
                                            .read()
                                            .expect("routing read lock")
                                            .failover_cached_upstream()
                                        {
                                            tracing::warn!(
                                                old_via = %upstream_mac,
                                                new_via = %new_upstream,
                                                retry = retries + 1,
                                                "DH timeout threshold reached, failing over to next RSU candidate"
                                            );
                                            upstream_mac = new_upstream;
                                        }
                                    } else {
                                        tracing::warn!(
                                            via = %upstream_mac,
                                            retry = retries + 1,
                                            "Server DH reply timed out, re-initiating (no session — packets will be dropped until established)"
                                        );
                                    }
                                    Some(DhAction::Reinitiate)
                                } else {
                                    None // still pending, wait
                                }
                            };

                            if let Some(action) = action {
                                let (key_id, pub_key) = {
                                    let mut store = crypto
                                        .dh_key_store
                                        .write()
                                        .expect("dh key store write lock poisoned");
                                    match action {
                                        DhAction::Reinitiate => {
                                            store.reinitiate_exchange(server_virtual_mac())
                                        }
                                        DhAction::Initiate => {
                                            store.initiate_exchange(server_virtual_mac())
                                        }
                                    }
                                };

                                let our_mac = device.mac_address();
                                let dh_group = crypto.crypto_config.dh_group;
                                let init_msg = if let Some(ref kp) = crypto.signing_keypair {
                                    let unsigned = KeyExchangeInit::new_unsigned(
                                        dh_group,
                                        key_id,
                                        pub_key.clone(),
                                        our_mac,
                                    );
                                    let base = unsigned.base_payload();
                                    let sig = kp.sign(&base);
                                    let spk = kp.verifying_key_bytes();
                                    KeyExchangeInit::new_signed(
                                        dh_group,
                                        key_id,
                                        pub_key,
                                        our_mac,
                                        kp.signing_algorithm(),
                                        spk,
                                        sig,
                                    )
                                } else {
                                    KeyExchangeInit::new_unsigned(dh_group, key_id, pub_key, our_mac)
                                };
                                let msg = Message::new(
                                    our_mac,
                                    upstream_mac,
                                    PacketType::Auth(Auth::KeyExchangeInit(init_msg)),
                                );
                                let wire: Vec<u8> = (&msg).into();

                                if let Err(e) = device.send(&wire).await {
                                    tracing::warn!(
                                        error = %e,
                                        via = %upstream_mac,
                                        key_id = key_id,
                                        "Failed to send DH KeyExchangeInit (for server)"
                                    );
                                } else {
                                    tracing::info!(
                                        via = %upstream_mac,
                                        key_id = key_id,
                                        mode = ?action,
                                        dh_group = %crypto.crypto_config.dh_group,
                                        "Sent DH KeyExchangeInit upstream"
                                    );
                                }
                            }
                        }
                    } else {
                        tracing::warn!(
                            "DH rekey task: no upstream RSU cached yet — skipping exchange until next wakeup"
                        );
                    }

                    // Wait for either the normal rekey interval or an early wake-up
                    // triggered by receiving a SessionTerminated notice.
                    // When a DH exchange is in-flight, use a short sleep equal to
                    // reply_timeout_ms so we wake promptly to detect the timeout and
                    // retransmit.  When no upstream was available, also use a short
                    // retry so we attempt again as soon as the first heartbeat
                    // populates the routing table.
                    let no_upstream = cached_upstream().is_none();
                    let exchange_pending = crypto
                        .dh_key_store
                        .read()
                        .map(|g| g.has_pending(server_virtual_mac()))
                        .unwrap_or(false);
                    let sleep_duration = if exchange_pending || no_upstream {
                        Duration::from_millis(reply_timeout_ms)
                    } else {
                        rekey_interval
                    };
                    tokio::select! {
                        _ = tokio::time::sleep(sleep_duration) => {}
                        _ = rekey_notify.notified() => {
                            tracing::debug!("DH rekey task woken early by SessionTerminated");
                        }
                    }
                }
            }
            .instrument(span),
        );
        Ok(())
    }

    /// Spawn the TUN session task.
    ///
    /// Reads IP frames from the TUN interface, optionally encrypts them with
    /// the established DH session key, and forwards them upstream.
    pub(super) fn start_session_task(
        crypto: Arc<Self>,
        session: Arc<Session>,
        tun: SharedTun,
        device: SharedDevice,
        routing: Shared<Routing>,
        node_name: String,
    ) -> Result<()> {
        let span = tracing::info_span!("node", name = %node_name);
        tokio::task::spawn(
            async move {
                loop {
                    let devicec = device.clone();
                    let routing_for_closure = routing.clone();
                    let routing_for_handle = routing.clone();
                    let crypto_c = crypto.clone();
                    let messages = session
                        .process(|x, size| async move {
                            let y: &[u8] = &x[..size];
                            let Some(upstream) = routing_for_closure
                                .read()
                                .expect("routing table read lock poisoned in session task")
                                .get_route_to(None)
                            else {
                                return Ok(None);
                            };

                            let payload_data = if crypto_c.enable_encryption {
                                // Always use server_virtual_mac() — the key is
                                // negotiated with the server, not the RSU.
                                let dh_store_guard = crypto_c
                                    .dh_key_store
                                    .read()
                                    .expect("dh key store read lock poisoned");
                                let dh_key = dh_store_guard.get_key(server_virtual_mac());
                                let cipher = crypto_c.crypto_config.cipher;
                                let Some(key) = dh_key else {
                                    tracing::debug!(
                                        size = y.len(),
                                        cipher = %cipher,
                                        "No DH session established with server, dropping upstream frame"
                                    );
                                    return Ok(None);
                                };
                                match node_lib::crypto::encrypt_with_config(cipher, y, key) {
                                    Ok(encrypted_data) => encrypted_data,
                                    Err(e) => {
                                        tracing::error!(
                                            size = y.len(),
                                            cipher = %cipher,
                                            error = %e,
                                            "Failed to encrypt upstream frame"
                                        );
                                        return Ok(None);
                                    }
                                }
                            } else {
                                y.to_vec()
                            };

                            let origin = devicec.mac_address();
                            let mut wire = Vec::with_capacity(24 + payload_data.len());
                            let tu = ToUpstream::new(origin, &payload_data);
                            Message::serialize_upstream_forward_into(
                                &tu,
                                origin,
                                upstream.mac,
                                &mut wire,
                            );
                            let outgoing = vec![ReplyType::WireFlat(wire)];
                            Ok(Some(outgoing))
                        })
                        .await;

                    if let Ok(Some(messages)) = messages {
                        let _ = node::handle_messages(
                            messages,
                            &tun,
                            &device,
                            Some(routing_for_handle.clone()),
                        )
                        .await;
                    }
                }
            }
            .instrument(span),
        );
        Ok(())
    }

    // ── Control message handlers ──────────────────────────────────────────────

    /// Handle an inbound `KeyExchangeInit`.
    ///
    /// OBUs forward the message upstream toward the server, recording the
    /// downstream path in the client cache so the reply can be routed back.
    pub(super) fn handle_key_exchange_init(
        &self,
        ke_init: &KeyExchangeInit<'_>,
        msg: &Message<'_>,
        device_mac: MacAddress,
        routing: &Shared<Routing>,
    ) -> Result<Option<Vec<ReplyType>>> {
        let routing = routing.read().expect("routing table read lock poisoned");
        let Some(upstream) = routing.get_route_to(None) else {
            tracing::warn!(
                obu = %ke_init.sender(),
                "No upstream route — dropping KeyExchangeInit (relay OBU has no path to server)"
            );
            return Ok(None);
        };

        // Record the downstream path so we can route the reply back without
        // relying solely on the heartbeat-reply-based routing table.
        if let Ok(from_mac) = msg.from() {
            self.downstream_client_cache
                .store_mac(ke_init.sender(), from_mac);
        }

        // Preserve all fields (algorithm, key material, signature) when forwarding.
        let init = ke_init.clone_into_owned();
        let fwd = Message::new(
            device_mac,
            upstream.mac,
            PacketType::Auth(Auth::KeyExchangeInit(init)),
        );
        let wire: Vec<u8> = (&fwd).into();
        tracing::info!(
            obu = %ke_init.sender(),
            via = %upstream.mac,
            signed = ke_init.is_signed(),
            "Forwarding KeyExchangeInit upstream"
        );
        Ok(Some(vec![ReplyType::WireFlat(wire)]))
    }

    /// Handle an inbound `SessionTerminated` notice.
    ///
    /// Forwards the notice downstream if it is not addressed to this node.
    /// If it is for us, authenticates it (signature, timestamp, nonce replay
    /// prevention), clears the DH session, and wakes the re-keying task.
    pub(super) fn handle_session_terminated(
        &self,
        st: &SessionTerminated<'_>,
        device_mac: MacAddress,
        routing: &Shared<Routing>,
    ) -> Result<Option<Vec<ReplyType>>> {
        let target = st.target();

        if target != device_mac {
            // Not for us — forward toward the target OBU.
            let routing = routing.read().expect("routing table read lock poisoned");
            let Some(next_hop) = routing.get_route_to(Some(target)) else {
                tracing::debug!(
                    target = %target,
                    "No route to forward SessionTerminated, dropping"
                );
                return Ok(None);
            };
            let owned = st.clone_into_owned();
            let fwd = Message::new(
                device_mac,
                next_hop.mac,
                PacketType::Auth(Auth::SessionTerminated(owned)),
            );
            let wire: Vec<u8> = (&fwd).into();
            tracing::debug!(
                target = %target,
                via = %next_hop.mac,
                "Forwarding SessionTerminated down the tree"
            );
            return Ok(Some(vec![ReplyType::WireFlat(wire)]));
        }

        // It's for us.
        //
        // Authenticate the revocation notice before acting on it to prevent DoS:
        //   1. If server_signing_pubkey is configured: require a valid signature.
        //   2. Verify the signature over [0x04][TARGET_MAC 6B][TIMESTAMP 8B][NONCE 8B].
        //   3. Check timestamp is within the validity window.
        //   4. Check the nonce has not been seen within the window (replay prevention).
        if let Some(ref expected_hex) = self.server_signing_pubkey {
            let (ts, nonce, algo, sig) = match (
                st.timestamp_secs(),
                st.nonce(),
                st.signing_algorithm(),
                st.signature(),
            ) {
                (Some(t), Some(n), Some(algo), Some(s)) => (t, n, algo, s),
                _ => {
                    tracing::warn!(
                        "SessionTerminated is unsigned but server_signing_pubkey is \
                             configured — dropping (possible replay or misconfigured server)"
                    );
                    return Ok(None);
                }
            };

            let now_secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let too_old = ts < now_secs.saturating_sub(VALIDITY_SECS);
            let too_new = ts > now_secs.saturating_add(CLOCK_SKEW_TOLERANCE_SECS);
            if too_old || too_new {
                tracing::warn!(
                    msg_ts = ts,
                    now = now_secs,
                    "SessionTerminated timestamp outside validity window — dropping"
                );
                return Ok(None);
            }

            let expected_bytes = match decode_hex(expected_hex) {
                Some(b) if b.len() == 32 || b.len() == 1952 => b,
                _ => {
                    tracing::warn!(
                        "server_signing_pubkey is invalid hex or has wrong length, \
                         cannot verify SessionTerminated — dropping"
                    );
                    return Ok(None);
                }
            };

            let payload = SessionTerminated::build_signed_payload(device_mac, ts, *nonce);

            if let Err(e) =
                node_lib::crypto::verify_dh_signature(algo, &payload, &expected_bytes, sig)
            {
                tracing::warn!(error = %e, "SessionTerminated has invalid signature — dropping");
                return Ok(None);
            }

            // Signature valid — check nonce freshness.
            let validity = std::time::Duration::from_secs(VALIDITY_SECS);
            {
                let mut cache = self
                    .seen_revocation_nonces
                    .lock()
                    .expect("revocation nonce cache lock poisoned");
                while cache.front().is_some_and(|(_, t)| t.elapsed() > validity) {
                    cache.pop_front();
                }
                if cache.iter().any(|(n, _)| n == nonce) {
                    tracing::debug!("SessionTerminated nonce already seen — dropping (replay)");
                    return Ok(None);
                }
                cache.push_back((*nonce, std::time::Instant::now()));
            }

            tracing::debug!(
                ts,
                "SessionTerminated signature, timestamp, and nonce verified"
            );
        }

        // Clear the DH session and wake the re-keying task.
        {
            let mut store = self
                .dh_key_store
                .write()
                .expect("dh key store write lock poisoned");
            store.clear_session(server_virtual_mac());
        }
        tracing::warn!(
            "SessionTerminated received from server — DH session cleared, re-keying immediately"
        );
        self.rekey_notify.notify_one();
        Ok(None)
    }

    /// Handle an inbound `KeyExchangeReply`.
    ///
    /// Forwards the reply downstream when it is addressed to another OBU.
    /// When addressed to this node, verifies the server's signature (if
    /// configured) and completes the DH key exchange to establish a session key.
    pub(super) fn handle_key_exchange_reply(
        &self,
        ke_reply: &KeyExchangeReply<'_>,
        msg: &Message<'_>,
        device_mac: MacAddress,
        routing: &Shared<Routing>,
    ) -> Result<Option<Vec<ReplyType>>> {
        tracing::info!(
            key_id = ke_reply.key_id(),
            dest = %ke_reply.sender(),
            my_mac = %device_mac,
            wire_from = ?msg.from().ok(),
            "KeyExchangeReply received on VANET"
        );

        if !self.enable_encryption {
            return Ok(None);
        }

        // The sender field carries the final destination OBU MAC (set by the server).
        let dest = ke_reply.sender();
        if dest != device_mac {
            // Not for us — forward down the tree.
            let cache_hit = self.downstream_client_cache.get(dest);
            let next_hop_mac = cache_hit.or_else(|| {
                let routing = routing.read().expect("routing table read lock poisoned");
                routing.get_route_to(Some(dest)).map(|r| r.mac)
            });
            let Some(next_hop_mac) = next_hop_mac else {
                tracing::warn!(
                    dest = %dest,
                    key_id = ke_reply.key_id(),
                    "No route to forward KeyExchangeReply (not in downstream cache or routing table), dropping"
                );
                return Ok(None);
            };

            let reply = ke_reply.clone_into_owned();
            let fwd = Message::new(
                device_mac,
                next_hop_mac,
                PacketType::Auth(Auth::KeyExchangeReply(reply)),
            );
            let wire: Vec<u8> = (&fwd).into();
            tracing::info!(
                dest = %dest,
                via = %next_hop_mac,
                key_id = ke_reply.key_id(),
                "Relaying KeyExchangeReply down the tree"
            );
            return Ok(Some(vec![ReplyType::WireFlat(wire)]));
        }

        // It's for us — verify the server's signature before completing the exchange.
        if self.enable_dh_signatures {
            match (
                ke_reply.signing_algorithm(),
                ke_reply.signing_pubkey(),
                ke_reply.signature(),
            ) {
                (Some(algo), Some(spk), Some(sig)) => {
                    let base = ke_reply.base_payload();
                    if let Err(e) = node_lib::crypto::verify_dh_signature(algo, &base, spk, sig) {
                        tracing::warn!(
                            error = %e,
                            key_id = ke_reply.key_id(),
                            "KeyExchangeReply has invalid signature, dropping"
                        );
                        return Ok(None);
                    }
                    tracing::debug!(
                        key_id = ke_reply.key_id(),
                        "KeyExchangeReply signature verified"
                    );
                }
                _ => {
                    tracing::warn!(
                        key_id = ke_reply.key_id(),
                        "KeyExchangeReply is unsigned but enable_dh_signatures is set, dropping"
                    );
                    return Ok(None);
                }
            }
        }

        // PKI: if a pinned server public key is configured, reject replies from
        // any server whose signing key doesn't match.
        if self.server_signing_pubkey.is_some() && !self.enable_dh_signatures {
            tracing::warn!(
                key_id = ke_reply.key_id(),
                "server_signing_pubkey is configured but enable_dh_signatures is false; \
                 dropping KeyExchangeReply to prevent key-pinning bypass"
            );
            return Ok(None);
        }
        if let Some(ref expected_hex) = self.server_signing_pubkey {
            let expected_bytes =
                decode_hex(expected_hex).filter(|b| b.len() == 32 || b.len() == 1952);
            if expected_bytes.is_none() && !expected_hex.is_empty() {
                tracing::warn!(
                    key_id = ke_reply.key_id(),
                    "server_signing_pubkey is invalid hex or has wrong length \
                     (expected 32B Ed25519 or 1952B ML-DSA-65), dropping reply"
                );
                return Ok(None);
            }
            match (ke_reply.signing_pubkey(), expected_bytes) {
                (Some(spk), Some(ref expected)) if spk == expected.as_slice() => {
                    tracing::debug!(
                        key_id = ke_reply.key_id(),
                        "KeyExchangeReply signing key matches pinned server pubkey"
                    );
                }
                (Some(_), Some(_)) => {
                    tracing::warn!(
                        key_id = ke_reply.key_id(),
                        "KeyExchangeReply signing key does not match pinned server pubkey, dropping"
                    );
                    return Ok(None);
                }
                (None, _) => {
                    tracing::warn!(
                        key_id = ke_reply.key_id(),
                        "KeyExchangeReply is unsigned but server_signing_pubkey is configured, dropping"
                    );
                    return Ok(None);
                }
                (_, None) => {
                    tracing::warn!(
                        "server_signing_pubkey is not valid hex, cannot verify reply — dropping"
                    );
                    return Ok(None);
                }
            }
        }

        // Validate that the reply algorithm matches what we initiated.
        {
            let expected_group = self.crypto_config.dh_group;
            match ke_reply.dh_group() {
                Some(g) if g == expected_group => {}
                received => {
                    tracing::warn!(
                        key_id = ke_reply.key_id(),
                        expected = %expected_group,
                        received = ?received,
                        "KeyExchangeReply dh_group does not match initiated algorithm, dropping"
                    );
                    return Ok(None);
                }
            }
        }

        // Complete the key exchange.
        let key_id = ke_reply.key_id();
        let peer_response = ke_reply.key_material();

        let result = {
            let mut store = self
                .dh_key_store
                .write()
                .expect("dh key store write lock poisoned");
            store.complete_exchange(server_virtual_mac(), key_id, peer_response)
        };

        match result {
            Some((ref derived_key, elapsed)) => {
                tracing::info!(
                    key_id = key_id,
                    cipher = %self.crypto_config.cipher,
                    kdf = %self.crypto_config.kdf,
                    key_len = derived_key.len(),
                    elapsed_ms = elapsed.as_millis() as u64,
                    "DH key exchange with server completed, session key established"
                );
            }
            None => {
                tracing::warn!(
                    key_id = key_id,
                    "Failed to complete DH key exchange with server — no matching pending exchange"
                );
            }
        }

        Ok(None)
    }
}
