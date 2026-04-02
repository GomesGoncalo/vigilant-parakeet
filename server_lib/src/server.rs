use crate::cloud_protocol::{
    CloudMessage, DownstreamForward, KeyExchangeResponse, UpstreamForward,
};
use crate::registry::RegistrationMessage;
use anyhow::Result;
use common::tun::Tun;
use mac_address::MacAddress;
use node_lib::crypto::{
    sig_algo_from_id, CryptoConfig, DhKeypair, SigningAlgorithm, SigningKeypair,
};
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::{Mutex, RwLock};
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
    /// UDP socket for receiving traffic from RSUs.
    socket: Arc<Mutex<Option<Arc<UdpSocket>>>>,
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
    /// Ed25519 identity keypair for signing DH replies (present when `enable_dh_signatures`).
    signing_keypair: Option<Arc<SigningKeypair>>,
    /// PKI allowlist: OBU VANET MAC → expected verifying key bytes (Ed25519: 32B, ML-DSA-65: 1952B).
    /// When non-empty and enable_dh_signatures is set, only OBUs whose signing key
    /// matches the registered entry are allowed to complete key exchange.
    dh_signing_allowlist: HashMap<MacAddress, Vec<u8>>,
    /// Signing algorithm used for DH message signing (default: Ed25519).
    signing_algorithm: SigningAlgorithm,
}

impl Server {
    /// Create a new Server that will listen on the specified IP and port.
    /// Note: The server does not start listening until `start()` is called.
    pub fn new(ip: Ipv4Addr, port: u16, node_name: String) -> Self {
        Self {
            ip,
            port,
            socket: Arc::new(Mutex::new(None)),
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
            dh_signing_allowlist: HashMap::new(),
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
        self.dh_signing_allowlist = allowlist;
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
            tracing::info!(
                signing_pubkey = %pubkey_hex,
                pki_entries = self.dh_signing_allowlist.len(),
                "DH signing enabled on server"
            );
        }

        let socket = UdpSocket::bind(&bind_addr).await?;
        let socket = Arc::new(socket);

        {
            let mut sock_lock = self.socket.lock().await;
            *sock_lock = Some(socket.clone());
        }

        // Spawn cloud recv task (handles registration + upstream forwarding + key exchange)
        let socket_for_recv = socket.clone();
        let registry = self.registry.clone();
        let obu_routes = self.obu_routes.clone();
        let tun_for_recv = self.tun.clone();
        let enable_encryption = self.enable_encryption;
        let enable_dh_signatures = self.enable_dh_signatures;
        let signing_keypair = self.signing_keypair.clone();
        let dh_signing_allowlist = Arc::new(self.dh_signing_allowlist.clone());
        let dh_keys = self.dh_keys.clone();
        let crypto_config = self.crypto_config;
        let name_for_recv = node_name.clone();

        let key_ttl_ms_recv = self.key_ttl_ms;
        let recv_span = tracing::info_span!("node", name = %name_for_recv);
        tokio::spawn(
            async move {
                Self::cloud_recv_loop(
                    socket_for_recv,
                    registry,
                    obu_routes,
                    tun_for_recv,
                    enable_encryption,
                    enable_dh_signatures,
                    signing_keypair,
                    dh_signing_allowlist,
                    dh_keys,
                    crypto_config,
                    key_ttl_ms_recv,
                )
                .await;
            }
            .instrument(recv_span),
        );

        // Spawn TAP read task if a TUN device is available
        if let Some(tun) = &self.tun {
            let tun_for_tap = tun.clone();
            let socket_for_tap = socket.clone();
            let obu_routes_for_tap = self.obu_routes.clone();
            let enable_enc = self.enable_encryption;
            let dh_keys_tap = self.dh_keys.clone();
            let crypto_config_tap = self.crypto_config;
            let key_ttl_ms_tap = self.key_ttl_ms;
            let name_for_tap = node_name.clone();

            let tap_span = tracing::info_span!("node", name = %name_for_tap);
            tokio::spawn(
                async move {
                    Self::tap_read_loop(
                        tun_for_tap,
                        socket_for_tap,
                        obu_routes_for_tap,
                        enable_enc,
                        dh_keys_tap,
                        crypto_config_tap,
                        key_ttl_ms_tap,
                    )
                    .await;
                }
                .instrument(tap_span),
            );
        }

        Ok(())
    }

