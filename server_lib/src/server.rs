use crate::cloud_protocol::{
    CloudMessage, DownstreamForward, KeyExchangeResponse, SessionTerminatedForward, UpstreamForward,
};
use crate::registry::RegistrationMessage;
use anyhow::Result;
use common::tun::Tun;
use mac_address::MacAddress;
use node_lib::crypto::{CryptoConfig, DhKeypair, SigningAlgorithm, SigningKeypair};
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::{OnceCell, RwLock};
use tokio::time::Instant;
use tracing::Instrument;

/// Shared reference to a Tun device.
pub type SharedTun = Arc<Tun>;

/// An established DH-derived key for a specific OBU.
#[derive(Debug, Clone)]
struct ObuKey {
    /// The derived symmetric key.
    key: Arc<[u8]>,
    /// Key ID from the exchange (used for logging/diagnostics).
    #[allow(dead_code)]
    key_id: u32,
    /// When the key was established.
    established_at: Instant,
}

impl ObuKey {
    /// Check if this key has expired given a TTL in milliseconds.
    fn is_expired(&self, ttl_ms: u64) -> bool {
        self.established_at.elapsed() > std::time::Duration::from_millis(ttl_ms)
    }
}

/// Per-OBU DH key store on the server side, keyed by OBU VANET MAC.
type DhKeyStore = HashMap<MacAddress, ObuKey>;

/// Routing entry for an OBU, keyed by virtual TAP MAC.
#[derive(Debug, Clone, Copy)]
struct ObuRoute {
    /// OBU's VANET MAC (used in DownstreamForward.obu_dest_mac for RSU routing).
    vanet_mac: MacAddress,
    /// Socket address of the RSU that forwards to this OBU.
    rsu_addr: SocketAddr,
}

/// Shared server state passed to async tasks.
///
/// All fields are either `Copy` or `Arc`-wrapped, so `.clone()` is O(1).
#[derive(Clone)]
struct ServerCtx {
    socket: Arc<UdpSocket>,
    registry: Arc<RwLock<HashMap<MacAddress, Vec<MacAddress>>>>,
    obu_routes: Arc<RwLock<HashMap<MacAddress, ObuRoute>>>,
    tun: Option<SharedTun>,
    enable_encryption: bool,
    enable_dh_signatures: bool,
    signing_keypair: Option<Arc<SigningKeypair>>,
    dh_signing_allowlist: Arc<RwLock<HashMap<MacAddress, Vec<u8>>>>,
    dh_keys: Arc<RwLock<DhKeyStore>>,
    crypto_config: CryptoConfig,
    key_ttl_ms: u64,
}

/// ServerNode receives traffic from RSU nodes over UDP via the cloud interface.
///
/// The Server owns the TAP device and handles all encryption/decryption of OBU
/// traffic. RSUs are transparent relays: they forward upstream data from OBUs
/// to the Server (as `UpstreamForward`), and the Server sends downstream data
/// back through the appropriate RSU (as `DownstreamForward`).
///
/// The server maintains:
/// - A registry of RSU MAC → associated OBU MACs (from `RegistrationMessage`)
/// - An OBU routing table: virtual TAP MAC → (VANET MAC, RSU addr)
///   learned from upstream traffic
#[derive(Clone)]
pub struct Server {
    /// IP address for the UDP server (cloud interface IP).
    ip: Ipv4Addr,
    /// UDP port to listen on.
    port: u16,
    /// UDP socket for receiving traffic from RSUs. Set once in `start()`.
    socket: Arc<OnceCell<Arc<UdpSocket>>>,
    /// Registry: RSU VANET MAC → list of associated OBU MACs.
    registry: Arc<RwLock<HashMap<MacAddress, Vec<MacAddress>>>>,
    /// OBU routing table: virtual TAP MAC → (VANET MAC, RSU addr).
    /// Keyed by virtual TAP MAC so that we can look up downstream destinations
    /// using the dest MAC from Ethernet frames read off the server's TAP.
    obu_routes: Arc<RwLock<HashMap<MacAddress, ObuRoute>>>,
    /// Optional TAP device for decapsulated traffic.
    tun: Option<SharedTun>,
    /// Whether encryption is enabled for OBU traffic (implies DH key exchange).
    enable_encryption: bool,
    /// Maximum lifetime for per-OBU DH keys in milliseconds.
    key_ttl_ms: u64,
    /// Per-OBU DH-derived keys, keyed by OBU VANET MAC.
    dh_keys: Arc<RwLock<DhKeyStore>>,
    /// Crypto configuration for key derivation.
    crypto_config: CryptoConfig,
    /// Node name for tracing/logging identification.
    node_name: String,
    /// Whether to sign DH replies and verify incoming DH message signatures.
    enable_dh_signatures: bool,
    /// Identity keypair for signing DH replies using the configured signing algorithm
    /// (present when `enable_dh_signatures`).
    signing_keypair: Option<Arc<SigningKeypair>>,
    /// PKI allowlist: OBU VANET MAC → expected verifying key bytes (Ed25519: 32B, ML-DSA-65: 1952B).
    /// When non-empty and enable_dh_signatures is set, only OBUs whose signing key
    /// matches the registered entry are allowed to complete key exchange.
    /// Wrapped in Arc<RwLock> so the allowlist can be hot-reloaded at runtime via
    /// `reload_allowlist()` without restarting the server.
    dh_signing_allowlist: Arc<RwLock<HashMap<MacAddress, Vec<u8>>>>,
    /// Signing algorithm used for DH message signing (default: Ed25519).
    signing_algorithm: SigningAlgorithm,
}

/// Parse the destination and source MAC addresses from an Ethernet frame.
/// Returns `None` if the frame is shorter than 12 bytes.
fn parse_eth_addrs(frame: &[u8]) -> Option<(MacAddress, MacAddress)> {
    if frame.len() < 12 {
        return None;
    }
    let dest: [u8; 6] = frame[..6].try_into().expect("length checked");
    let src: [u8; 6] = frame[6..12].try_into().expect("length checked");
    Some((MacAddress::new(dest), MacAddress::new(src)))
}

impl Server {
    /// Create a new Server that will listen on the specified IP and port.
    /// Note: The server does not start listening until `start()` is called.
    pub fn new(ip: Ipv4Addr, port: u16, node_name: String) -> Self {
        Self {
            ip,
            port,
            socket: Arc::new(OnceCell::new()),
            registry: Arc::new(RwLock::new(HashMap::new())),
            obu_routes: Arc::new(RwLock::new(HashMap::new())),
            tun: None,
            enable_encryption: false,
            key_ttl_ms: 86_400_000,
            dh_keys: Arc::new(RwLock::new(HashMap::new())),
            crypto_config: CryptoConfig::default(),
            node_name,
            enable_dh_signatures: false,
            signing_keypair: None,
            dh_signing_allowlist: Arc::new(RwLock::new(HashMap::new())),
            signing_algorithm: SigningAlgorithm::default(),
        }
    }

    /// Set the TAP device for decapsulated traffic.
    pub fn with_tun(mut self, tun: SharedTun) -> Self {
        self.tun = Some(tun);
        self
    }

    /// Enable or disable encryption for OBU traffic.
    pub fn with_encryption(mut self, enable: bool) -> Self {
        self.enable_encryption = enable;
        self
    }

    /// Set the key TTL in milliseconds (default: 86400000 — 24h).
    pub fn with_key_ttl_ms(mut self, ms: u64) -> Self {
        self.key_ttl_ms = ms;
        self
    }

