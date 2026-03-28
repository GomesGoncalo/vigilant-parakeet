use crate::cloud_protocol::{
    CloudMessage, DownstreamForward, KeyExchangeResponse, UpstreamForward,
};
use crate::registry::RegistrationMessage;
use anyhow::Result;
use common::tun::Tun;
use mac_address::MacAddress;
use node_lib::crypto::{CryptoConfig, DhKeypair};
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
#[allow(dead_code)]
#[derive(Debug, Clone)]
struct ObuKey {
    /// The derived symmetric key.
    key: Vec<u8>,
    /// Key ID from the exchange.
    key_id: u32,
    /// When the key was established.
    established_at: Instant,
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
    /// Whether encryption is enabled for OBU traffic.
    enable_encryption: bool,
    /// Whether DH key exchange is enabled (per-OBU keys).
    enable_dh: bool,
    /// Per-OBU DH-derived keys, keyed by OBU VANET MAC.
    dh_keys: Arc<RwLock<DhKeyStore>>,
    /// Crypto configuration for key derivation.
    crypto_config: CryptoConfig,
    /// Node name for tracing/logging identification.
    node_name: String,
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
            enable_dh: false,
            dh_keys: Arc::new(RwLock::new(HashMap::new())),
            crypto_config: CryptoConfig::default(),
            node_name,
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

    /// Enable or disable DH key exchange (per-OBU session keys).
    pub fn with_dh(mut self, enable: bool) -> Self {
        self.enable_dh = enable;
        self
    }

