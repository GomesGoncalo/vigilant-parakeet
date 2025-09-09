use anyhow::{anyhow, Result};
use mac_address::MacAddress;
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, RwLock},
};
use tokio::net::UdpSocket;
use tracing::{debug, error, info, warn};

use crate::{
    control::client_cache::ClientCache,
    messages::{
        data::{Data, ToDownstream, ToUpstream},
        message::Message,
        packet_type::PacketType,
    },
};

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
#[derive(Debug, Clone)]
pub struct RsuToServerMessage {
    /// MAC address of the RSU sending this message
    pub rsu_mac: MacAddress,
    /// Original encrypted upstream message data
    pub encrypted_data: Vec<u8>,
    /// Source MAC from the original message
    pub original_source: MacAddress,
}

/// Message sent from Server back to RSUs with decrypted/processed data
#[derive(Debug, Clone)]
pub struct ServerToRsuMessage {
    /// Encrypted payload data (re-encrypted by server)
    pub encrypted_payload: Vec<u8>,
    /// Target RSU MAC addresses to forward to (for broadcast/multicast)
    pub target_rsus: Vec<MacAddress>,
    /// Original destination MAC address
    pub destination_mac: MacAddress,
    /// Original source MAC address
    pub source_mac: MacAddress,
}

impl RsuToServerMessage {
    pub fn new(rsu_mac: MacAddress, encrypted_data: Vec<u8>, original_source: MacAddress) -> Self {
        Self {
            rsu_mac,
            encrypted_data,
            original_source,
        }
    }

    /// Convert to wire format using existing Message protocol
    pub fn to_wire(&self) -> Vec<u8> {
        // Create upstream data message with RSU MAC as origin
        let upstream = ToUpstream::new(self.rsu_mac, &self.encrypted_data);
        let data = Data::Upstream(upstream);
        let packet_type = PacketType::Data(data);

        // Create message from original source to server (using broadcast address as placeholder)
        let msg = Message::new(
            self.original_source,
            MacAddress::from([255; 6]),
            packet_type,
        );
        let parts: Vec<Vec<u8>> = (&msg).into();
        parts.into_iter().flatten().collect()
    }

    /// Parse from wire format
    pub fn from_wire(data: &[u8], rsu_mac: MacAddress) -> Result<Self> {
        let msg = Message::try_from(data)?;
        match msg.get_packet_type() {
            PacketType::Data(Data::Upstream(upstream)) => Ok(Self {
                rsu_mac,
                encrypted_data: upstream.data().to_vec(),
                original_source: msg.from()?,
            }),
            _ => Err(anyhow!("Invalid message type for RSU to server")),
        }
    }
}

impl ServerToRsuMessage {
    pub fn new(
        encrypted_payload: Vec<u8>,
        target_rsus: Vec<MacAddress>,
        destination_mac: MacAddress,
        source_mac: MacAddress,
    ) -> Self {
        Self {
            encrypted_payload,
            target_rsus,
            destination_mac,
            source_mac,
        }
    }

    /// Convert to wire format using existing Message protocol
    pub fn to_wire(&self) -> Vec<u8> {
        // Create downstream data message
        let source_bytes = self.source_mac.bytes();
        let downstream =
            ToDownstream::new(&source_bytes, self.destination_mac, &self.encrypted_payload);
        let data = Data::Downstream(downstream);
        let packet_type = PacketType::Data(data);

        // Create message from server (using broadcast) to destination
        let msg = Message::new(
            MacAddress::from([255; 6]),
            self.destination_mac,
            packet_type,
        );
        let parts: Vec<Vec<u8>> = (&msg).into();
        parts.into_iter().flatten().collect()
    }

    /// Parse from wire format
    pub fn from_wire(data: &[u8]) -> Result<Self> {
        let msg = Message::try_from(data)?;
        match msg.get_packet_type() {
            PacketType::Data(Data::Downstream(downstream)) => {
                let source_bytes: [u8; 6] = downstream
                    .source()
                    .as_ref()
                    .try_into()
                    .map_err(|_| anyhow!("Invalid source MAC"))?;
                Ok(Self {
                    encrypted_payload: downstream.data().to_vec(),
                    target_rsus: vec![], // Will be filled separately based on routing
                    destination_mac: msg.to()?,
                    source_mac: MacAddress::from(source_bytes),
                })
            }
            _ => Err(anyhow!("Invalid message type for server to RSU")),
        }
    }
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
        // Parse the message from RSU using the wire format
        // We need to extract the RSU MAC from the socket address mapping
        let rsu_mac = self
            .rsu_addresses
            .read()
            .unwrap()
            .iter()
            .find(|(_, &addr)| addr == from_addr)
            .map(|(&mac, _)| mac)
            .unwrap_or_else(|| {
                // For new RSUs, we'll use a placeholder and register them
                MacAddress::from([0; 6])
            });

        let rsu_message = RsuToServerMessage::from_wire(data, rsu_mac)?;

        debug!(
            "Received message from RSU {:?} at {}",
            rsu_message.rsu_mac, from_addr
        );

        // Register this RSU's address
        self.rsu_addresses
            .write()
            .unwrap()
            .insert(rsu_message.rsu_mac, from_addr);

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
        self.cache.store_mac(from, rsu_message.original_source);

        let is_multicast = to.bytes()[0] & 0x1 != 0;

        // Determine target RSUs
        let target_rsus: Vec<MacAddress> = if is_multicast {
            // For multicast, send to all RSUs except the sender
            self.rsu_addresses
                .read()
                .unwrap()
                .keys()
                .filter(|&&rsu_mac| rsu_mac != rsu_message.rsu_mac)
                .copied()
                .collect()
        } else {
            // For unicast, determine which RSU should handle this
            // For now, we'll send it back to all RSUs and let them decide based on their routing
            self.rsu_addresses.read().unwrap().keys().copied().collect()
        };

        // Re-encrypt the payload for sending back to RSUs
        let encrypted_payload = crate::crypto::encrypt_payload(&decrypted_payload)?;

        // Create response message
        let response = ServerToRsuMessage {
            encrypted_payload,
            target_rsus: target_rsus.clone(),
            destination_mac: to,
            source_mac: from,
        };

        // Send response to target RSUs
        let rsu_addrs: Vec<(_, SocketAddr)> = {
            let rsu_addresses = self.rsu_addresses.read().unwrap();
            target_rsus
                .iter()
                .filter_map(|target_rsu| {
                    rsu_addresses
                        .get(target_rsu)
                        .map(|&addr| (*target_rsu, addr))
                })
                .collect()
        };

        for (target_rsu, rsu_addr) in rsu_addrs {
            let wire_data = response.to_wire();

            if let Err(e) = self.socket.send_to(&wire_data, rsu_addr).await {
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
        let rsu_mac = MacAddress::from([1, 2, 3, 4, 5, 6]);
        let original_source = MacAddress::from([7, 8, 9, 10, 11, 12]);
        let encrypted_data = vec![1, 2, 3, 4, 5];

        let message = RsuToServerMessage {
            rsu_mac,
            encrypted_data: encrypted_data.clone(),
            original_source,
        };

        let wire_data = message.to_wire();
        let deserialized =
            RsuToServerMessage::from_wire(&wire_data, rsu_mac).expect("Failed to deserialize");

        assert_eq!(deserialized.rsu_mac, rsu_mac);
        assert_eq!(deserialized.encrypted_data, encrypted_data);
        assert_eq!(deserialized.original_source, original_source);
    }
}