    /// Set the crypto configuration for key derivation.
    pub fn with_crypto_config(mut self, config: CryptoConfig) -> Self {
        // Also sync signing_algorithm so that with_dh_signatures() called afterward
        // generates a keypair with the correct algorithm without requiring an extra
        // with_signing_algorithm() call.
        self.signing_algorithm = config.signing_algorithm;
        self.crypto_config = config;
        self
    }

    /// Set the signing algorithm to use for DH message signing (default: Ed25519).
    /// This is automatically set by `with_crypto_config`; call this only when
    /// overriding the algorithm independently of the full crypto config.
    pub fn with_signing_algorithm(mut self, algo: SigningAlgorithm) -> Self {
        self.signing_algorithm = algo;
        self
    }

    /// Enable or disable DH message signing and verification.
    /// Generates a random ephemeral keypair when enabled.
    /// Call `with_signing_key_seed` afterwards to use a stable keypair instead.
    pub fn with_dh_signatures(mut self, enabled: bool) -> Self {
        self.enable_dh_signatures = enabled;
        if enabled {
            self.signing_keypair = Some(Arc::new(SigningKeypair::generate(self.signing_algorithm)));
        } else {
            self.signing_keypair = None;
        }
        self
    }

    /// Load the signing keypair from a 32-byte hex-encoded seed instead of generating
    /// a random one. The signing algorithm is taken from `self.signing_algorithm`
    /// (set via `with_crypto_config` or `with_signing_algorithm`).
    /// The same `(algorithm, seed)` pair always produces the same keypair, giving the
    /// server a stable identity across restarts so OBUs can pin it via
    /// `server_signing_pubkey`.
    pub fn with_signing_key_seed(mut self, hex_seed: &str) -> anyhow::Result<Self> {
        let seed = node_lib::crypto::decode_hex_32(hex_seed)
            .ok_or_else(|| anyhow::anyhow!("signing_key_seed must be exactly 64 hex characters"))?;
        let kp = SigningKeypair::from_seed(self.signing_algorithm, seed);
        // Sanity-check: the derived keypair's algorithm must match self.signing_algorithm.
        // If this fires, with_signing_algorithm() was called after with_signing_key_seed(),
        // which would silently deploy the wrong keypair.
        debug_assert_eq!(
            kp.signing_algorithm(),
            self.signing_algorithm,
            "signing keypair algorithm mismatch — call with_signing_algorithm before with_signing_key_seed"
        );
        self.signing_keypair = Some(Arc::new(kp));
        Ok(self)
    }

    /// Set the PKI allowlist mapping OBU VANET MAC → expected verifying key bytes.
    /// When non-empty and DH signatures are enabled, only allowlisted OBUs may complete
    /// key exchange (closes the TOFU first-contact impersonation gap).
    pub fn with_dh_signing_allowlist(mut self, allowlist: HashMap<MacAddress, Vec<u8>>) -> Self {
        self.dh_signing_allowlist = Arc::new(RwLock::new(allowlist));
        self
    }

    pub async fn start(&self) -> Result<()> {
        let bind_addr = format!("{}:{}", self.ip, self.port);
        let node_name = self.node_name.clone();

        let _span = tracing::info_span!("node", name = %node_name).entered();

        tracing::info!(
            ip = %self.ip,
            port = self.port,
            bind_addr = %bind_addr,
            "Starting server UDP listener"
        );

        if !self.enable_encryption {
            tracing::warn!(
                "Encryption is DISABLED — all downstream traffic is sent in the clear. \
                 Set enable_encryption = true to protect data payloads."
            );
        }
        if !self.enable_dh_signatures {
            tracing::warn!(
                "DH signatures are DISABLED — key exchange messages are not authenticated. \
                 Set enable_dh_signatures = true to prevent MITM key substitution."
            );
        }
        if let Some(ref kp) = self.signing_keypair {
            let pubkey_hex: String = kp
                .verifying_key_bytes()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            let pki_entries = self.dh_signing_allowlist.read().await.len();
            tracing::info!(
                signing_pubkey = %pubkey_hex,
                pki_entries,
                "DH signing enabled on server"
            );
        }

        let socket = Arc::new(UdpSocket::bind(&bind_addr).await?);
        // Ignore error: if called twice, the first socket wins.
        let _ = self.socket.set(socket.clone());

        let ctx = ServerCtx {
            socket,
            registry: self.registry.clone(),
            obu_routes: self.obu_routes.clone(),
            tun: self.tun.clone(),
            enable_encryption: self.enable_encryption,
            enable_dh_signatures: self.enable_dh_signatures,
            signing_keypair: self.signing_keypair.clone(),
            dh_signing_allowlist: self.dh_signing_allowlist.clone(),
            dh_keys: self.dh_keys.clone(),
            crypto_config: self.crypto_config,
            key_ttl_ms: self.key_ttl_ms,
        };

        // Spawn cloud recv task (handles registration + upstream forwarding + key exchange)
        let recv_span = tracing::info_span!("node", name = %node_name);
        let ctx_recv = ctx.clone();
        tokio::spawn(async move { Self::cloud_recv_loop(ctx_recv).await }.instrument(recv_span));

        // Spawn TAP read task if a TUN device is available
        if ctx.tun.is_some() {
            let tap_span = tracing::info_span!("node", name = %node_name);
            let ctx_tap = ctx.clone();
            tokio::spawn(async move { Self::tap_read_loop(ctx_tap).await }.instrument(tap_span));
        }

        Ok(())
    }

    /// Main cloud receive loop: handles Registration, UpstreamForward, KeyExchangeForward.
    async fn cloud_recv_loop(ctx: ServerCtx) {
        let mut buf = vec![0u8; 65536];
        loop {
            match ctx.socket.recv_from(&mut buf).await {
                Ok((len, src_addr)) => {
                    let data = &buf[..len];
                    match CloudMessage::try_from_bytes(data) {
                        Some(CloudMessage::Registration(msg)) => {
                            Self::handle_registration(&ctx.registry, &msg, src_addr).await;
                        }
                        Some(CloudMessage::UpstreamForward(fwd)) => {
                            Self::handle_upstream(&fwd, src_addr, &ctx).await;
                        }
                        Some(CloudMessage::KeyExchangeForward(ke_fwd)) => {
                            if ctx.enable_encryption {
                                Self::handle_key_exchange_forward(&ke_fwd, src_addr, &ctx).await;
                            } else {
                                tracing::warn!(
                                    src = %src_addr,
                                    "Ignoring KeyExchangeForward — encryption is disabled"
                                );
                            }
                        }
                        Some(CloudMessage::SessionTerminatedForward(_)) => {
                            // Server never receives its own session-termination messages;
                            // this type only flows server → RSU.
                            tracing::warn!(
                                src = %src_addr,
                                "Received unexpected SessionTerminatedForward on server"
                            );
                        }
                        Some(CloudMessage::DownstreamForward(_))
                        | Some(CloudMessage::KeyExchangeResponse(_)) => {
                            tracing::warn!(
                                src = %src_addr,
                                "Received unexpected downstream/response message on server"
                            );
                        }
                        None => {
                            tracing::debug!(
                                src = %src_addr,
                                len = len,
                                "Received unrecognised UDP packet"
                            );
                        }
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "Error receiving UDP packet");
                }
            }
        }
    }

