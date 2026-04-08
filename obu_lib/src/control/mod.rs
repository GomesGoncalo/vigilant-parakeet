pub mod dh_key_store;
pub mod node;
pub mod routing;
mod routing_cache;

// Re-export shared modules from node_lib to avoid duplication
pub use node_lib::control::route;
pub use node_lib::control::routing_utils;
mod session;

use crate::args::ObuArgs;
use anyhow::{anyhow, Result};
use common::tun::Tun;
use common::{device::Device, network_interface::NetworkInterface};
use dh_key_store::DhKeyStore;
use mac_address::MacAddress;
use node::ReplyType;
use node_lib::crypto::{sig_algo_from_id, CryptoConfig, SigningAlgorithm, SigningKeypair};
use node_lib::messages::{
    control::{
        key_exchange::{KeyExchangeInit, KeyExchangeReply},
        session_terminated::SessionTerminated,
        Control,
    },
    data::{Data, ToUpstream},
    message::Message,
    packet_type::PacketType,
};
use routing::Routing;
use session::Session;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, RwLock};
use tokio::sync::Notify;
use tokio::time::{Duration, Instant};
use tracing::Instrument;

// Re-export type aliases for cleaner code
use node_lib::{Shared, SharedDevice, SharedTun};

type SharedKeyStore = Arc<RwLock<DhKeyStore>>;
type RevocationNonceCache = Arc<Mutex<VecDeque<([u8; 8], std::time::Instant)>>>;

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
fn server_virtual_mac() -> MacAddress {
    MacAddress::new([0, 0, 0, 0, 0, 0])
}

pub struct Obu {
    args: ObuArgs,
    routing: Shared<Routing>,
    tun: SharedTun,
    device: SharedDevice,
    session: Arc<Session>,
    node_name: String,
    dh_key_store: SharedKeyStore,
    crypto_config: CryptoConfig,
    /// Ed25519 identity keypair used to sign outgoing DH messages.
    /// Present only when `enable_dh_signatures` is `true`.
    signing_keypair: Option<Arc<SigningKeypair>>,
    /// Wakes the DH re-keying task immediately when a `SessionTerminated` notice
    /// is received, bypassing the normal re-key interval sleep.
    rekey_notify: Arc<Notify>,
    /// Time-bounded cache of recently-seen `SessionTerminated` nonces.
    /// Each entry is `(nonce, received_at)`. Entries older than `VALIDITY_SECS`
    /// are pruned on each check. Because eviction is time-driven (not count-driven)
    /// the cache never grows stale: old nonces expire along with the messages that
    /// carried them, so there is no fixed-size window that an attacker could outlast.
    seen_revocation_nonces: RevocationNonceCache,
}

impl Obu {
    pub fn new(
        args: ObuArgs,
        tun: Arc<Tun>,
        device: Arc<Device>,
        node_name: String,
    ) -> Result<Arc<Self>> {
        let _span = tracing::info_span!("node", name = %node_name).entered();

        let boot = Instant::now();
        let routing = Arc::new(RwLock::new(Routing::new(&args, &boot)?));

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

        let dh_key_store = Arc::new(RwLock::new(DhKeyStore::new(crypto_config)));
        let rekey_notify = Arc::new(Notify::new());
        let seen_revocation_nonces = Arc::new(Mutex::new(VecDeque::new()));
        let obu = Arc::new(Self {
            args,
            routing,
            tun: tun.clone(),
            device,
            session: Session::new(tun).into(),
            node_name,
            dh_key_store,
            crypto_config,
            signing_keypair,
            rekey_notify,
            seen_revocation_nonces,
        });

        tracing::info!(
            bind = %obu.args.bind,
            mac = %obu.device.mac_address(),
            mtu = obu.args.mtu,
            hello_history = obu.args.obu_params.hello_history,
            cached_candidates = obu.args.obu_params.cached_candidates,
            encryption = obu.args.obu_params.enable_encryption,
            cipher = %obu.crypto_config.cipher,
            dh_enabled = obu.args.obu_params.enable_encryption,
            dh_group = %obu.crypto_config.dh_group,
            kdf = %obu.crypto_config.kdf,
            dh_rekey_interval_ms = obu.args.obu_params.dh_rekey_interval_ms,
            dh_key_lifetime_ms = obu.args.obu_params.dh_key_lifetime_ms,
            dh_reply_timeout_ms = obu.args.obu_params.dh_reply_timeout_ms,
            "OBU initialized"
        );
        if !obu.args.obu_params.enable_encryption {
            tracing::warn!(
                "Encryption is DISABLED — all traffic is sent in the clear. \
                 Set --enable-encryption to protect data payloads."
            );
        }
        if !obu.args.obu_params.enable_dh_signatures {
            tracing::warn!(
                "DH signatures are DISABLED — key exchange messages are not authenticated. \
                 Set --enable-dh-signatures to prevent MITM key substitution."
            );
        }
        obu.session_task()?;
        Obu::wire_traffic_task(obu.clone())?;
        if obu.args.obu_params.enable_encryption {
            tracing::info!(
                cipher = %obu.crypto_config.cipher,
                kdf = %obu.crypto_config.kdf,
                dh_group = %obu.crypto_config.dh_group,
                rekey_interval_ms = obu.args.obu_params.dh_rekey_interval_ms,
                "Starting DH re-keying task"
            );
            Obu::dh_rekey_task(obu.clone())?;
        }
        Ok(obu)
    }

