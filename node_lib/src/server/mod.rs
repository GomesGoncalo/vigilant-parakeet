use anyhow::{anyhow, Result};
use mac_address::MacAddress;
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, RwLock},
};
use tokio::net::UdpSocket;
use tracing::{debug, error, info, warn};

use crate::control::client_cache::ClientCache;

/// Server that handles encrypted traffic from RSUs
pub struct Server {
    /// UDP socket for receiving data from RSUs
    socket: Arc<UdpSocket>,
    /// Cache for MAC address mappings
    cache: Arc<ClientCache>,
    /// Registered RSUs and their addresses
    rsu_addresses: Arc<RwLock<HashMap<MacAddress, SocketAddr>>>,
}

/// Message sent from RSU to Server containing encrypted upstream data
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct RsuToServerMessage {
    /// MAC address of the RSU sending this message (as 6-byte array)
    pub rsu_mac: [u8; 6],
    /// Original encrypted upstream message data
    pub encrypted_data: Vec<u8>,
    /// Source MAC from the original message (as 6-byte array)
    pub original_source: [u8; 6],
}

/// Message sent from Server back to RSUs with decrypted/processed data
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct ServerToRsuMessage {
    /// Decrypted payload data
    pub decrypted_payload: Vec<u8>,
    /// Target RSU MAC addresses to forward to (for broadcast/multicast)
    pub target_rsus: Vec<[u8; 6]>,
    /// Original destination MAC address
    pub destination_mac: [u8; 6],
    /// Original source MAC address
    pub source_mac: [u8; 6],
}

impl Server {
    pub async fn new(bind_addr: SocketAddr) -> Result<Arc<Self>> {
        let socket = UdpSocket::bind(bind_addr).await?;
        info!("Server bound to {}", bind_addr);

        let server = Arc::new(Self {
            socket: Arc::new(socket),
            cache: Arc::new(ClientCache::default()),
            rsu_addresses: Arc::new(RwLock::new(HashMap::new())),
        });

        // Start the server task
        let server_clone = server.clone();
        tokio::spawn(async move {
            if let Err(e) = server_clone.run().await {
                error!("Server error: {:?}", e);
            }
        });

        Ok(server)
    }

    async fn run(&self) -> Result<()> {
        let mut buffer = vec![0u8; 65536]; // Max UDP packet size

        loop {
            match self.socket.recv_from(&mut buffer).await {
                Ok((len, addr)) => {
                    let data = &buffer[..len];
                    if let Err(e) = self.handle_message(data, addr).await {
                        warn!("Error handling message from {}: {:?}", addr, e);
                    }
                }
                Err(e) => {
                    error!("Error receiving UDP packet: {:?}", e);
                    break;
                }
            }
        }
        Ok(())
    }

    async fn handle_message(&self, data: &[u8], from_addr: SocketAddr) -> Result<()> {
        // Deserialize the message from RSU
        let rsu_message: RsuToServerMessage = bincode::deserialize(data)
            .map_err(|e| anyhow!("Failed to deserialize message: {}", e))?;

        debug!(
            "Received message from RSU {:?} at {}",
            rsu_message.rsu_mac, from_addr
        );

        // Register this RSU's address
        self.rsu_addresses
            .write()
            .unwrap()
            .insert(rsu_message.rsu_mac.into(), from_addr);

        // Decrypt the payload
        let decrypted_payload = crate::crypto::decrypt_payload(&rsu_message.encrypted_data)?;

        // Extract MAC addresses from decrypted data (same logic as RSU)
        let to: [u8; 6] = decrypted_payload
            .get(0..6)
            .ok_or_else(|| anyhow!("decrypted frame too short for destination MAC"))?
            .try_into()?;
        let to: MacAddress = to.into();

        let from: [u8; 6] = decrypted_payload
            .get(6..12)
            .ok_or_else(|| anyhow!("decrypted frame too short for source MAC"))?
            .try_into()?;
        let from: MacAddress = from.into();

        // Store MAC mapping
        self.cache
            .store_mac(from, rsu_message.original_source.into());

        let is_broadcast = to == [255; 6].into() || to.bytes()[0] & 0x1 != 0;

        // Determine target RSUs
        let target_rsus: Vec<[u8; 6]> = if is_broadcast {
            // For broadcast, send to all RSUs except the sender
            self.rsu_addresses
                .read()
                .unwrap()
                .keys()
                .filter(|&&rsu_mac| rsu_mac != rsu_message.rsu_mac.into())
                .map(|mac| mac.bytes())
                .collect()
        } else {
            // For unicast, determine which RSU should handle this
            // For now, we'll send it back to all RSUs and let them decide based on their routing
            self.rsu_addresses
                .read()
                .unwrap()
                .keys()
                .map(|mac| mac.bytes())
                .collect()
        };

        // Create response message
        let response = ServerToRsuMessage {
            decrypted_payload,
            target_rsus: target_rsus.clone(),
            destination_mac: to.bytes(),
            source_mac: from.bytes(),
        };

        // Send response to target RSUs
        let rsu_addrs: Vec<(_, SocketAddr)> = {
            let rsu_addresses = self.rsu_addresses.read().unwrap();
            target_rsus
                .iter()
                .filter_map(|target_rsu| {
                    rsu_addresses
                        .get(&MacAddress::from(*target_rsu))
                        .map(|&addr| (*target_rsu, addr))
                })
                .collect()
        };

        for (target_rsu, rsu_addr) in rsu_addrs {
            let serialized = bincode::serialize(&response)
                .map_err(|e| anyhow!("Failed to serialize response: {}", e))?;

            if let Err(e) = self.socket.send_to(&serialized, rsu_addr).await {
                warn!(
                    "Failed to send response to RSU {:?} at {}: {:?}",
                    target_rsu, rsu_addr, e
                );
            }
        }

        Ok(())
    }

    /// Get the local address the server is bound to
    pub fn local_addr(&self) -> Result<SocketAddr> {
        self.socket.local_addr().map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    #[tokio::test]
    async fn server_creation_and_binding() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
        let server = Server::new(addr).await.expect("Failed to create server");

        // Should be able to get the local address
        let local_addr = server.local_addr().expect("Failed to get local address");
        assert_eq!(local_addr.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_ne!(local_addr.port(), 0); // Should have been assigned a port
    }

    #[tokio::test]
    async fn server_message_serialization() {
        let rsu_mac = [1, 2, 3, 4, 5, 6];
        let original_source = [7, 8, 9, 10, 11, 12];
        let encrypted_data = vec![1, 2, 3, 4, 5];

        let message = RsuToServerMessage {
            rsu_mac,
            encrypted_data: encrypted_data.clone(),
            original_source,
        };

        let serialized = bincode::serialize(&message).expect("Failed to serialize");
        let deserialized: RsuToServerMessage =
            bincode::deserialize(&serialized).expect("Failed to deserialize");

        assert_eq!(deserialized.rsu_mac, rsu_mac);
        assert_eq!(deserialized.encrypted_data, encrypted_data);
        assert_eq!(deserialized.original_source, original_source);
    }
}