    async fn handle_registration(
        registry: &Arc<RwLock<HashMap<MacAddress, Vec<MacAddress>>>>,
        msg: &RegistrationMessage,
        src_addr: SocketAddr,
    ) {
        registry
            .write()
            .await
            .insert(msg.rsu_mac, msg.obu_macs.clone());
        tracing::info!(
            rsu = %msg.rsu_mac,
            obu_count = msg.obu_macs.len(),
            from = %src_addr,
            "RSU registration received"
        );
    }

    /// Handle upstream data from an OBU via RSU.
    ///
    /// The payload inside `UpstreamForward` is the raw encrypted/unencrypted TAP
    /// frame data (as produced by `ToUpstream.data()` on the RSU side).
    /// There is NO origin-MAC prefix — the OBU source VANET MAC is carried
    /// separately in `fwd.obu_source_mac`.
    ///
    /// After decryption (if enabled), the result is the original Ethernet frame
    /// that was read from the OBU's virtual TAP. We extract the source MAC from
    /// that frame to learn the OBU's virtual TAP MAC, then apply L2 switch logic:
    ///
    /// - **Multicast/broadcast** (group bit set): write to server TAP **and** fan-out
    ///   to all known OBUs via `DownstreamForward`.
    /// - **Unicast to a known OBU** (dest MAC is in `obu_routes`): forward as
    ///   `DownstreamForward` directly to that OBU's RSU (do **not** write to server TAP).
    /// - **Unicast to unknown dest** (server-destined or unknown): write to server TAP.
    async fn handle_upstream(fwd: &UpstreamForward, src_addr: SocketAddr, ctx: &ServerCtx) {
        let socket = &ctx.socket;
        let obu_routes = &ctx.obu_routes;
        let dh_keys = &ctx.dh_keys;
        let tun = ctx.tun.as_ref();
        let enable_encryption = ctx.enable_encryption;
        let key_ttl_ms = ctx.key_ttl_ms;
        let crypto_config = ctx.crypto_config;

        // Decrypt the payload if encryption is enabled.
        let tap_frame = if enable_encryption {
            let key = {
                let store = dh_keys.read().await;
                store.get(&fwd.obu_source_mac).and_then(|k| {
                    if k.is_expired(key_ttl_ms) {
                        None
                    } else {
                        Some(k.key.clone())
                    }
                })
            };
            let Some(key) = key else {
                tracing::debug!(
                    obu = %fwd.obu_source_mac,
                    "No valid DH session for OBU (missing or expired), dropping upstream payload"
                );
                return;
            };
            match node_lib::crypto::decrypt_with_config(crypto_config.cipher, &fwd.payload, &key) {
                Ok(plaintext) => plaintext,
                Err(e) => {
                    tracing::error!(
                        obu = %fwd.obu_source_mac,
                        error = %e,
                        "Failed to decrypt upstream payload"
                    );
                    return;
                }
            }
        } else {
            fwd.payload.clone()
        };

        let Some((dest_mac, virtual_tap_mac)) = parse_eth_addrs(&tap_frame) else {
            // Too short to contain an Ethernet header — pass to TAP as-is.
            if let Some(tun) = tun {
                if let Err(e) = tun.send_all(&tap_frame).await {
                    tracing::error!(error = %e, "Failed to write short upstream frame to TAP");
                }
            }
            return;
        };

        // Learn the OBU's virtual TAP MAC from the Ethernet frame source.
        obu_routes.write().await.insert(
            virtual_tap_mac,
            ObuRoute {
                vanet_mac: fwd.obu_source_mac,
                rsu_addr: src_addr,
            },
        );
        tracing::trace!(
            virtual_tap_mac = %virtual_tap_mac,
            vanet_mac = %fwd.obu_source_mac,
            rsu = %src_addr,
            "Learned OBU route from upstream traffic"
        );

        let is_multicast = dest_mac.bytes()[0] & 0x01 != 0;

        if is_multicast {
            // Multicast/broadcast: deliver to server TAP and fan-out to all OBUs.
            if let Some(tun) = tun {
                if let Err(e) = tun.send_all(&tap_frame).await {
                    tracing::error!(error = %e, "Failed to write multicast frame to TAP");
                }
            }
            // Snapshot routes and encrypt payloads while holding locks,
            // then drop locks before awaiting on network I/O.
            let sends: Vec<_> = {
                let routes = obu_routes.read().await;
                let keys = dh_keys.read().await;
                routes
                    .iter()
                    .filter_map(|(_, route)| {
                        let payload = if enable_encryption {
                            Self::try_encrypt_for_obu(
                                &tap_frame,
                                route.vanet_mac,
                                &keys,
                                key_ttl_ms,
                                crypto_config,
                                "upstream multicast",
                            )?
                        } else {
                            tap_frame.clone()
                        };
                        let downstream = DownstreamForward::new(
                            route.vanet_mac,
                            MacAddress::new([0; 6]),
                            payload,
                        );
                        Some((downstream.to_bytes(), route.rsu_addr, route.vanet_mac))
                    })
                    .collect()
            };
            for (bytes, addr, vanet_mac) in &sends {
                if let Err(e) = socket.send_to(bytes, addr).await {
                    tracing::error!(
                        obu = %vanet_mac,
                        error = %e,
                        "Failed to send multicast downstream to OBU"
                    );
                }
            }
        } else {
            // Unicast: L2 switch — if dest is a known OBU, forward directly.
            let route = { obu_routes.read().await.get(&dest_mac).copied() };
            if let Some(route) = route {
                let payload = if enable_encryption {
                    let keys = dh_keys.read().await;
                    let Some(enc) = Self::try_encrypt_for_obu(
                        &tap_frame,
                        route.vanet_mac,
                        &keys,
                        key_ttl_ms,
                        crypto_config,
                        "upstream unicast L2-switch",
                    ) else {
                        return;
                    };
                    enc
                } else {
                    tap_frame.clone()
                };
                let downstream =
                    DownstreamForward::new(route.vanet_mac, MacAddress::new([0; 6]), payload);
                if let Err(e) = socket.send_to(&downstream.to_bytes(), route.rsu_addr).await {
                    tracing::error!(
                        dest = %dest_mac,
                        error = %e,
                        "Failed to L2-switch upstream frame to OBU via RSU"
                    );
                }
            } else {
                // Server-destined traffic: write to server TAP.
                let Some(tun) = tun else {
                    tracing::debug!(
                        obu = %fwd.obu_source_mac,
                        "Upstream received but no TAP device configured"
                    );
                    return;
                };
                if let Err(e) = tun.send_all(&tap_frame).await {
                    tracing::error!(error = %e, "Failed to write decrypted upstream frame to TAP");
                }
            }
        }
    }

