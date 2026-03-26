use crate::cloud_protocol::{CloudMessage, DownstreamForward, UpstreamForward};
use crate::registry::RegistrationMessage;
use anyhow::Result;
use common::tun::Tun;
use mac_address::MacAddress;
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::{Mutex, RwLock};
use tracing::Instrument;

/// Shared reference to a Tun device.
pub type SharedTun = Arc<Tun>;

/// ServerNode receives traffic from RSU nodes over UDP via the cloud interface.
///
/// The Server owns the TAP device and handles all encryption/decryption of OBU
/// traffic. RSUs are transparent relays: they forward upstream data from OBUs
/// to the Server (as `UpstreamForward`), and the Server sends downstream data
/// back through the appropriate RSU (as `DownstreamForward`).
///
/// The server maintains:
/// - A registry of RSU MAC → associated OBU MACs (from `RegistrationMessage`)
/// - An OBU routing table: OBU MAC → RSU socket address (learned from upstream traffic)
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
    /// OBU routing table: OBU MAC → RSU socket address (for downstream delivery).
    obu_routes: Arc<RwLock<HashMap<MacAddress, SocketAddr>>>,
    /// Optional TAP device for decapsulated traffic.
    tun: Option<SharedTun>,
    /// Whether encryption is enabled for OBU traffic.
    enable_encryption: bool,
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

        // Spawn cloud recv task (handles registration + upstream forwarding)
        let socket_for_recv = socket.clone();
        let registry = self.registry.clone();
        let obu_routes = self.obu_routes.clone();
        let tun_for_recv = self.tun.clone();
        let enable_encryption = self.enable_encryption;
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
            let name_for_tap = node_name.clone();

            let tap_span = tracing::info_span!("node", name = %name_for_tap);
            tokio::spawn(
                async move {
                    Self::tap_read_loop(
                        tun_for_tap,
                        socket_for_tap,
                        obu_routes_for_tap,
                        enable_enc,
                    )
                    .await;
                }
                .instrument(tap_span),
            );
        }

        Ok(())
    }

    /// Main cloud receive loop: handles Registration, UpstreamForward messages.
    async fn cloud_recv_loop(
        socket: Arc<UdpSocket>,
        registry: Arc<RwLock<HashMap<MacAddress, Vec<MacAddress>>>>,
        obu_routes: Arc<RwLock<HashMap<MacAddress, SocketAddr>>>,
        tun: Option<SharedTun>,
        enable_encryption: bool,
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
                            // Learn OBU→RSU route from upstream traffic
                            obu_routes
                                .write()
                                .await
                                .insert(fwd.obu_source_mac, src_addr);

                            Self::handle_upstream(&fwd, tun.as_ref(), enable_encryption).await;
                        }
                        Some(CloudMessage::DownstreamForward(_)) => {
                            tracing::warn!(
                                src = %src_addr,
                                "Received unexpected DownstreamForward on server"
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
    /// The payload inside `UpstreamForward` is the raw `ToUpstream` data:
    /// `[origin_mac 6B][encrypted_data...]`
    ///
    /// We decrypt the data portion and write the original TAP frame to the TAP device.
    async fn handle_upstream(
        fwd: &UpstreamForward,
        tun: Option<&SharedTun>,
        enable_encryption: bool,
    ) {
        let tun = match tun {
            Some(t) => t,
            None => {
                tracing::debug!(
                    obu = %fwd.obu_source_mac,
                    "Upstream received but no TAP device configured"
                );
                return;
            }
        };

        // The payload is [origin_mac 6B][data...]
        if fwd.payload.len() < 6 {
            tracing::warn!(
                len = fwd.payload.len(),
                "Upstream payload too short (need at least 6 bytes for origin MAC)"
            );
            return;
        }

        let data = &fwd.payload[6..];
        let tap_data = if enable_encryption {
            match node_lib::crypto::decrypt_payload(data) {
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
            data.to_vec()
        };

        if let Err(e) = tun.send_all(&tap_data).await {
            tracing::error!(error = %e, "Failed to write decrypted data to TAP");
        }
    }

    /// Read frames from TAP, encrypt, and send downstream to the appropriate RSU.
    async fn tap_read_loop(
        tun: SharedTun,
        socket: Arc<UdpSocket>,
        obu_routes: Arc<RwLock<HashMap<MacAddress, SocketAddr>>>,
        enable_encryption: bool,
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

            if n < 6 {
                continue; // Need at least destination MAC
            }

            let frame = &buf[..n];
            // Ethernet frame: first 6 bytes = destination MAC
            let dest_mac_bytes: [u8; 6] = frame[..6].try_into().unwrap();
            let dest_mac = MacAddress::new(dest_mac_bytes);

            // Encrypt the frame data for the OBU
            let payload_data = if enable_encryption {
                match node_lib::crypto::encrypt_payload(frame) {
                    Ok(encrypted) => encrypted,
                    Err(e) => {
                        tracing::error!(error = %e, "Failed to encrypt downstream payload");
                        continue;
                    }
                }
            } else {
                frame.to_vec()
            };

            let is_broadcast = dest_mac_bytes == [0xFF; 6];

            if is_broadcast {
                // Send to all known OBU routes
                let routes = obu_routes.read().await;
                for (&obu_mac, &rsu_addr) in routes.iter() {
                    let fwd = DownstreamForward::new(
                        obu_mac,
                        MacAddress::new([0; 6]), // server origin
                        payload_data.clone(),
                    );
                    if let Err(e) = socket.send_to(&fwd.to_bytes(), rsu_addr).await {
                        tracing::error!(
                            obu = %obu_mac,
                            error = %e,
                            "Failed to send broadcast downstream to RSU"
                        );
                    }
                }
            } else {
                // Unicast: find the RSU that has this OBU
                let rsu_addr = {
                    let routes = obu_routes.read().await;
                    routes.get(&dest_mac).copied()
                };

                if let Some(addr) = rsu_addr {
                    let fwd = DownstreamForward::new(
                        dest_mac,
                        MacAddress::new([0; 6]), // server origin
                        payload_data,
                    );
                    if let Err(e) = socket.send_to(&fwd.to_bytes(), addr).await {
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

    /// Return the current OBU → RSU address routing table.
    pub async fn get_obu_routes(&self) -> HashMap<MacAddress, SocketAddr> {
        self.obu_routes.read().await.clone()
    }

    /// Get the IP address of this server.
    pub fn ip(&self) -> Ipv4Addr {
        self.ip
    }

    /// Get the port this server is listening on.
    pub fn port(&self) -> u16 {
        self.port
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[tokio::test]
    async fn test_server_creation() {
        let server = Server::new(Ipv4Addr::new(127, 0, 0, 1), 9999, "test_server".to_string());
        assert_eq!(server.ip(), Ipv4Addr::new(127, 0, 0, 1));
        assert_eq!(server.port(), 9999);
        assert!(server.get_registry().await.is_empty());
        assert!(server.get_obu_routes().await.is_empty());
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

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

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
        let obu_mac: MacAddress = [1u8; 6].into();
        // Payload: [origin_mac 6B][data...]
        let mut payload = obu_mac.bytes().to_vec();
        payload.extend_from_slice(b"test_data");
        let fwd = UpstreamForward::new(rsu_mac, obu_mac, payload);

        let client = UdpSocket::bind("127.0.0.1:0").await?;
        client
            .send_to(&fwd.to_bytes(), format!("127.0.0.1:{}", actual_port))
            .await?;

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let routes = server.get_obu_routes().await;
        assert!(routes.contains_key(&obu_mac));

        Ok(())
    }

    #[tokio::test]
    async fn test_get_obus_for_unknown_rsu_returns_empty() {
        let server = Server::new(Ipv4Addr::new(127, 0, 0, 1), 0, "test_server".to_string());
        let unknown: MacAddress = [9u8; 6].into();
        assert!(server.get_obus_for_rsu(unknown).await.is_empty());
    }
}