    /// Main cloud receive loop: handles Registration, UpstreamForward, KeyExchangeForward.
    #[allow(clippy::too_many_arguments)]
    async fn cloud_recv_loop(
        socket: Arc<UdpSocket>,
        registry: Arc<RwLock<HashMap<MacAddress, Vec<MacAddress>>>>,
        obu_routes: Arc<RwLock<HashMap<MacAddress, ObuRoute>>>,
        tun: Option<SharedTun>,
        enable_encryption: bool,
        enable_dh_signatures: bool,
        signing_keypair: Option<Arc<SigningKeypair>>,
        dh_signing_allowlist: Arc<HashMap<MacAddress, Vec<u8>>>,
        dh_keys: Arc<RwLock<DhKeyStore>>,
        crypto_config: CryptoConfig,
        key_ttl_ms: u64,
    ) {
        let mut buf = vec![0u8; 65536];
        loop {
            match socket.recv_from(&mut buf).await {
                Ok((len, src_addr)) => {
                    let data = &buf[..len];
                    match CloudMessage::try_from_bytes(data) {
                        Some(CloudMessage::Registration(msg)) => {
                            Self::handle_registration(&registry, &msg, src_addr).await;
                        }
                        Some(CloudMessage::UpstreamForward(fwd)) => {
                            Self::handle_upstream(
                                &fwd,
                                src_addr,
                                &obu_routes,
                                tun.as_ref(),
                                enable_encryption,
                                &dh_keys,
                                key_ttl_ms,
                                crypto_config,
                                &socket,
                            )
                            .await;
                        }
                        Some(CloudMessage::KeyExchangeForward(ke_fwd)) => {
                            if enable_encryption {
                                Self::handle_key_exchange_forward(
                                    &ke_fwd,
                                    src_addr,
                                    &dh_keys,
                                    crypto_config,
                                    &socket,
                                    enable_dh_signatures,
                                    signing_keypair.as_deref(),
                                    &dh_signing_allowlist,
                                )
                                .await;
                            } else {
                                tracing::warn!(
                                    src = %src_addr,
                                    "Ignoring KeyExchangeForward — encryption is disabled"
                                );
                            }
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
    #[allow(clippy::too_many_arguments)]
    async fn handle_upstream(
        fwd: &UpstreamForward,
        src_addr: SocketAddr,
        obu_routes: &Arc<RwLock<HashMap<MacAddress, ObuRoute>>>,
        tun: Option<&SharedTun>,
        enable_encryption: bool,
        dh_keys: &Arc<RwLock<DhKeyStore>>,
        key_ttl_ms: u64,
        crypto_config: CryptoConfig,
        socket: &Arc<UdpSocket>,
    ) {
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

        if tap_frame.len() < 12 {
            // Too short to contain an Ethernet header — pass to TAP as-is.
            if let Some(tun) = tun {
                if let Err(e) = tun.send_all(&tap_frame).await {
                    tracing::error!(error = %e, "Failed to write short upstream frame to TAP");
                }
            }
            return;
        }

        // Learn the OBU's virtual TAP MAC from the Ethernet frame source (bytes 6..12).
        let src_mac_bytes: [u8; 6] = tap_frame[6..12].try_into().unwrap();
        let virtual_tap_mac = MacAddress::new(src_mac_bytes);
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

        // Extract dest MAC for routing decision.
        let dest_mac_bytes: [u8; 6] = tap_frame[..6].try_into().unwrap();
        let dest_mac = MacAddress::new(dest_mac_bytes);
        let is_multicast = dest_mac_bytes[0] & 0x01 != 0;

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
                        let payload_for_obu = if enable_encryption {
                            match Self::encrypt_for_obu(
                                &tap_frame,
                                route.vanet_mac,
                                &keys,
                                key_ttl_ms,
                                crypto_config,
                            ) {
                                Some(Ok(enc)) => enc,
                                Some(Err(e)) => {
                                    tracing::error!(
                                        obu = %route.vanet_mac,
                                        error = %e,
                                        "Failed to re-encrypt multicast frame for OBU"
                                    );
                                    return None;
                                }
                                None => {
                                    tracing::debug!(
                                        obu = %route.vanet_mac,
                                        "No DH session for OBU, skipping multicast re-encrypt"
                                    );
                                    return None;
                                }
                            }
                        } else {
                            tap_frame.clone()
                        };
                        let downstream = DownstreamForward::new(
                            route.vanet_mac,
                            MacAddress::new([0; 6]),
                            payload_for_obu,
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
                    match Self::encrypt_for_obu(
                        &tap_frame,
                        route.vanet_mac,
                        &keys,
                        key_ttl_ms,
                        crypto_config,
                    ) {
                        Some(Ok(enc)) => enc,
                        Some(Err(e)) => {
                            tracing::error!(error = %e, "Failed to re-encrypt frame for OBU L2 switch");
                            return;
                        }
                        None => {
                            tracing::debug!(
                                obu = %route.vanet_mac,
                                "No DH session for OBU, dropping L2-switched frame"
                            );
                            return;
                        }
                    }
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
    #[allow(clippy::too_many_arguments)]
    async fn tap_read_loop(
        tun: SharedTun,
        socket: Arc<UdpSocket>,
        obu_routes: Arc<RwLock<HashMap<MacAddress, ObuRoute>>>,
        enable_encryption: bool,
        dh_keys: Arc<RwLock<DhKeyStore>>,
        crypto_config: CryptoConfig,
        key_ttl_ms: u64,
    ) {
        let mut buf = vec![0u8; 65536];
        loop {
            let n = match tun.recv(&mut buf).await {
                Ok(n) => n,
                Err(e) => {
                    tracing::error!(error = %e, "Error reading from TAP device");
                    continue;
                }
            };

            if n < 14 {
                continue; // Need at least an Ethernet header
            }

            let frame = &buf[..n];
            // Ethernet frame: first 6 bytes = destination MAC
            let dest_mac_bytes: [u8; 6] = frame[..6].try_into().unwrap();
            let dest_mac = MacAddress::new(dest_mac_bytes);

            // Broadcast (FF:FF:FF:FF:FF:FF) has the group bit set, so is_multicast
            // already covers it — no separate is_broadcast check needed.
            let is_multicast = dest_mac_bytes[0] & 0x01 != 0;

            if is_multicast {
                // Snapshot routes and encrypt payloads while holding locks,
                // then drop locks before awaiting on network I/O.
                let sends: Vec<_> = {
                    let routes = obu_routes.read().await;
                    let keys = dh_keys.read().await;
                    routes
                        .iter()
                        .filter_map(|(&_tap_mac, route)| {
                            let payload_data = if enable_encryption {
                                match Self::encrypt_for_obu(
                                    frame,
                                    route.vanet_mac,
                                    &keys,
                                    key_ttl_ms,
                                    crypto_config,
                                ) {
                                    Some(Ok(enc)) => enc,
                                    Some(Err(e)) => {
                                        tracing::error!(
                                            obu = %route.vanet_mac,
                                            error = %e,
                                            "Failed to encrypt broadcast downstream for OBU"
                                        );
                                        return None;
                                    }
                                    None => {
                                        tracing::debug!(
                                            obu = %route.vanet_mac,
                                            "No DH session for OBU, skipping broadcast"
                                        );
                                        return None;
                                    }
                                }
                            } else {
                                frame.to_vec()
                            };
                            let fwd = DownstreamForward::new(
                                route.vanet_mac,
                                MacAddress::new([0; 6]), // server origin
                                payload_data,
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
                        match Self::encrypt_for_obu(
                            frame,
                            route.vanet_mac,
                            &keys,
                            key_ttl_ms,
                            crypto_config,
                        ) {
                            Some(Ok(enc)) => enc,
                            Some(Err(e)) => {
                                tracing::error!(
                                    obu = %dest_mac,
                                    error = %e,
                                    "Failed to encrypt downstream for OBU"
                                );
                                continue;
                            }
                            None => {
                                tracing::debug!(
                                    obu = %dest_mac,
                                    "No DH session for OBU, dropping downstream"
                                );
                                continue;
                            }
                        }
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

    /// Handle a KeyExchangeForward from an RSU: generate our keypair,
    /// compute the shared secret, store the per-OBU key, and send a
    /// KeyExchangeResponse back to the RSU.
    #[allow(clippy::too_many_arguments)]
    async fn handle_key_exchange_forward(
        ke_fwd: &crate::cloud_protocol::KeyExchangeForward,
        src_addr: SocketAddr,
        dh_keys: &Arc<RwLock<DhKeyStore>>,
        crypto_config: CryptoConfig,
        socket: &Arc<UdpSocket>,
        enable_dh_signatures: bool,
        signing_keypair: Option<&SigningKeypair>,
        dh_signing_allowlist: &HashMap<MacAddress, Vec<u8>>,
    ) {
        // Parse the KeyExchangeInit payload
        let ke_init = match node_lib::messages::control::key_exchange::KeyExchangeInit::try_from(
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
                ke_init.sig_algo_id(),
                ke_init.signing_pubkey(),
                ke_init.signature(),
            ) {
                (Some(sig_algo), Some(spk), Some(sig)) => {
                    let algo = match sig_algo_from_id(sig_algo) {
                        Some(a) => a,
                        None => {
                            tracing::warn!(
                                obu = %ke_fwd.obu_mac,
                                sig_algo_id = sig_algo,
                                "KeyExchangeInit uses unknown signature algorithm, dropping"
                            );
                            return;
                        }
                    };
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
        if !dh_signing_allowlist.is_empty() && !enable_dh_signatures {
            tracing::warn!(
                obu = %ke_fwd.obu_mac,
                "dh_signing_allowlist is configured but enable_dh_signatures is false; \
                 dropping KeyExchangeInit to prevent allowlist bypass"
            );
            return;
        }
        if !dh_signing_allowlist.is_empty() {
            match (
                ke_init.signing_pubkey(),
                dh_signing_allowlist.get(&ke_fwd.obu_mac),
            ) {
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

        let key_id = ke_init.key_id();
        let algo_id = ke_init.algo_id();

        // Deduplicate: if we already have a key for this OBU with the same key_id,
        // skip reprocessing (duplicate KeyExchangeInit can arrive via multiple
        // VANET paths when intermediate OBUs relay the message).
        {
            let store = dh_keys.read().await;
            if let Some(existing) = store.get(&ke_fwd.obu_mac) {
                if existing.key_id == key_id {
                    tracing::trace!(
                        obu = %ke_fwd.obu_mac,
                        key_id = key_id,
                        "Duplicate KeyExchangeInit for same key_id, ignoring"
                    );
                    return;
                }
            }
        }

        use node_lib::messages::control::key_exchange::{
            KE_ALGO_ML_KEM_768, KE_ALGO_X25519, SIG_ALGO_ED25519, SIG_ALGO_ML_DSA_65,
        };

        // Perform the key exchange based on the algorithm in the init message.
        let (our_reply_material, shared_secret_bytes) = match algo_id {
            KE_ALGO_X25519 => {
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
            KE_ALGO_ML_KEM_768 => {
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
            _ => {
                tracing::warn!(
                    obu = %ke_fwd.obu_mac,
                    algo_id = algo_id,
                    "Unknown key exchange algorithm, dropping"
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
            algo_id = algo_id,
            cipher = %crypto_config.cipher,
            kdf = %crypto_config.kdf,
            key_len = key_len,
            "Key exchange completed with OBU, session key established"
        );

        // Build the KeyExchangeReply payload.
        // Set sender = obu_mac so relay OBUs know who the final recipient is.
        // Sign the reply if signatures are enabled.
        let ke_reply = if let Some(kp) = signing_keypair {
            let sig_algo_id = match kp.signing_algorithm() {
                SigningAlgorithm::Ed25519 => SIG_ALGO_ED25519,
                SigningAlgorithm::MlDsa65 => SIG_ALGO_ML_DSA_65,
            };
            let unsigned = node_lib::messages::control::key_exchange::KeyExchangeReply::new_raw(
                algo_id,
                key_id,
                our_reply_material.clone(),
                ke_fwd.obu_mac,
                None,
                None,
                None,
            );
            let base = unsigned.base_payload();
            let sig = kp.sign(&base);
            let spk = kp.verifying_key_bytes();
            node_lib::messages::control::key_exchange::KeyExchangeReply::new_raw(
                algo_id,
                key_id,
                our_reply_material,
                ke_fwd.obu_mac,
                Some(sig_algo_id),
                Some(spk),
                Some(sig),
            )
        } else {
            node_lib::messages::control::key_exchange::KeyExchangeReply::new_raw(
                algo_id,
                key_id,
                our_reply_material,
                ke_fwd.obu_mac,
                None,
                None,
                None,
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
        let sock = self.socket.lock().await;
        sock.as_ref()?.local_addr().ok()
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

        let actual_port = {
            let sock_lock = server.socket.lock().await;
            sock_lock.as_ref().unwrap().local_addr()?.port()
        };

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

        let actual_port = {
            let sock_lock = server.socket.lock().await;
            sock_lock.as_ref().unwrap().local_addr()?.port()
        };

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

        let server_port = {
            let sock = server.socket.lock().await;
            sock.as_ref().unwrap().local_addr()?.port()
        };

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