    /// Set the crypto configuration for key derivation.
    pub fn with_crypto_config(mut self, config: CryptoConfig) -> Self {
        self.crypto_config = config;
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
        let enable_dh = self.enable_dh;
        let dh_keys = self.dh_keys.clone();
        let crypto_config = self.crypto_config;
        let name_for_recv = node_name.clone();

        let recv_span = tracing::info_span!("node", name = %name_for_recv);
        tokio::spawn(
            async move {
                Self::cloud_recv_loop(
                    socket_for_recv,
                    registry,
                    obu_routes,
                    tun_for_recv,
                    enable_encryption,
                    enable_dh,
                    dh_keys,
                    crypto_config,
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
            let enable_dh_tap = self.enable_dh;
            let dh_keys_tap = self.dh_keys.clone();
            let crypto_config_tap = self.crypto_config;
            let name_for_tap = node_name.clone();

            let tap_span = tracing::info_span!("node", name = %name_for_tap);
            tokio::spawn(
                async move {
                    Self::tap_read_loop(
                        tun_for_tap,
                        socket_for_tap,
                        obu_routes_for_tap,
                        enable_enc,
                        enable_dh_tap,
                        dh_keys_tap,
                        crypto_config_tap,
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
        enable_dh: bool,
        dh_keys: Arc<RwLock<DhKeyStore>>,
        crypto_config: CryptoConfig,
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
                                enable_dh,
                                &dh_keys,
                                crypto_config,
                                &socket,
                            )
                            .await;
                        }
                        Some(CloudMessage::KeyExchangeForward(ke_fwd)) => {
                            Self::handle_key_exchange_forward(
                                &ke_fwd,
                                src_addr,
                                &dh_keys,
                                crypto_config,
                                &socket,
                            )
                            .await;
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
        enable_dh: bool,
        dh_keys: &Arc<RwLock<DhKeyStore>>,
        crypto_config: CryptoConfig,
        socket: &Arc<UdpSocket>,
    ) {
        // Decrypt the payload if encryption is enabled.
        let tap_frame = if enable_encryption {
            let decrypt_result = if enable_dh {
                let key = dh_keys
                    .read()
                    .await
                    .get(&fwd.obu_source_mac)
                    .map(|k| k.key.clone());
                if let Some(key) = key {
                    node_lib::crypto::decrypt_with_config(crypto_config.cipher, &fwd.payload, &key)
                } else {
                    // Fallback to fixed key
                    let fallback = &node_lib::crypto::FIXED_KEY[..crypto_config.cipher.key_len()];
                    node_lib::crypto::decrypt_with_config(
                        crypto_config.cipher,
                        &fwd.payload,
                        fallback,
                    )
                }
            } else {
                node_lib::crypto::decrypt_with_config(
                    crypto_config.cipher,
                    &fwd.payload,
                    &node_lib::crypto::FIXED_KEY[..crypto_config.cipher.key_len()],
                )
            };
            match decrypt_result {
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
            let routes = obu_routes.read().await;
            let keys = dh_keys.read().await;
            for (_, route) in routes.iter() {
                let payload_for_obu = if enable_encryption {
                    let enc_result = Self::encrypt_for_obu(
                        &tap_frame,
                        route.vanet_mac,
                        enable_dh,
                        &keys,
                        crypto_config,
                    );
                    match enc_result {
                        Ok(enc) => enc,
                        Err(e) => {
                            tracing::error!(
                                obu = %route.vanet_mac,
                                error = %e,
                                "Failed to re-encrypt multicast frame for OBU"
                            );
                            continue;
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
                if let Err(e) = socket.send_to(&downstream.to_bytes(), route.rsu_addr).await {
                    tracing::error!(
                        obu = %route.vanet_mac,
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
                        enable_dh,
                        &keys,
                        crypto_config,
                    ) {
                        Ok(enc) => enc,
                        Err(e) => {
                            tracing::error!(error = %e, "Failed to re-encrypt frame for OBU L2 switch");
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
        enable_dh: bool,
        dh_keys: Arc<RwLock<DhKeyStore>>,
        crypto_config: CryptoConfig,
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
                // Send to all known OBU routes, encrypting per-OBU if DH enabled
                let routes = obu_routes.read().await;
                let keys = dh_keys.read().await;
                for (&_tap_mac, route) in routes.iter() {
                    let payload_data = if enable_encryption {
                        match Self::encrypt_for_obu(
                            frame,
                            route.vanet_mac,
                            enable_dh,
                            &keys,
                            crypto_config,
                        ) {
                            Ok(enc) => enc,
                            Err(e) => {
                                tracing::error!(
                                    obu = %route.vanet_mac,
                                    error = %e,
                                    "Failed to encrypt broadcast downstream for OBU"
                                );
                                continue;
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
                    if let Err(e) = socket.send_to(&fwd.to_bytes(), route.rsu_addr).await {
                        tracing::error!(
                            obu = %route.vanet_mac,
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
                            enable_dh,
                            &keys,
                            crypto_config,
                        ) {
                            Ok(enc) => enc,
                            Err(e) => {
                                tracing::error!(
                                    obu = %dest_mac,
                                    error = %e,
                                    "Failed to encrypt downstream for OBU"
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

    /// Encrypt a payload for a specific OBU, using its DH key if available.
    fn encrypt_for_obu(
        plaintext: &[u8],
        obu_vanet_mac: MacAddress,
        enable_dh: bool,
        dh_keys: &DhKeyStore,
        crypto_config: CryptoConfig,
    ) -> std::result::Result<Vec<u8>, node_lib::error::NodeError> {
        if enable_dh {
            if let Some(obu_key) = dh_keys.get(&obu_vanet_mac) {
                return node_lib::crypto::encrypt_with_config(
                    crypto_config.cipher,
                    plaintext,
                    &obu_key.key,
                );
            }
        }
        // Fallback to fixed key
        let fallback = &node_lib::crypto::FIXED_KEY[..crypto_config.cipher.key_len()];
        node_lib::crypto::encrypt_with_config(crypto_config.cipher, plaintext, fallback)
    }

    /// Handle a KeyExchangeForward from an RSU: generate our keypair,
    /// compute the shared secret, store the per-OBU key, and send a
    /// KeyExchangeResponse back to the RSU.
    async fn handle_key_exchange_forward(
        ke_fwd: &crate::cloud_protocol::KeyExchangeForward,
        src_addr: SocketAddr,
        dh_keys: &Arc<RwLock<DhKeyStore>>,
        crypto_config: CryptoConfig,
        socket: &Arc<UdpSocket>,
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

        let key_id = ke_init.key_id();
        let peer_pub_bytes = ke_init.public_key();

        // Generate our keypair and compute shared secret
        let our_keypair = DhKeypair::generate();
        let our_public = *our_keypair.public.as_bytes();

        let peer_public = x25519_dalek::PublicKey::from(peer_pub_bytes);
        let shared_secret = our_keypair.diffie_hellman(&peer_public);
        let derived_key = node_lib::crypto::derive_key(
            crypto_config.kdf,
            shared_secret.as_bytes(),
            key_id,
            crypto_config.cipher.key_len(),
        );

        // Store the per-OBU key
        dh_keys.write().await.insert(
            ke_fwd.obu_mac,
            ObuKey {
                key: derived_key.clone(),
                key_id,
                established_at: Instant::now(),
            },
        );

        tracing::info!(
            obu = %ke_fwd.obu_mac,
            rsu = %ke_fwd.rsu_mac,
            key_id = key_id,
            cipher = %crypto_config.cipher,
            kdf = %crypto_config.kdf,
            key_len = derived_key.len(),
            "DH key exchange completed with OBU, session key established"
        );

        // Build the KeyExchangeReply payload (42 bytes: key_id + public_key + sender)
        // The "sender" in the reply is [0;6] (server has no VANET MAC)
        let ke_reply = node_lib::messages::control::key_exchange::KeyExchangeReply::new(
            key_id,
            our_public,
            MacAddress::new([0; 6]), // Server "MAC"
        );
        let reply_bytes: Vec<u8> = (&ke_reply).into();

        // Send KeyExchangeResponse back to the RSU
        let rsp = KeyExchangeResponse::new(ke_fwd.obu_mac, reply_bytes);
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
    #[tokio::test]
    async fn tap_read_loop_sends_downstream_forward_encrypted() -> anyhow::Result<()> {
        let (tun, tun_peer) = node_lib::test_helpers::util::mk_shim_pair();
        let tun = Arc::new(tun);

        let server = Server::new(Ipv4Addr::new(127, 0, 0, 1), 0, "test_server".to_string())
            .with_tun(tun)
            .with_encryption(true);
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

        // Decrypt and verify roundtrip
        let decrypted =
            node_lib::crypto::decrypt_payload(&msg.payload).expect("decryption should succeed");
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