    /// Attach a live RSSI table for proximity-based RSU selection.
    ///
    /// The table maps neighbor MAC addresses to their received signal strength in dBm.
    /// It is populated by the simulator fading task (from computed distances) or by a
    /// real radio driver.  Once attached, `select_and_cache_upstream` uses RSSI as the
    /// primary RSU selection metric with a 3 dB hysteresis.
    pub fn set_rssi_table(&self, table: routing::RssiTable) {
        self.routing
            .write()
            .expect("routing table write lock poisoned")
            .set_rssi_table(table);
    }

    /// Return the cached upstream MAC if present.
    pub fn cached_upstream_mac(&self) -> Option<mac_address::MacAddress> {
        self.routing
            .read()
            .expect("routing table read lock poisoned")
            .get_cached_upstream()
    }

    /// Check whether a DH session with the server has been established.
    pub fn has_dh_session(&self) -> bool {
        self.dh_key_store
            .read()
            .expect("dh key store read lock poisoned")
            .has_established_key(server_virtual_mac())
    }

    /// Return the cached upstream Route if present (hops, mac, latency).
    pub fn cached_upstream_route(&self) -> Option<route::Route> {
        self.routing
            .read()
            .expect("routing table read lock poisoned")
            .get_route_to(None)
    }

    /// Return the number of cached upstream candidates kept for failover.
    pub fn cached_upstream_candidates_len(&self) -> usize {
        self.routing
            .read()
            .expect("routing table read lock poisoned")
            .get_cached_candidates()
            .map(|v| v.len())
            .unwrap_or(0)
    }

    /// Return this node's name.
    pub fn node_name(&self) -> &str {
        &self.node_name
    }

    /// Return this node's VANET MAC address.
    pub fn mac_address(&self) -> MacAddress {
        self.device.mac_address()
    }

    /// Return the ordered list of cached upstream candidates (primary first).
    pub fn get_upstream_candidates(&self) -> Vec<MacAddress> {
        self.routing
            .read()
            .expect("routing table read lock poisoned")
            .get_cached_candidates()
            .unwrap_or_default()
    }

    /// Return the established DH session info: `(key_id, age_secs)`.
    /// Returns `None` when no session has been established yet.
    pub fn get_dh_session_info(&self) -> Option<(u32, u64)> {
        self.dh_key_store
            .read()
            .expect("dh key store read lock poisoned")
            .get_session_info(server_virtual_mac())
    }

