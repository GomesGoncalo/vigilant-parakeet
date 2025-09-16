use crate::args::ServerArgs;
use anyhow::Result;
use common::device::Device;
use common::tun::Tun;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tracing::{debug, info, error};

pub struct Server {
    _device: Arc<Device>,
    _tun: Arc<Tun>,
    args: ServerArgs,
    socket: Option<UdpSocket>,
}

impl Server {
    pub fn new(args: ServerArgs, tun: Arc<Tun>, device: Arc<Device>) -> Result<Arc<Self>> {
        let server = Server {
            _device: device,
            _tun: tun,
            args,
            socket: None,
        };
        Ok(Arc::new(server))
    }
    
    pub async fn start(&mut self) -> Result<()> {
        // Get the IP address from args
        let ip = self.args.ip.ok_or_else(|| {
            anyhow::anyhow!("Server requires an IP address")
        })?;
        
        let bind_addr = format!("{}:{}", ip, self.args.server_params.bind_port);
        
        debug!("Server binding to {}", bind_addr);
        
        // Create and bind UDP socket
        let socket = UdpSocket::bind(&bind_addr).await?;
        
        info!("Server bound to UDP socket at {}", bind_addr);
        
        self.socket = Some(socket);
        
        // Start the server loop
        self.run_server().await?;
        
        Ok(())
    }
    
    async fn run_server(&self) -> Result<()> {
        let socket = self.socket.as_ref().ok_or_else(|| {
            anyhow::anyhow!("Socket not bound")
        })?;
        
        let mut buffer = [0u8; 1024];
        
        loop {
            match socket.recv_from(&mut buffer).await {
                Ok((size, addr)) => {
                    debug!("Received {} bytes from {}", size, addr);
                    
                    // Echo back the received data as a simple server response
                    if let Err(e) = socket.send_to(&buffer[..size], addr).await {
                        error!("Failed to send response to {}: {}", addr, e);
                    } else {
                        debug!("Echoed {} bytes back to {}", size, addr);
                    }
                },
                Err(e) => {
                    error!("Failed to receive UDP packet: {}", e);
                    return Err(e.into());
                }
            }
        }
    }
}