    /// Read frames from TAP, encrypt, and send downstream to the appropriate RSU.
    async fn tap_read_loop(ctx: ServerCtx) {
        let tun = ctx
            .tun
            .as_ref()
            .expect("tap_read_loop spawned without a tun device");
        let socket = &ctx.socket;
        let obu_routes = &ctx.obu_routes;
        let dh_keys = &ctx.dh_keys;
        let enable_encryption = ctx.enable_encryption;
        let crypto_config = ctx.crypto_config;
        let key_ttl_ms = ctx.key_ttl_ms;

        let mut buf = vec![0u8; 65536];
        loop {
            let n = match tun.recv(&mut buf).await {
                Ok(n) => n,
                Err(e) => {
                    tracing::error!(error = %e, "Error reading from TAP device");
                    continue;
                }
            };

            let frame = &buf[..n];
            let Some((dest_mac, _src_mac)) = parse_eth_addrs(frame) else {
                continue; // Need at least an Ethernet header
            };

            // Broadcast (FF:FF:FF:FF:FF:FF) has the group bit set, so is_multicast
            // already covers it — no separate is_broadcast check needed.
            let is_multicast = dest_mac.bytes()[0] & 0x01 != 0;

            if is_multicast {
                // Snapshot routes and encrypt payloads while holding locks,
                // then drop locks before awaiting on network I/O.
                let sends: Vec<_> = {
                    let routes = obu_routes.read().await;
                    let keys = dh_keys.read().await;
                    routes
                        .iter()
                        .filter_map(|(&_tap_mac, route)| {
                            let payload = if enable_encryption {
                                Self::try_encrypt_for_obu(
                                    frame,
                                    route.vanet_mac,
                                    &keys,
                                    key_ttl_ms,
                                    crypto_config,
                                    "tap broadcast",
                                )?
                            } else {
                                frame.to_vec()
                            };
                            let fwd = DownstreamForward::new(
                                route.vanet_mac,
                                MacAddress::new([0; 6]),
                                payload,
                            );
                            Some((fwd.to_bytes(), route.rsu_addr, route.vanet_mac))
                        })
                        .collect()
                };
                for (bytes, addr, vanet_mac) in &sends {
                    if let Err(e) = socket.send_to(bytes, addr).await {
                        tracing::error!(
                            obu = %vanet_mac,
                            error = %e,
                            "Failed to send broadcast downstream to RSU"
                        );
                    }
                }
            } else {
                // Unicast: find the RSU for this OBU via its virtual TAP MAC
                let route = {
                    let routes = obu_routes.read().await;
                    routes.get(&dest_mac).copied()
                };

                if let Some(route) = route {
                    let payload_data = if enable_encryption {
                        let keys = dh_keys.read().await;
                        let Some(enc) = Self::try_encrypt_for_obu(
                            frame,
                            route.vanet_mac,
                            &keys,
                            key_ttl_ms,
                            crypto_config,
                            "tap unicast",
                        ) else {
                            continue;
                        };
                        enc
                    } else {
                        frame.to_vec()
                    };
                    let fwd = DownstreamForward::new(
                        route.vanet_mac,         // VANET MAC for RSU routing lookup
                        MacAddress::new([0; 6]), // server origin
                        payload_data,
                    );
                    if let Err(e) = socket.send_to(&fwd.to_bytes(), route.rsu_addr).await {
                        tracing::error!(
                            obu = %dest_mac,
                            error = %e,
                            "Failed to send downstream to RSU"
                        );
                    }
                } else {
                    tracing::debug!(
                        dest = %dest_mac,
                        "No route to OBU for downstream delivery"
                    );
                }
            }
        }
    }

    /// Encrypt a payload for a specific OBU using its DH session key.
    /// Returns `None` if no DH session is established for this OBU.
    fn encrypt_for_obu(
        plaintext: &[u8],
        obu_vanet_mac: MacAddress,
        dh_keys: &DhKeyStore,
        key_ttl_ms: u64,
        crypto_config: CryptoConfig,
    ) -> Option<std::result::Result<Vec<u8>, node_lib::error::NodeError>> {
        let obu_key = dh_keys.get(&obu_vanet_mac)?;
        if obu_key.is_expired(key_ttl_ms) {
            return None;
        }
        Some(node_lib::crypto::encrypt_with_config(
            crypto_config.cipher,
            plaintext,
            &obu_key.key,
        ))
    }

    /// Encrypt `frame` for `obu_mac`, logging failures and returning `None` on any error.
    ///
    /// Combines the three outcomes of `encrypt_for_obu` into a single `Option`:
    /// - `Some(enc)` — ready to send
    /// - `None` — no session or encryption error (already logged; caller should skip)
    fn try_encrypt_for_obu(
        frame: &[u8],
        obu_mac: MacAddress,
        keys: &DhKeyStore,
        key_ttl_ms: u64,
        crypto_config: CryptoConfig,
        context: &str,
    ) -> Option<Vec<u8>> {
        match Self::encrypt_for_obu(frame, obu_mac, keys, key_ttl_ms, crypto_config) {
            Some(Ok(enc)) => Some(enc),
            Some(Err(e)) => {
                tracing::error!(obu = %obu_mac, error = %e, context, "Encryption failed");
                None
            }
            None => {
                tracing::debug!(obu = %obu_mac, context, "No DH session, skipping");
                None
            }
        }
    }

