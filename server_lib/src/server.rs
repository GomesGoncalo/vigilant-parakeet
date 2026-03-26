use crate::registry::RegistrationMessage;
use anyhow::Result;
use mac_address::MacAddress;
use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::{Mutex, RwLock};
use tracing::Instrument;

/// ServerNode receives traffic from RSU nodes over UDP via the cloud interface.
///
/// Unlike OBU/RSU nodes which use the custom VANET routing protocol, the Server
/// communicates with RSUs via standard UDP sockets over dedicated cloud interfaces
/// (simulating an internet connection between RSUs and infrastructure).
///
/// The server maintains a registry of which OBUs are associated with each RSU,
/// updated as RSUs send periodic `RegistrationMessage` packets.
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
            node_name,
        }
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

        let socket_clone = socket.clone();
        let registry = self.registry.clone();
        let node_name_for_task = node_name.clone();

        let span = tracing::info_span!("node", name = %node_name_for_task);
        tokio::spawn(
            async move {
                let mut buf = vec![0u8; 65536];
                loop {
                    match socket_clone.recv_from(&mut buf).await {
                        Ok((len, src_addr)) => {
                            if let Some(msg) = RegistrationMessage::try_from_bytes(&buf[..len]) {
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
                            } else {
                                tracing::debug!(
                                    src = %src_addr,
                                    len = len,
                                    "Received unrecognised UDP packet"
                                );
                            }
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "Error receiving UDP packet");
                        }
                    }
                }
            }
            .instrument(span),
        );

        Ok(())
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
    async fn test_get_obus_for_unknown_rsu_returns_empty() {
        let server = Server::new(Ipv4Addr::new(127, 0, 0, 1), 0, "test_server".to_string());
        let unknown: MacAddress = [9u8; 6].into();
        assert!(server.get_obus_for_rsu(unknown).await.is_empty());
    }
}
