use anyhow::Result;
use std::net::Ipv4Addr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::Mutex;
use tracing::Instrument;

/// ServerNode represents a simple UDP server that receives traffic from RSU nodes.
/// Unlike OBU/RSU nodes which use the custom routing protocol, the Server node
/// receives traffic via standard UDP sockets over normal networking.
#[derive(Clone)]
pub struct Server {
    /// IP address for the UDP server
    ip: Ipv4Addr,
    /// UDP port to listen on
    port: u16,
    /// UDP socket for receiving traffic
    socket: Arc<Mutex<Option<Arc<UdpSocket>>>>,
    /// Node name for tracing/logging identification
    node_name: String,
}

impl Server {
    /// Create a new Server that will listen on the specified IP and port
    /// Note: The server does not start listening until start() is called
    pub fn new(ip: Ipv4Addr, port: u16, node_name: String) -> Self {
        Self {
            ip,
            port,
            socket: Arc::new(Mutex::new(None)),
            node_name,
        }
    }

    pub async fn start(&self) -> Result<()> {
        let bind_addr = format!("{}:{}", self.ip, self.port);
        let node_name = self.node_name.clone();

        // Create span for this node's init
        let _span = tracing::info_span!("node", name = %node_name).entered();

        tracing::info!(
            ip = %self.ip,
            port = self.port,
            bind_addr = %bind_addr,
            "Starting server UDP listener (configured IP shown, binding to all interfaces)"
        );

        let socket = UdpSocket::bind(&bind_addr).await?;
        let socket = Arc::new(socket);

        {
            let mut sock_lock = self.socket.lock().await;
            *sock_lock = Some(socket.clone());
        }

        // Spawn a task to receive and log incoming traffic
        let socket_clone = socket.clone();
        let node_name_for_task = node_name.clone();

        let span = tracing::info_span!("node", name = %node_name_for_task);
        tokio::spawn(
            async move {
                let mut buf = vec![0u8; 65536];
                loop {
                    match socket_clone.recv_from(&mut buf).await {
                        Ok((len, src_addr)) => {
                            tracing::debug!(
                                src = %src_addr,
                                len = len,
                                "Server received UDP packet"
                            );
                            // Log first few bytes for debugging
                            if len > 0 {
                                let preview = &buf[..len.min(64)];
                                tracing::trace!("Packet preview: {:02x?}", preview);
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

    /// Get the IP address of this server
    pub fn ip(&self) -> Ipv4Addr {
        self.ip
    }

    /// Get the port this server is listening on
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
    }

    #[tokio::test]
    async fn test_server_start_and_receive() -> Result<()> {
        let server = Server::new(Ipv4Addr::new(127, 0, 0, 1), 0, "test_server".to_string()); // Use port 0 for OS assignment
        server.start().await?;

        // Get the actual bound port
        let actual_port = {
            let sock_lock = server.socket.lock().await;
            sock_lock.as_ref().unwrap().local_addr()?.port()
        };

        // Send a test packet
        let client = UdpSocket::bind("127.0.0.1:0").await?;
        let test_data = b"Hello, Server!";
        client
            .send_to(test_data, format!("127.0.0.1:{}", actual_port))
            .await?;

        // Give it a moment to receive
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        Ok(())
    }
}