    /// Handle a KeyExchangeForward from an RSU: generate our keypair,
    /// compute the shared secret, store the per-OBU key, and send a
    /// KeyExchangeResponse back to the RSU.
    async fn handle_key_exchange_forward(
        ke_fwd: &crate::cloud_protocol::KeyExchangeForward,
        src_addr: SocketAddr,
        ctx: &ServerCtx,
    ) {
        let dh_keys = &ctx.dh_keys;
        let crypto_config = ctx.crypto_config;
        let socket = &ctx.socket;
        let enable_dh_signatures = ctx.enable_dh_signatures;
        let signing_keypair = ctx.signing_keypair.as_deref();
        let dh_signing_allowlist = &*ctx.dh_signing_allowlist;
        tracing::info!(
            obu = %ke_fwd.obu_mac,
            rsu = %ke_fwd.rsu_mac,
            "Received KeyExchangeForward from RSU"
        );
        let allowlist = dh_signing_allowlist.read().await;
        // Parse the KeyExchangeInit payload
        let ke_init = match node_lib::messages::auth::key_exchange::KeyExchangeInit::try_from(
            ke_fwd.payload.as_slice(),
        ) {
            Ok(init) => init,
            Err(e) => {
                tracing::warn!(
                    obu = %ke_fwd.obu_mac,
                    error = %e,
                    "Failed to parse KeyExchangeInit payload"
                );
                return;
            }
        };

        // Verify the OBU's signature if signatures are required.
        if enable_dh_signatures {
            match (
                ke_init.signing_algorithm(),
                ke_init.signing_pubkey(),
                ke_init.signature(),
            ) {
                (Some(algo), Some(spk), Some(sig)) => {
                    let base = ke_init.base_payload();
                    if let Err(e) = node_lib::crypto::verify_dh_signature(algo, &base, spk, sig) {
                        tracing::warn!(
                            obu = %ke_fwd.obu_mac,
                            error = %e,
                            "KeyExchangeInit has invalid signature, dropping"
                        );
                        return;
                    }
                    tracing::debug!(
                        obu = %ke_fwd.obu_mac,
                        "KeyExchangeInit signature verified"
                    );
                }
                _ => {
                    tracing::warn!(
                        obu = %ke_fwd.obu_mac,
                        "KeyExchangeInit is unsigned but enable_dh_signatures is set, dropping"
                    );
                    return;
                }
            }
        }

        // PKI allowlist check: if configured, the OBU's signing key must match the
        // pre-registered entry for its MAC address, closing the TOFU gap.
        // Signature verification must be enabled when an allowlist is configured;
        // without it the key bytes in the message are unverified and can be forged.
        if !allowlist.is_empty() && !enable_dh_signatures {
            tracing::warn!(
                obu = %ke_fwd.obu_mac,
                "dh_signing_allowlist is configured but enable_dh_signatures is false; \
                 dropping KeyExchangeInit to prevent allowlist bypass"
            );
            return;
        }
        if !allowlist.is_empty() {
            match (ke_init.signing_pubkey(), allowlist.get(&ke_fwd.obu_mac)) {
                (Some(spk), Some(expected)) if spk == expected.as_slice() => {
                    tracing::debug!(
                        obu = %ke_fwd.obu_mac,
                        "KeyExchangeInit signing key matches PKI allowlist"
                    );
                }
                (Some(_), Some(_)) => {
                    tracing::warn!(
                        obu = %ke_fwd.obu_mac,
                        "KeyExchangeInit signing key does not match PKI allowlist, dropping"
                    );
                    return;
                }
                (_, None) => {
                    tracing::warn!(
                        obu = %ke_fwd.obu_mac,
                        "OBU MAC not found in PKI allowlist, dropping"
                    );
                    return;
                }
                (None, Some(_)) => {
                    tracing::warn!(
                        obu = %ke_fwd.obu_mac,
                        "KeyExchangeInit unsigned but PKI allowlist is configured, dropping"
                    );
                    return;
                }
            }
        }

        // Release the allowlist lock before the expensive crypto operations below.
        drop(allowlist);

        let key_id = ke_init.key_id();
        let dh_group = ke_init.dh_group();

        // Reject key exchanges that use an algorithm the server isn't configured for.
        {
            let expected_group = crypto_config.dh_group;
            if dh_group != Some(expected_group) {
                tracing::warn!(
                    obu = %ke_fwd.obu_mac,
                    dh_group = ?dh_group,
                    expected = %expected_group,
                    "KeyExchangeInit dh_group does not match server crypto config, dropping"
                );
                return;
            }
        }

        // Deduplicate: if we already have a key for this OBU with the same key_id,
        // skip reprocessing (duplicate KeyExchangeInit can arrive via multiple
        // VANET paths when intermediate OBUs relay the message).
        {
            let store = dh_keys.read().await;
            if let Some(existing) = store.get(&ke_fwd.obu_mac) {
                if existing.key_id == key_id {
                    tracing::debug!(
                        obu = %ke_fwd.obu_mac,
                        key_id = key_id,
                        "Duplicate KeyExchangeInit for established key_id — ignoring, but NOT re-sending reply"
                    );
                    return;
                }
            }
        }

        use node_lib::crypto::DhGroup;

        // Perform the key exchange based on the algorithm in the init message.
        let (our_reply_material, shared_secret_bytes) = match dh_group {
            Some(DhGroup::X25519) => {
                let peer_pub_bytes: [u8; 32] = match ke_init.key_material().try_into() {
                    Ok(b) => b,
                    Err(_) => {
                        tracing::warn!(obu = %ke_fwd.obu_mac, "X25519 key_material wrong length");
                        return;
                    }
                };
                let our_keypair = DhKeypair::generate();
                let our_public = our_keypair.public.as_bytes().to_vec();
                let peer_public = x25519_dalek::PublicKey::from(peer_pub_bytes);
                let ss = our_keypair.diffie_hellman(&peer_public).as_bytes().to_vec();
                (our_public, ss)
            }
            Some(DhGroup::MlKem768) => {
                let ek_bytes: &[u8; node_lib::crypto::ML_KEM_768_EK_LEN] =
                    match ke_init.key_material().try_into() {
                        Ok(b) => b,
                        Err(_) => {
                            tracing::warn!(
                                obu = %ke_fwd.obu_mac,
                                "ML-KEM-768 encap key wrong length (expected {})",
                                node_lib::crypto::ML_KEM_768_EK_LEN,
                            );
                            return;
                        }
                    };
                match node_lib::crypto::kem_768_encapsulate(ek_bytes) {
                    Ok((ct, ss)) => (ct.to_vec(), ss.to_vec()),
                    Err(e) => {
                        tracing::error!(
                            obu = %ke_fwd.obu_mac,
                            error = %e,
                            "ML-KEM-768 encapsulation failed"
                        );
                        return;
                    }
                }
            }
            None => {
                tracing::warn!(
                    obu = %ke_fwd.obu_mac,
                    "Unknown key exchange algorithm in KeyExchangeInit, dropping"
                );
                return;
            }
        };

        let derived_key = match node_lib::crypto::derive_key(
            crypto_config.kdf,
            &shared_secret_bytes,
            key_id,
            crypto_config.cipher.key_len(),
        ) {
            Ok(k) => k,
            Err(e) => {
                tracing::error!(
                    obu = %ke_fwd.obu_mac,
                    error = %e,
                    "Failed to derive session key for OBU"
                );
                return;
            }
        };

        // Store the per-OBU key
        let key_len = derived_key.len();
        dh_keys.write().await.insert(
            ke_fwd.obu_mac,
            ObuKey {
                key: derived_key.into(),
                key_id,
                established_at: Instant::now(),
            },
        );

        tracing::info!(
            obu = %ke_fwd.obu_mac,
            rsu = %ke_fwd.rsu_mac,
            key_id = key_id,
            dh_group = ?dh_group,
            cipher = %crypto_config.cipher,
            kdf = %crypto_config.kdf,
            key_len = key_len,
            "Key exchange completed with OBU, session key established"
        );

        // Build the KeyExchangeReply payload.
        // Set sender = obu_mac so relay OBUs know who the final recipient is.
        // Sign the reply if signatures are enabled.
        let ke_reply = if let Some(kp) = signing_keypair {
            let sig_algo = kp.signing_algorithm();
            let unsigned = node_lib::messages::auth::key_exchange::KeyExchangeReply::new_unsigned(
                crypto_config.dh_group,
                key_id,
                our_reply_material.clone(),
                ke_fwd.obu_mac,
            );
            let base = unsigned.base_payload();
            let sig = kp.sign(&base);
            let spk = kp.verifying_key_bytes();
            node_lib::messages::auth::key_exchange::KeyExchangeReply::new_signed(
                crypto_config.dh_group,
                key_id,
                our_reply_material,
                ke_fwd.obu_mac,
                sig_algo,
                spk,
                sig,
            )
        } else {
            node_lib::messages::auth::key_exchange::KeyExchangeReply::new_unsigned(
                crypto_config.dh_group,
                key_id,
                our_reply_material,
                ke_fwd.obu_mac,
            )
        };
        let reply_bytes: Vec<u8> = (&ke_reply).into();

        // Send KeyExchangeResponse back to the RSU
        let rsp = match KeyExchangeResponse::new(ke_fwd.obu_mac, reply_bytes) {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(
                    obu = %ke_fwd.obu_mac,
                    error = %e,
                    "Failed to build KeyExchangeResponse"
                );
                return;
            }
        };
        if let Err(e) = socket.send_to(&rsp.to_bytes(), src_addr).await {
            tracing::error!(
                obu = %ke_fwd.obu_mac,
                error = %e,
                "Failed to send KeyExchangeResponse to RSU"
            );
        } else {
            tracing::info!(
                obu = %ke_fwd.obu_mac,
                rsu_addr = %src_addr,
                "Sent KeyExchangeResponse back to RSU"
            );
        }
    }

    /// Return a snapshot of the current RSU → OBU registry.
    pub async fn get_registry(&self) -> HashMap<MacAddress, Vec<MacAddress>> {
        self.registry.read().await.clone()
    }

    /// Return the OBUs currently associated with the given RSU MAC address.
    /// Returns an empty list if the RSU is not yet known to the server.
    pub async fn get_obus_for_rsu(&self, rsu_mac: MacAddress) -> Vec<MacAddress> {
        self.registry
            .read()
            .await
            .get(&rsu_mac)
            .cloned()
            .unwrap_or_default()
    }

    /// Return the number of OBU routes currently known.
    pub async fn obu_route_count(&self) -> usize {
        self.obu_routes.read().await.len()
    }

    /// Get the IP address of this server.
    pub fn ip(&self) -> Ipv4Addr {
        self.ip
    }

    /// Get the port this server is listening on.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Return the actual local address the server's UDP socket is bound to.
    ///
    /// Returns `None` if the server has not been started yet.
    /// Useful in tests where the server binds to port 0 (OS-assigned).
    pub async fn bound_addr(&self) -> Option<std::net::SocketAddr> {
        self.socket.get()?.local_addr().ok()
    }

    /// Revoke the active session for a single OBU.
    ///
    /// This immediately:
    /// 1. Removes the OBU's DH key from the server's key store (all further encrypted
    ///    traffic from that OBU will be dropped until it re-keys).
    /// 2. Sends a `SessionTerminatedForward` to the RSU currently serving the OBU,
    ///    which relays it as a VANET `SessionTerminated` control message so the OBU
    ///    learns about the revocation promptly and re-initiates key exchange.
    ///
    /// Returns `true` if an active session was found and terminated, `false` if the
    /// OBU had no established key (the notification is still sent if a route is known).
    pub async fn revoke_node(&self, obu_mac: MacAddress) -> bool {
        // Remove the DH key.
        let had_key = {
            let mut keys = self.dh_keys.write().await;
            keys.remove(&obu_mac).is_some()
        };

        // Look up the RSU address from the OBU route table (keyed by virtual TAP MAC;
        // scan by VANET MAC since that's what we have).
        let rsu_addr = {
            let routes = self.obu_routes.read().await;
            routes
                .values()
                .find(|r| r.vanet_mac == obu_mac)
                .map(|r| r.rsu_addr)
        };

        // Notify the RSU so the OBU can react promptly.
        if let Some(rsu_addr) = rsu_addr {
            use node_lib::messages::auth::session_terminated::SessionTerminated;
            use rand_core::{OsRng, RngCore};
            use std::time::{SystemTime, UNIX_EPOCH};
            let fwd = if let Some(ref kp) = self.signing_keypair {
                let timestamp_secs = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let mut nonce = [0u8; 8];
                OsRng.fill_bytes(&mut nonce);
                let sig_algo = kp.signing_algorithm();
                let payload =
                    SessionTerminated::build_signed_payload(obu_mac, timestamp_secs, nonce);
                let sig = kp.sign(&payload);
                SessionTerminatedForward::new_signed(obu_mac, timestamp_secs, nonce, sig_algo, sig)
            } else {
                SessionTerminatedForward::new(obu_mac)
            };
            if let Some(socket) = self.socket.get() {
                if let Err(e) = socket.send_to(&fwd.to_bytes(), rsu_addr).await {
                    tracing::warn!(
                        obu = %obu_mac,
                        error = %e,
                        "Failed to send SessionTerminatedForward to RSU"
                    );
                } else {
                    tracing::debug!(
                        obu = %obu_mac,
                        rsu = %rsu_addr,
                        "Sent SessionTerminatedForward to RSU"
                    );
                }
            }
        } else {
            tracing::debug!(
                obu = %obu_mac,
                "No known RSU route for revoked OBU; session key removed server-side only"
            );
        }

        tracing::info!(
            obu = %obu_mac,
            had_active_session = had_key,
            "Revoked OBU session"
        );
        had_key
    }

    /// Hot-reload the PKI allowlist.
    ///
    /// Computes the difference between the old and new allowlists and revokes the
    /// session of any OBU that was previously allowed but is absent from
    /// `new_allowlist`.  New entries are added silently (they will be enforced on
    /// the next key exchange attempt from those OBUs).
    ///
    /// If `new_allowlist` is empty the check is disabled — no sessions are revoked
    /// because an empty allowlist means "allow all".
    pub async fn reload_allowlist(&self, new_allowlist: HashMap<MacAddress, Vec<u8>>) {
        // Determine which OBUs have been removed from the allowlist.
        let revoked: Vec<MacAddress> = {
            let old = self.dh_signing_allowlist.read().await;
            if !old.is_empty() && !new_allowlist.is_empty() {
                old.keys()
                    .filter(|mac| !new_allowlist.contains_key(*mac))
                    .copied()
                    .collect()
            } else {
                Vec::new()
            }
        };

        let added = {
            let old = self.dh_signing_allowlist.read().await;
            new_allowlist
                .keys()
                .filter(|mac| !old.contains_key(*mac))
                .count()
        };

        // Atomically swap in the new allowlist.
        *self.dh_signing_allowlist.write().await = new_allowlist;

        tracing::info!(
            revoked = revoked.len(),
            added = added,
            "PKI allowlist reloaded"
        );

        // Revoke sessions for removed OBUs.
        for mac in revoked {
            self.revoke_node(mac).await;
        }
    }

    /// Return a snapshot of the current PKI signing allowlist.
    pub async fn get_dh_signing_allowlist(&self) -> HashMap<MacAddress, Vec<u8>> {
        self.dh_signing_allowlist.read().await.clone()
    }

    /// Return a snapshot of all non-expired DH sessions.
    /// Each entry is `(obu_vanet_mac, key_id, age_secs)`.
    pub async fn get_sessions(&self) -> Vec<(MacAddress, u32, u64)> {
        let store = self.dh_keys.read().await;
        store
            .iter()
            .filter(|(_, k)| !k.is_expired(self.key_ttl_ms))
            .map(|(mac, k)| (*mac, k.key_id, k.established_at.elapsed().as_secs()))
            .collect()
    }

    /// Return a snapshot of the OBU route table.
    /// Each entry is `(virtual_tap_mac, obu_vanet_mac, rsu_socket_addr)`.
    pub async fn get_routes(&self) -> Vec<(MacAddress, MacAddress, std::net::SocketAddr)> {
        let routes = self.obu_routes.read().await;
        routes
            .iter()
            .map(|(tap_mac, route)| (*tap_mac, route.vanet_mac, route.rsu_addr))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use std::time::Duration;

    /// Poll a condition with bounded timeout instead of fixed sleep.
    async fn poll_until<F, Fut>(f: F, timeout: Duration)
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if f().await {
                return;
            }
            if tokio::time::Instant::now() >= deadline {
                panic!("poll_until timed out after {:?}", timeout);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn test_server_creation() {
        let server = Server::new(Ipv4Addr::new(127, 0, 0, 1), 9999, "test_server".to_string());
        assert_eq!(server.ip(), Ipv4Addr::new(127, 0, 0, 1));
        assert_eq!(server.port(), 9999);
        assert!(server.get_registry().await.is_empty());
        assert_eq!(server.obu_route_count().await, 0);
    }

    #[tokio::test]
    async fn test_server_start_and_receive_registration() -> Result<()> {
        let server = Server::new(Ipv4Addr::new(127, 0, 0, 1), 0, "test_server".to_string());
        server.start().await?;

        let actual_port = { server.socket.get().unwrap().local_addr()?.port() };

        let rsu_mac: MacAddress = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF].into();
        let obu_mac: MacAddress = [1u8; 6].into();
        let msg = RegistrationMessage::new(rsu_mac, vec![obu_mac]);

        let client = UdpSocket::bind("127.0.0.1:0").await?;
        client
            .send_to(&msg.to_bytes(), format!("127.0.0.1:{}", actual_port))
            .await?;

        let server_ref = &server;
        poll_until(
            || async { !server_ref.get_obus_for_rsu(rsu_mac).await.is_empty() },
            Duration::from_secs(5),
        )
        .await;

        let obus = server.get_obus_for_rsu(rsu_mac).await;
        assert_eq!(obus, vec![obu_mac]);

        Ok(())
    }

    #[tokio::test]
    async fn test_server_receives_upstream_and_learns_route() -> Result<()> {
        let server = Server::new(Ipv4Addr::new(127, 0, 0, 1), 0, "test_server".to_string());
        server.start().await?;

        let actual_port = { server.socket.get().unwrap().local_addr()?.port() };

        let rsu_mac: MacAddress = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF].into();
        let obu_vanet_mac: MacAddress = [1u8; 6].into();
        // Simulate a TAP Ethernet frame: [dest_mac 6B][src_mac 6B][ethertype 2B][payload...]
        let obu_tap_mac: [u8; 6] = [0x02, 0x42, 0xAC, 0x10, 0x00, 0x02];
        let server_tap_mac: [u8; 6] = [0x02, 0x42, 0xAC, 0x10, 0x00, 0x64];
        let mut fake_frame = Vec::new();
        fake_frame.extend_from_slice(&server_tap_mac); // dest = server TAP
        fake_frame.extend_from_slice(&obu_tap_mac); // src = OBU TAP
        fake_frame.extend_from_slice(&[0x08, 0x00]); // ethertype IPv4
        fake_frame.extend_from_slice(b"test_payload");

        let fwd = UpstreamForward::new(rsu_mac, obu_vanet_mac, fake_frame);

        let client = UdpSocket::bind("127.0.0.1:0").await?;
        client
            .send_to(&fwd.to_bytes(), format!("127.0.0.1:{}", actual_port))
            .await?;

        let server_ref = &server;
        poll_until(
            || async { server_ref.obu_route_count().await == 1 },
            Duration::from_secs(5),
        )
        .await;

        assert_eq!(server.obu_route_count().await, 1);

        Ok(())
    }

    #[tokio::test]
    async fn test_get_obus_for_unknown_rsu_returns_empty() {
        let server = Server::new(Ipv4Addr::new(127, 0, 0, 1), 0, "test_server".to_string());
        let unknown: MacAddress = [9u8; 6].into();
        assert!(server.get_obus_for_rsu(unknown).await.is_empty());
    }
}