    /// Immediately trigger a DH re-key exchange, bypassing the normal interval.
    ///
    /// Clears the current established session (if any) and wakes the re-keying
    /// task so it initiates a new key exchange on the next loop iteration.
    pub fn trigger_rekey(&self) {
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
    pub fn has_dh_pending(&self) -> bool {
        self.dh_key_store
            .read()
            .expect("dh key store read lock poisoned")
            .has_pending(server_virtual_mac())
    }

    /// Periodic DH re-keying task.
    ///
    /// Normally wakes every `dh_rekey_interval_ms` to check whether a new key
    /// exchange is needed.  The `rekey_notify` Notify bypasses the sleep so the
    /// task reacts immediately when a `SessionTerminated` notice clears the key.
    fn dh_rekey_task(obu: Arc<Self>) -> Result<()> {
        let rekey_interval = Duration::from_millis(obu.args.obu_params.dh_rekey_interval_ms);
        let key_lifetime_ms = obu.args.obu_params.dh_key_lifetime_ms;
        let reply_timeout_ms = obu.args.obu_params.dh_reply_timeout_ms;
        let node_name = obu.node_name.clone();
        let rekey_notify = obu.rekey_notify.clone();

        let span = tracing::info_span!("node", name = %node_name);
        tokio::task::spawn(
            async move {
                // Initial delay to allow routing to establish
                tokio::time::sleep(Duration::from_millis(500)).await;

                loop {
                    if let Some(upstream_mac) = obu.cached_upstream_mac() {
                        // Use server_virtual_mac() for key store lookups — the key
                        // is with the server, not with a specific RSU/peer.
                        let needs_exchange = {
                            let store = obu
                                .dh_key_store
                                .read()
                                .expect("dh key store read lock poisoned");
                            let no_key = !store.has_established_key(server_virtual_mac());
                            let expired = store.is_key_expired(server_virtual_mac(), key_lifetime_ms);
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
                            // - If no pending exchange, initiate a new one
                            // - If pending exchange timed out, re-initiate (preserving retry count)
                            // - If pending exchange still in progress, wait
                            let action = {
                                let store = obu
                                    .dh_key_store
                                    .read()
                                    .expect("dh key store read lock poisoned");
                                if !store.has_pending(server_virtual_mac()) {
                                    Some("initiate")
                                } else if store.is_pending_timed_out(server_virtual_mac(), reply_timeout_ms) {
                                    let retries = store.pending_retries(server_virtual_mac()).unwrap_or(0);
                                    tracing::warn!(
                                        via = %upstream_mac,
                                        retry = retries + 1,
                                        "Server DH reply timed out, re-initiating (no session — packets will be dropped until established)"
                                    );
                                    Some("reinitiate")
                                } else {
                                    None // still pending, wait
                                }
                            };

                            if let Some(mode) = action {
                                let (key_id, pub_key) = {
                                    let mut store = obu
                                        .dh_key_store
                                        .write()
                                        .expect("dh key store write lock poisoned");
                                    if mode == "reinitiate" {
                                        store.reinitiate_exchange(server_virtual_mac())
                                    } else {
                                        store.initiate_exchange(server_virtual_mac())
                                    }
                                };

                                let our_mac = obu.device.mac_address();
                                let algo_id = match obu.crypto_config.dh_group {
                                    node_lib::crypto::DhGroup::X25519 => {
                                        node_lib::messages::control::key_exchange::KE_ALGO_X25519
                                    }
                                    node_lib::crypto::DhGroup::MlKem768 => {
                                        node_lib::messages::control::key_exchange::KE_ALGO_ML_KEM_768
                                    }
                                };
                                let init_msg = if let Some(ref kp) = obu.signing_keypair {
                                    let sig_algo_id = match kp.signing_algorithm() {
                                        SigningAlgorithm::Ed25519 => {
                                            node_lib::messages::control::key_exchange::SIG_ALGO_ED25519
                                        }
                                        SigningAlgorithm::MlDsa65 => {
                                            node_lib::messages::control::key_exchange::SIG_ALGO_ML_DSA_65
                                        }
                                    };
                                    let unsigned = KeyExchangeInit::new_raw(
                                        algo_id, key_id, pub_key.clone(), our_mac,
                                        None, None, None,
                                    );
                                    let base = unsigned.base_payload();
                                    let sig = kp.sign(&base);
                                    let spk = kp.verifying_key_bytes();
                                    KeyExchangeInit::new_raw(
                                        algo_id, key_id, pub_key, our_mac,
                                        Some(sig_algo_id), Some(spk), Some(sig),
                                    )
                                } else {
                                    KeyExchangeInit::new_raw(
                                        algo_id, key_id, pub_key, our_mac,
                                        None, None, None,
                                    )
                                };
                                // Send to upstream RSU — it will relay to server
                                let msg = Message::new(
                                    our_mac,
                                    upstream_mac,
                                    PacketType::Control(Control::KeyExchangeInit(init_msg)),
                                );
                                let wire: Vec<u8> = (&msg).into();

                                if let Err(e) = obu.device.send(&wire).await {
                                    tracing::warn!(
                                        error = %e,
                                        via = %upstream_mac,
                                        key_id = key_id,
                                        "Failed to send DH KeyExchangeInit (for server)"
                                    );
                                } else {
                                    tracing::debug!(
                                        via = %upstream_mac,
                                        key_id = key_id,
                                        dh_group = %obu.crypto_config.dh_group,
                                        "Sent DH KeyExchangeInit to server (via RSU)"
                                    );
                                }
                            }
                        }
                    }

                    // Wait for either the normal rekey interval or an early wake-up
                    // triggered by receiving a SessionTerminated notice.
                    // When a DH exchange is in-flight, use a short sleep equal to
                    // reply_timeout_ms so we wake promptly to detect the timeout and
                    // retransmit, rather than waiting the full 12-hour rekey interval
                    // before retrying a failed initial exchange.
                    let exchange_pending = obu
                        .dh_key_store
                        .read()
                        .map(|g| g.has_pending(server_virtual_mac()))
                        .unwrap_or(false);
                    let sleep_duration = if exchange_pending {
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

    fn wire_traffic_task(obu: Arc<Self>) -> Result<()> {
        let device = obu.device.clone();
        let tun = obu.tun.clone();
        let routing_handle = obu.routing.clone();
        let node_name = obu.node_name.clone();

        let span = tracing::info_span!("node", name = %node_name);
        tokio::task::spawn(
            async move {
                loop {
                    let obu_c = obu.clone();
                    let messages = node::wire_traffic(&device, |pkt, size| {
                        let obu = obu_c.clone();
                        async move {
                            let data = &pkt[..size];
                            let mut all_responses = Vec::new();
                            let mut offset = 0;

                            while offset < data.len() {
                                match Message::try_from(&data[offset..]) {
                                    Ok(msg) => {
                                        let response = obu.handle_msg(&msg).await;

                                        if let Ok(Some(responses)) = response {
                                            all_responses.extend(responses);
                                        }
                                        let msg_bytes: Vec<u8> = (&msg).into();
                                        let msg_size: usize = msg_bytes.len();
                                        offset += msg_size;
                                    }
                                    Err(e) => {
                                        tracing::trace!(
                                            offset = offset,
                                            remaining = data.len() - offset,
                                            total_size = data.len(),
                                            error = %e,
                                            "Failed to parse message, stopping batch processing"
                                        );
                                        break;
                                    }
                                }
                            }

                            if all_responses.is_empty() {
                                Ok(None)
                            } else {
                                Ok(Some(all_responses))
                            }
                        }
                    })
                    .await;
                    if let Ok(Some(messages)) = messages {
                        let _ = node::handle_messages_batched(
                            messages,
                            &tun,
                            &device,
                            Some(routing_handle.clone()),
                        )
                        .await;
                    }
                }
            }
            .instrument(span),
        );
        Ok(())
    }

    fn session_task(&self) -> Result<()> {
        let routing = self.routing.clone();
        let session = self.session.clone();
        let device = self.device.clone();
        let tun = self.tun.clone();
        let routing_handle = routing.clone();
        let enable_encryption = self.args.obu_params.enable_encryption;
        let cipher = self.crypto_config.cipher;
        let dh_key_store = self.dh_key_store.clone();
        let node_name = self.node_name.clone();

        let span = tracing::info_span!("node", name = %node_name);
        tokio::task::spawn(
            async move {
                loop {
                    let devicec = device.clone();
                    let routing_for_closure = routing_handle.clone();
                    let routing_for_handle = routing_handle.clone();
                    let dh_store = dh_key_store.clone();
                    let messages = session
                        .process(|x, size| async move {
                            let y: &[u8] = &x[..size];
                            let Some(upstream) = routing_for_closure
                                .read()
                                .expect("routing table read lock poisoned in heartbeat task")
                                .get_route_to(None)
                            else {
                                return Ok(None);
                            };

                            let payload_data = if enable_encryption {
                                // Always use server_virtual_mac() — the key is
                                // negotiated with the server, not the RSU.
                                let dh_store_guard = dh_store
                                    .read()
                                    .expect("dh key store read lock poisoned");
                                let dh_key = dh_store_guard.get_key(server_virtual_mac());
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

    async fn handle_msg(&self, msg: &Message<'_>) -> Result<Option<Vec<ReplyType>>> {
        match msg.get_packet_type() {
            PacketType::Data(Data::Upstream(buf)) => {
                let routing = self
                    .routing
                    .read()
                    .expect("routing table read lock poisoned during upstream data handling");
                let Some(upstream) = routing.get_route_to(None) else {
                    return Ok(None);
                };

                let mut wire = Vec::with_capacity(24 + buf.data().len());
                Message::serialize_upstream_forward_into(
                    buf,
                    self.device.mac_address(),
                    upstream.mac,
                    &mut wire,
                );
                Ok(Some(vec![ReplyType::WireFlat(wire)]))
            }
            PacketType::Data(Data::Downstream(buf)) => {
                let destination: [u8; 6] = buf
                    .destination()
                    .get(0..6)
                    .ok_or_else(|| anyhow!("error"))?
                    .try_into()?;
                let destination: MacAddress = destination.into();
                let is_for_us =
                    destination == self.device.mac_address() || destination.bytes()[0] & 0x1 != 0;
                if is_for_us {
                    let payload_data = if self.args.obu_params.enable_encryption {
                        // Always use server_virtual_mac() — the key is
                        // negotiated with the server, not the sender RSU.
                        let dh_key_store = self
                            .dh_key_store
                            .read()
                            .expect("dh key store read lock poisoned");
                        let dh_key = dh_key_store.get_key(server_virtual_mac());
                        let cipher = self.crypto_config.cipher;
                        let Some(key) = dh_key else {
                            tracing::debug!(
                                size = buf.data().len(),
                                cipher = %cipher,
                                "No DH session established with server, dropping downstream frame"
                            );
                            return Ok(None);
                        };
                        match node_lib::crypto::decrypt_with_config(cipher, buf.data(), key) {
                            Ok(decrypted_data) => decrypted_data,
                            Err(e) => {
                                tracing::warn!(
                                    size = buf.data().len(),
                                    cipher = %cipher,
                                    error = %e,
                                    "Failed to decrypt downstream frame, dropping"
                                );
                                return Ok(None);
                            }
                        }
                    } else {
                        buf.data().to_vec()
                    };
                    return Ok(Some(vec![ReplyType::TapFlat(payload_data)]));
                }
                let target = destination;
                let routing = self
                    .routing
                    .read()
                    .expect("routing table read lock poisoned during downstream forwarding");
                Ok(Some({
                    let Some(next_hop) = routing.get_route_to(Some(target)) else {
                        return Ok(None);
                    };

                    let mut wire = Vec::with_capacity(30 + buf.data().len());
                    Message::serialize_downstream_forward_into(
                        buf,
                        self.device.mac_address(),
                        next_hop.mac,
                        &mut wire,
                    );
                    vec![ReplyType::WireFlat(wire)]
                }))
            }
            PacketType::Control(Control::Heartbeat(_)) => self
                .routing
                .write()
                .expect("routing table write lock poisoned during heartbeat handling")
                .handle_heartbeat(msg, self.device.mac_address()),
            PacketType::Control(Control::HeartbeatReply(_)) => self
                .routing
                .write()
                .expect("routing table write lock poisoned during heartbeat reply handling")
                .handle_heartbeat_reply(msg, self.device.mac_address()),
            PacketType::Control(Control::KeyExchangeInit(ke_init)) => {
                self.handle_key_exchange_init(ke_init, msg)
            }
            PacketType::Control(Control::KeyExchangeReply(ke_reply)) => {
                self.handle_key_exchange_reply(ke_reply, msg)
            }
            PacketType::Control(Control::SessionTerminated(st)) => {
                self.handle_session_terminated(st)
            }
        }
    }

    fn handle_key_exchange_init(
        &self,
        ke_init: &KeyExchangeInit<'_>,
        _msg: &Message<'_>,
    ) -> Result<Option<Vec<ReplyType>>> {
        // OBUs forward KeyExchangeInit up the tree toward the server.
        let routing = self
            .routing
            .read()
            .expect("routing table read lock poisoned");
        let Some(upstream) = routing.get_route_to(None) else {
            tracing::debug!("No upstream route, dropping KeyExchangeInit");
            return Ok(None);
        };

        // Preserve all fields (algorithm, key material, signature) when forwarding.
        let init = ke_init.clone_into_owned();
        let fwd = Message::new(
            self.device.mac_address(),
            upstream.mac,
            PacketType::Control(Control::KeyExchangeInit(init)),
        );
        let wire: Vec<u8> = (&fwd).into();
        tracing::debug!(
            obu = %ke_init.sender(),
            via = %upstream.mac,
            signed = ke_init.is_signed(),
            "Forwarding KeyExchangeInit up the tree"
        );
        Ok(Some(vec![ReplyType::WireFlat(wire)]))
    }

    fn handle_session_terminated(
        &self,
        st: &SessionTerminated<'_>,
    ) -> Result<Option<Vec<ReplyType>>> {
        let target = st.target();
        let our_mac = self.device.mac_address();

        if target != our_mac {
            // Not for us — forward toward the target OBU.
            let routing = self
                .routing
                .read()
                .expect("routing table read lock poisoned");
            let Some(next_hop) = routing.get_route_to(Some(target)) else {
                tracing::debug!(
                    target = %target,
                    "No route to forward SessionTerminated, dropping"
                );
                return Ok(None);
            };
            let owned = st.clone_into_owned();
            let fwd = Message::new(
                our_mac,
                next_hop.mac,
                PacketType::Control(Control::SessionTerminated(owned)),
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
        //      Unsigned messages are dropped.
        //   2. Verify the signature over [0x04][TARGET_MAC 6B][TIMESTAMP 8B][NONCE 8B].
        //   3. Check timestamp is within the validity window.
        //   4. Check the nonce has not been seen within the window (replay prevention).
        //      The cache is time-bounded: entries older than VALIDITY_SECS are pruned
        //      on each check, so it never accumulates stale nonces that could be replayed
        //      after a count-bounded window wraps.
        use node_lib::messages::control::session_terminated::{
            CLOCK_SKEW_TOLERANCE_SECS, VALIDITY_SECS,
        };
        if let Some(ref expected_hex) = self.args.obu_params.server_signing_pubkey {
            let (ts, nonce, algo_id, sig) = match (
                st.timestamp_secs(),
                st.nonce(),
                st.sig_algo_id(),
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

            // Timestamp check: reject messages outside the validity window.
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

            let algo = match node_lib::crypto::sig_algo_from_id(algo_id) {
                Some(a) => a,
                None => {
                    tracing::warn!(
                        algo_id,
                        "SessionTerminated uses unknown signature algorithm — dropping"
                    );
                    return Ok(None);
                }
            };

            let mut payload = vec![0x04u8];
            payload.extend_from_slice(&our_mac.bytes());
            payload.extend_from_slice(&ts.to_be_bytes());
            payload.extend_from_slice(nonce);

            if let Err(e) =
                node_lib::crypto::verify_dh_signature(algo, &payload, &expected_bytes, sig)
            {
                tracing::warn!(error = %e, "SessionTerminated has invalid signature — dropping");
                return Ok(None);
            }

            // Signature valid — check nonce freshness using a time-bounded cache.
            let validity = std::time::Duration::from_secs(VALIDITY_SECS);
            {
                let mut cache = self
                    .seen_revocation_nonces
                    .lock()
                    .expect("revocation nonce cache lock poisoned");
                // Prune expired entries first.
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

    fn handle_key_exchange_reply(
        &self,
        ke_reply: &KeyExchangeReply<'_>,
        _msg: &Message<'_>,
    ) -> Result<Option<Vec<ReplyType>>> {
        if !self.args.obu_params.enable_encryption {
            return Ok(None);
        }

        // The sender field carries the final destination OBU MAC (set by the server).
        // Use it — rather than msg.to() — to decide whether to consume or relay:
        // msg.to() is always the VANET next-hop (this node) due to per-hop unicast
        // delivery enforced by the channel MAC filter, so it cannot distinguish
        // "the reply is for me" from "the reply arrived here via me as a relay hop".
        let dest = ke_reply.sender();
        if dest != self.device.mac_address() {
            // Not for us — forward down the tree toward the target OBU.
            let routing = self
                .routing
                .read()
                .expect("routing table read lock poisoned");
            let Some(next_hop) = routing.get_route_to(Some(dest)) else {
                tracing::debug!(
                    dest = %dest,
                    "No route to forward KeyExchangeReply, dropping"
                );
                return Ok(None);
            };

            // Preserve all fields (algorithm, key material, signature) when forwarding.
            let reply = ke_reply.clone_into_owned();
            let fwd = Message::new(
                self.device.mac_address(),
                next_hop.mac,
                PacketType::Control(Control::KeyExchangeReply(reply)),
            );
            let wire: Vec<u8> = (&fwd).into();
            tracing::debug!(
                dest = %dest,
                via = %next_hop.mac,
                signed = ke_reply.is_signed(),
                "Forwarding KeyExchangeReply down the tree"
            );
            return Ok(Some(vec![ReplyType::WireFlat(wire)]));
        }

        // It's for us — verify the server's signature before completing the exchange.
        if self.args.obu_params.enable_dh_signatures {
            match (
                ke_reply.sig_algo_id(),
                ke_reply.signing_pubkey(),
                ke_reply.signature(),
            ) {
                (Some(sig_algo), Some(spk), Some(sig)) => {
                    let algo = match sig_algo_from_id(sig_algo) {
                        Some(a) => a,
                        None => {
                            tracing::warn!(
                                sig_algo_id = sig_algo,
                                key_id = ke_reply.key_id(),
                                "KeyExchangeReply uses unknown signature algorithm, dropping"
                            );
                            return Ok(None);
                        }
                    };
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

        // PKI: if a pinned server public key is configured, reject replies
        // from any server whose signing key doesn't match.
        // Signature verification must be enabled; without it the key bytes are
        // unverified and an attacker could include the pinned key in a forged reply.
        if self.args.obu_params.server_signing_pubkey.is_some()
            && !self.args.obu_params.enable_dh_signatures
        {
            tracing::warn!(
                key_id = ke_reply.key_id(),
                "server_signing_pubkey is configured but enable_dh_signatures is false; \
                 dropping KeyExchangeReply to prevent key-pinning bypass"
            );
            return Ok(None);
        }
        if let Some(ref expected_hex) = self.args.obu_params.server_signing_pubkey {
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
        // Rejecting mismatches prevents an attacker from downgrading the algorithm
        // by rewriting the algo_id byte in a relayed reply.
        {
            use node_lib::messages::control::key_exchange::{KE_ALGO_ML_KEM_768, KE_ALGO_X25519};
            let expected_algo_id = match self.crypto_config.dh_group {
                node_lib::crypto::DhGroup::X25519 => KE_ALGO_X25519,
                node_lib::crypto::DhGroup::MlKem768 => KE_ALGO_ML_KEM_768,
            };
            if ke_reply.algo_id() != expected_algo_id {
                tracing::warn!(
                    key_id = ke_reply.key_id(),
                    expected = expected_algo_id,
                    received = ke_reply.algo_id(),
                    "KeyExchangeReply algo_id does not match initiated algorithm, dropping"
                );
                return Ok(None);
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

#[cfg(test)]
pub(crate) fn handle_msg_for_test(
    routing: Shared<Routing>,
    device_mac: mac_address::MacAddress,
    msg: &node_lib::messages::message::Message<'_>,
) -> anyhow::Result<Option<Vec<ReplyType>>> {
    use node_lib::messages::{control::Control, data::Data, packet_type::PacketType};

    match msg.get_packet_type() {
        PacketType::Data(Data::Upstream(buf)) => {
            let routing = routing
                .read()
                .expect("routing table read lock poisoned in test helper");
            let Some(upstream) = routing.get_route_to(None) else {
                return Ok(None);
            };

            let wire: Vec<u8> = (&node_lib::messages::message::Message::new(
                device_mac,
                upstream.mac,
                PacketType::Data(Data::Upstream(buf.clone())),
            ))
                .into();
            Ok(Some(vec![ReplyType::WireFlat(wire)]))
        }
        PacketType::Data(Data::Downstream(buf)) => {
            let destination: [u8; 6] = buf
                .destination()
                .get(0..6)
                .ok_or_else(|| anyhow::anyhow!("error"))?
                .try_into()?;
            let destination: mac_address::MacAddress = destination.into();
            if destination == device_mac {
                return Ok(Some(vec![ReplyType::TapFlat(buf.data().to_vec())]));
            }

            let target = destination;
            let routing = routing
                .read()
                .expect("routing table read lock poisoned in test helper");
            Ok(Some({
                let Some(next_hop) = routing.get_route_to(Some(target)) else {
                    return Ok(None);
                };

                let wire: Vec<u8> = (&node_lib::messages::message::Message::new(
                    device_mac,
                    next_hop.mac,
                    PacketType::Data(Data::Downstream(buf.clone())),
                ))
                    .into();
                vec![ReplyType::WireFlat(wire)]
            }))
        }
        PacketType::Control(Control::Heartbeat(_)) => routing
            .write()
            .expect("routing table write lock poisoned in test helper")
            .handle_heartbeat(msg, device_mac),
        PacketType::Control(Control::HeartbeatReply(_)) => routing
            .write()
            .expect("routing table write lock poisoned in test helper")
            .handle_heartbeat_reply(msg, device_mac),
        PacketType::Control(Control::KeyExchangeInit(_))
        | PacketType::Control(Control::KeyExchangeReply(_))
        | PacketType::Control(Control::SessionTerminated(_)) => {
            // Key exchange and session messages not handled in basic test helper
            Ok(None)
        }
    }
}

#[cfg(test)]
mod obu_tests {
    use super::{handle_msg_for_test, routing::Routing};
    use mac_address::MacAddress;
    use node_lib::messages::{
        control::heartbeat::Heartbeat,
        control::Control,
        data::{Data, ToDownstream, ToUpstream},
        message::Message,
        packet_type::PacketType,
    };
    use tokio::time::Instant;

    #[test]
    fn upstream_with_no_cached_upstream_returns_none() {
        let args = crate::test_helpers::mk_test_obu_args();
        let boot = Instant::now();
        let routing = std::sync::Arc::new(std::sync::RwLock::new(
            Routing::new(&args, &boot).expect("routing"),
        ));

        let from: MacAddress = [2u8; 6].into();
        let to: MacAddress = [3u8; 6].into();
        let payload = b"p";
        let tu = ToUpstream::new(from, payload);
        let msg = Message::new(from, to, PacketType::Data(Data::Upstream(tu)));

        let res = handle_msg_for_test(routing, [9u8; 6].into(), &msg).expect("ok");
        assert!(res.is_none());
    }

    #[test]
    fn downstream_to_self_returns_tap() {
        let args = crate::test_helpers::mk_test_obu_args();
        let boot = Instant::now();
        let routing = std::sync::Arc::new(std::sync::RwLock::new(
            Routing::new(&args, &boot).expect("routing"),
        ));

        let src = [4u8; 6];
        let dest_mac: MacAddress = [10u8; 6].into();
        let payload = b"abc";
        let td = ToDownstream::new(&src, dest_mac, payload);
        let msg = Message::new(
            dest_mac,
            [255u8; 6].into(),
            PacketType::Data(Data::Downstream(td)),
        );

        let res = handle_msg_for_test(routing, dest_mac, &msg).expect("ok");
        assert!(res.is_some());
        let v = res.unwrap();
        assert_eq!(v.len(), 1);
        match &v[0] {
            super::ReplyType::TapFlat(_) => {}
            _ => panic!("expected TapFlat"),
        }
    }

    #[test]
    fn upstream_with_cached_upstream_returns_wire() {
        let args = crate::test_helpers::mk_test_obu_args();
        let boot = Instant::now();
        let routing = std::sync::Arc::new(std::sync::RwLock::new(
            Routing::new(&args, &boot).expect("routing"),
        ));

        let hb_source: MacAddress = [7u8; 6].into();
        let pkt_from: MacAddress = [8u8; 6].into();
        let our_mac: MacAddress = [9u8; 6].into();
        let hb = Heartbeat::new(std::time::Duration::from_millis(1), 1u32, hb_source);
        let hb_msg = Message::new(
            pkt_from,
            [255u8; 6].into(),
            PacketType::Control(Control::Heartbeat(hb.clone())),
        );
        let _ = routing
            .write()
            .unwrap()
            .handle_heartbeat(&hb_msg, our_mac)
            .expect("handled hb");

        let _ = routing.read().unwrap().select_and_cache_upstream(hb_source);

        let from: MacAddress = [3u8; 6].into();
        let to: MacAddress = [4u8; 6].into();
        let payload = b"x";
        let tu = ToUpstream::new(from, payload);
        let msg = Message::new(from, to, PacketType::Data(Data::Upstream(tu)));

        let res = handle_msg_for_test(routing.clone(), our_mac, &msg).expect("ok");
        assert!(res.is_some());
        let v = res.unwrap();
        assert!(v.iter().any(|x| matches!(x, super::ReplyType::WireFlat(_))));
    }

    #[test]
    fn heartbeat_generates_forward_and_reply() {
        let args = crate::test_helpers::mk_test_obu_args();
        let boot = Instant::now();
        let routing = std::sync::Arc::new(std::sync::RwLock::new(
            Routing::new(&args, &boot).expect("routing"),
        ));

        let hb_source: MacAddress = [11u8; 6].into();
        let pkt_from: MacAddress = [12u8; 6].into();
        let our_mac: MacAddress = [13u8; 6].into();

        let hb = Heartbeat::new(std::time::Duration::from_millis(1), 2u32, hb_source);
        let msg = Message::new(
            pkt_from,
            [255u8; 6].into(),
            PacketType::Control(Control::Heartbeat(hb.clone())),
        );

        let res = handle_msg_for_test(routing.clone(), our_mac, &msg).expect("ok");
        assert!(res.is_some());
        let v = res.unwrap();
        assert!(v.len() >= 2);
        assert!(v.iter().all(|x| matches!(x, super::ReplyType::WireFlat(_))));
    }

    #[test]
    fn heartbeat_reply_updates_routing_and_replies() {
        let args = crate::test_helpers::mk_test_obu_args();
        let boot = Instant::now();
        let routing = std::sync::Arc::new(std::sync::RwLock::new(
            Routing::new(&args, &boot).expect("routing"),
        ));

        let hb_source: MacAddress = [21u8; 6].into();
        let pkt_from: MacAddress = [22u8; 6].into();
        let our_mac: MacAddress = [23u8; 6].into();

        let hb = Heartbeat::new(std::time::Duration::from_millis(1), 7u32, hb_source);
        let initial = Message::new(
            pkt_from,
            [255u8; 6].into(),
            PacketType::Control(Control::Heartbeat(hb.clone())),
        );
        let _ = routing
            .write()
            .unwrap()
            .handle_heartbeat(&initial, our_mac)
            .expect("handled");

        let reply_sender: MacAddress = [42u8; 6].into();
        let hbr =
            node_lib::messages::control::heartbeat::HeartbeatReply::from_sender(&hb, reply_sender);
        let reply_from: MacAddress = [55u8; 6].into();
        let reply_msg = Message::new(
            reply_from,
            [255u8; 6].into(),
            PacketType::Control(Control::HeartbeatReply(hbr.clone())),
        );

        let res = handle_msg_for_test(routing.clone(), our_mac, &reply_msg).expect("ok");
        assert!(res.is_some());
        let out = res.unwrap();
        assert!(!out.is_empty());
    }

    #[test]
    fn downstream_to_other_forwards_wire_when_route_exists() {
        let args = crate::test_helpers::mk_test_obu_args();
        let boot = Instant::now();
        let routing = std::sync::Arc::new(std::sync::RwLock::new(
            Routing::new(&args, &boot).expect("routing"),
        ));

        let hb_source: MacAddress = [77u8; 6].into();
        let pkt_from: MacAddress = [78u8; 6].into();
        let our_mac: MacAddress = [79u8; 6].into();
        let hb = Heartbeat::new(std::time::Duration::from_millis(1), 1u32, hb_source);
        let hb_msg = Message::new(
            pkt_from,
            [255u8; 6].into(),
            PacketType::Control(Control::Heartbeat(hb.clone())),
        );
        let _ = routing
            .write()
            .unwrap()
            .handle_heartbeat(&hb_msg, our_mac)
            .expect("handled hb");

        let _ = routing.read().unwrap().select_and_cache_upstream(hb_source);

        let src = [3u8; 6];
        let target_mac: MacAddress = hb_source;
        let payload = b"ok";
        let td = ToDownstream::new(&src, target_mac, payload);
        let msg = Message::new(
            target_mac,
            [255u8; 6].into(),
            PacketType::Data(Data::Downstream(td)),
        );

        let res = handle_msg_for_test(routing.clone(), our_mac, &msg).expect("ok");
        assert!(res.is_some());
        let v = res.unwrap();
        assert!(v.iter().any(|x| matches!(x, super::ReplyType::WireFlat(_))));
    }

    #[test]
    fn downstream_to_other_returns_none_when_no_route() {
        let args = crate::test_helpers::mk_test_obu_args();
        let boot = Instant::now();
        let routing = std::sync::Arc::new(std::sync::RwLock::new(
            Routing::new(&args, &boot).expect("routing"),
        ));

        let our_mac: MacAddress = [90u8; 6].into();
        let target_mac: MacAddress = [91u8; 6].into();
        let src = [5u8; 6];
        let payload = b"nope";
        let td = ToDownstream::new(&src, target_mac, payload);
        let msg = Message::new(
            target_mac,
            [255u8; 6].into(),
            PacketType::Data(Data::Downstream(td)),
        );

        let res = handle_msg_for_test(routing.clone(), our_mac, &msg).expect("ok");
        assert!(res.is_none());
    }
}