/// Tests that require the `test_helpers` feature (shim TUN devices).
#[cfg(all(test, feature = "test_helpers"))]
mod test_helpers_tests {
    use super::*;
    use crate::cloud_protocol::DownstreamForward;
    use std::net::Ipv4Addr;
    use std::time::Duration;

    /// Test the full downstream path: TAP frame → encrypt → DownstreamForward via UDP.
    ///
    /// Injects a frame into a shim TUN, pre-seeds obu_routes, and asserts that
    /// the server's UDP socket emits a valid DownstreamForward to the expected RSU.
    #[tokio::test]
    async fn tap_read_loop_sends_downstream_forward_unencrypted() -> anyhow::Result<()> {
        let (tun, tun_peer) = node_lib::test_helpers::util::mk_shim_pair();
        let tun = Arc::new(tun);

        let server = Server::new(Ipv4Addr::new(127, 0, 0, 1), 0, "test_server".to_string())
            .with_tun(tun)
            .with_encryption(false);
        server.start().await?;

        // Set up a "RSU" UDP socket that will receive the DownstreamForward
        let rsu_socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
        let rsu_addr = rsu_socket.local_addr()?;

        // Pre-seed an OBU route: virtual TAP MAC → (VANET MAC, RSU addr)
        let obu_tap_mac = MacAddress::new([0x02, 0x42, 0xAC, 0x10, 0x00, 0x02]);
        let obu_vanet_mac = MacAddress::new([0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x01]);
        server.obu_routes.write().await.insert(
            obu_tap_mac,
            ObuRoute {
                vanet_mac: obu_vanet_mac,
                rsu_addr,
            },
        );

        // Build an Ethernet frame destined for the OBU's virtual TAP MAC
        let server_tap_mac: [u8; 6] = [0x02, 0x42, 0xAC, 0x10, 0x00, 0x64];
        let mut frame = Vec::new();
        frame.extend_from_slice(&obu_tap_mac.bytes()); // dest = OBU TAP MAC
        frame.extend_from_slice(&server_tap_mac); // src = server TAP MAC
        frame.extend_from_slice(&[0x08, 0x00]); // ethertype IPv4
        frame.extend_from_slice(b"hello_from_server");

        // Inject frame into the server's TAP via the shim peer
        tun_peer.send_all(&frame).await?;

        // Receive the DownstreamForward on the RSU socket
        let mut buf = vec![0u8; 65536];
        let n = tokio::time::timeout(Duration::from_secs(5), rsu_socket.recv(&mut buf)).await??;

        let msg = DownstreamForward::try_from_bytes(&buf[..n])
            .expect("should parse as DownstreamForward");

        // The destination should be the OBU's VANET MAC (for RSU routing)
        assert_eq!(msg.obu_dest_mac, obu_vanet_mac);
        // Payload should be the raw frame (no encryption)
        assert_eq!(msg.payload, frame);

        Ok(())
    }

    /// Test downstream path with encryption enabled.
    /// Pre-seeds a DH session key so the server can encrypt.
    #[tokio::test]
    async fn tap_read_loop_sends_downstream_forward_encrypted() -> anyhow::Result<()> {
        let (tun, tun_peer) = node_lib::test_helpers::util::mk_shim_pair();
        let tun = Arc::new(tun);

        let crypto_config = node_lib::crypto::CryptoConfig::default();
        let server = Server::new(Ipv4Addr::new(127, 0, 0, 1), 0, "test_server".to_string())
            .with_tun(tun)
            .with_encryption(true)
            .with_crypto_config(crypto_config);
        server.start().await?;

        // Set up RSU receiver
        let rsu_socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
        let rsu_addr = rsu_socket.local_addr()?;

        // Pre-seed route
        let obu_tap_mac = MacAddress::new([0x02, 0x42, 0xAC, 0x10, 0x00, 0x03]);
        let obu_vanet_mac = MacAddress::new([0xCA, 0xFE, 0xBA, 0xBE, 0x00, 0x01]);
        server.obu_routes.write().await.insert(
            obu_tap_mac,
            ObuRoute {
                vanet_mac: obu_vanet_mac,
                rsu_addr,
            },
        );

        // Pre-seed a DH session key for this OBU
        let test_key: Arc<[u8]> = vec![0x42u8; crypto_config.cipher.key_len()].into();
        server.dh_keys.write().await.insert(
            obu_vanet_mac,
            ObuKey {
                key: test_key.clone(),
                key_id: 1,
                established_at: Instant::now(),
            },
        );

        // Build and inject Ethernet frame
        let server_tap_mac: [u8; 6] = [0x02, 0x42, 0xAC, 0x10, 0x00, 0x64];
        let mut frame = Vec::new();
        frame.extend_from_slice(&obu_tap_mac.bytes());
        frame.extend_from_slice(&server_tap_mac);
        frame.extend_from_slice(&[0x08, 0x00]);
        frame.extend_from_slice(b"encrypted_test_payload");

        tun_peer.send_all(&frame).await?;

        // Receive DownstreamForward
        let mut buf = vec![0u8; 65536];
        let n = tokio::time::timeout(Duration::from_secs(5), rsu_socket.recv(&mut buf)).await??;

        let msg = DownstreamForward::try_from_bytes(&buf[..n])
            .expect("should parse as DownstreamForward");

        assert_eq!(msg.obu_dest_mac, obu_vanet_mac);
        // Payload should be encrypted (different from original, larger due to overhead)
        assert_ne!(msg.payload, frame);
        assert!(msg.payload.len() >= frame.len() + 28); // 12 nonce + 16 tag

        // Decrypt with the same key and verify roundtrip
        let decrypted =
            node_lib::crypto::decrypt_with_config(crypto_config.cipher, &msg.payload, &test_key)
                .expect("decryption should succeed");
        assert_eq!(decrypted, frame);

        Ok(())
    }

    /// Test that upstream from OBU1 destined for OBU2's TAP MAC is L2-switched to OBU2,
    /// not written to the server TAP.
    #[tokio::test]
    async fn upstream_to_known_obu_is_l2_switched_not_to_tap() -> anyhow::Result<()> {
        let (server_tun, server_tun_peer) = node_lib::test_helpers::util::mk_shim_pair();
        let server_tun = Arc::new(server_tun);

        let server = Server::new(Ipv4Addr::new(127, 0, 0, 1), 0, "test_server".to_string())
            .with_tun(server_tun)
            .with_encryption(false);
        server.start().await?;

        let server_port = { server.socket.get().unwrap().local_addr()?.port() };

        // RSU socket (to receive DownstreamForward)
        let rsu_socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
        let rsu_addr = rsu_socket.local_addr()?;

        // Pre-seed OBU2 route: virtual TAP MAC → (VANET MAC, RSU addr)
        let obu2_tap_mac = MacAddress::new([0x02, 0x42, 0x00, 0x00, 0x00, 0x02]);
        let obu2_vanet_mac = MacAddress::new([0xBE, 0xEF, 0x00, 0x00, 0x00, 0x02]);
        server.obu_routes.write().await.insert(
            obu2_tap_mac,
            ObuRoute {
                vanet_mac: obu2_vanet_mac,
                rsu_addr,
            },
        );

        // Send UpstreamForward from OBU1 with dest MAC = OBU2's TAP MAC
        let obu1_vanet_mac = MacAddress::new([0xBE, 0xEF, 0x00, 0x00, 0x00, 0x01]);
        let obu1_tap_mac: [u8; 6] = [0x02, 0x42, 0x00, 0x00, 0x00, 0x01];
        let mut frame = Vec::new();
        frame.extend_from_slice(&obu2_tap_mac.bytes()); // dest = OBU2 TAP
        frame.extend_from_slice(&obu1_tap_mac); // src = OBU1 TAP
        frame.extend_from_slice(&[0x08, 0x00]); // ethertype
        frame.extend_from_slice(b"obu1_to_obu2");

        let fwd = UpstreamForward::new(MacAddress::new([0xAA; 6]), obu1_vanet_mac, frame.clone());
        let client = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
        client
            .send_to(&fwd.to_bytes(), format!("127.0.0.1:{}", server_port))
            .await?;

        // RSU should receive a DownstreamForward for OBU2
        let mut buf = vec![0u8; 65536];
        let n = tokio::time::timeout(Duration::from_secs(5), rsu_socket.recv(&mut buf)).await??;
        let downstream = DownstreamForward::try_from_bytes(&buf[..n])
            .expect("should parse as DownstreamForward");
        assert_eq!(downstream.obu_dest_mac, obu2_vanet_mac);
        assert_eq!(downstream.payload, frame);

        // Server TAP should NOT have received the frame (L2 switched, not local delivery)
        let result =
            tokio::time::timeout(Duration::from_millis(100), server_tun_peer.recv(&mut buf)).await;
        assert!(
            result.is_err(),
            "Server TAP should not receive L2-switched frame"
        );

        Ok(())
    }

    /// Test that broadcast frames are sent to all known OBU routes.
    #[tokio::test]
    async fn tap_read_loop_broadcasts_to_all_obus() -> anyhow::Result<()> {
        let (tun, tun_peer) = node_lib::test_helpers::util::mk_shim_pair();
        let tun = Arc::new(tun);

        let server = Server::new(Ipv4Addr::new(127, 0, 0, 1), 0, "test_server".to_string())
            .with_tun(tun)
            .with_encryption(false);
        server.start().await?;

        // Set up two RSU receivers
        let rsu1_socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
        let rsu1_addr = rsu1_socket.local_addr()?;
        let rsu2_socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
        let rsu2_addr = rsu2_socket.local_addr()?;

        // Pre-seed two OBU routes (different RSUs)
        let obu1_tap_mac = MacAddress::new([0x02, 0x42, 0x00, 0x00, 0x00, 0x01]);
        let obu1_vanet_mac = MacAddress::new([0xAA, 0x00, 0x00, 0x00, 0x00, 0x01]);
        let obu2_tap_mac = MacAddress::new([0x02, 0x42, 0x00, 0x00, 0x00, 0x02]);
        let obu2_vanet_mac = MacAddress::new([0xAA, 0x00, 0x00, 0x00, 0x00, 0x02]);

        {
            let mut routes = server.obu_routes.write().await;
            routes.insert(
                obu1_tap_mac,
                ObuRoute {
                    vanet_mac: obu1_vanet_mac,
                    rsu_addr: rsu1_addr,
                },
            );
            routes.insert(
                obu2_tap_mac,
                ObuRoute {
                    vanet_mac: obu2_vanet_mac,
                    rsu_addr: rsu2_addr,
                },
            );
        }

        // Build broadcast Ethernet frame
        let server_tap_mac: [u8; 6] = [0x02, 0x42, 0xAC, 0x10, 0x00, 0x64];
        let mut frame = Vec::new();
        frame.extend_from_slice(&[0xFF; 6]); // broadcast dest
        frame.extend_from_slice(&server_tap_mac);
        frame.extend_from_slice(&[0x08, 0x06]); // ARP
        frame.extend_from_slice(b"broadcast_data");

        tun_peer.send_all(&frame).await?;

        // Both RSU sockets should receive a DownstreamForward
        let mut buf1 = vec![0u8; 65536];
        let mut buf2 = vec![0u8; 65536];

        let n1 =
            tokio::time::timeout(Duration::from_secs(5), rsu1_socket.recv(&mut buf1)).await??;
        let n2 =
            tokio::time::timeout(Duration::from_secs(5), rsu2_socket.recv(&mut buf2)).await??;

        let msg1 = DownstreamForward::try_from_bytes(&buf1[..n1])
            .expect("RSU1 should receive valid DownstreamForward");
        let msg2 = DownstreamForward::try_from_bytes(&buf2[..n2])
            .expect("RSU2 should receive valid DownstreamForward");

        // Each message should target a different OBU VANET MAC
        let vanet_macs: std::collections::HashSet<_> =
            [msg1.obu_dest_mac, msg2.obu_dest_mac].into();
        assert!(vanet_macs.contains(&obu1_vanet_mac));
        assert!(vanet_macs.contains(&obu2_vanet_mac));

        Ok(())
    }
}
