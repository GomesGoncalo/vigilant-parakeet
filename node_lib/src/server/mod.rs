use anyhow::{anyhow, Result};
use common::{device::Device, tun::Tun};
use mac_address::MacAddress;
use std::{
    collections::{HashMap, HashSet},
    net::{Ipv4Addr, SocketAddr},
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
    /// Track which OBU MACs are reachable through which RSU
    obu_to_rsu: Arc<RwLock<HashMap<MacAddress, MacAddress>>>,
    /// Track all known OBU MACs per RSU (for broadcast)
    rsu_to_obus: Arc<RwLock<HashMap<MacAddress, HashSet<MacAddress>>>>,
    /// TUN device for network connectivity (so server can be pinged)
    tun: Arc<Tun>,
    /// Device for network interface management
    device: Arc<Device>,
    /// Server IP address
    ip_address: Ipv4Addr,
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

/// Message sent from RSU to Server to register/update connected OBUs
#[derive(Debug, Clone)]
pub struct RsuRegistrationMessage {
    /// MAC address of the RSU sending this message
    pub rsu_mac: MacAddress,
    /// Set of OBU MAC addresses currently connected to this RSU
    pub connected_obus: HashSet<MacAddress>,
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

impl RsuRegistrationMessage {
    pub fn new(rsu_mac: MacAddress, connected_obus: HashSet<MacAddress>) -> Self {
        Self {
            rsu_mac,
            connected_obus,
        }
    }

    /// Convert to wire format using existing Message protocol
    /// We'll use a special control message for registration
    pub fn to_wire(&self) -> Vec<u8> {
        // For now, we'll use a simple format with upstream data containing registration info
        // In a real implementation, we might want a dedicated control message type
        let mut data = Vec::new();
        data.extend_from_slice(&[0xFF]); // Magic byte to indicate registration
        data.extend_from_slice(&(self.connected_obus.len() as u32).to_be_bytes());
        for obu_mac in &self.connected_obus {
            data.extend_from_slice(&obu_mac.bytes());
        }

        let upstream = ToUpstream::new(self.rsu_mac, &data);
        let data_msg = Data::Upstream(upstream);
        let packet_type = PacketType::Data(data_msg);

        // Use special destination for registration messages
        let msg = Message::new(
            self.rsu_mac,
            MacAddress::from([0xFE; 6]), // Special registration destination
            packet_type,
        );
        let parts: Vec<Vec<u8>> = (&msg).into();
        parts.into_iter().flatten().collect()
    }

    /// Parse from wire format
    pub fn from_wire(data: &[u8], rsu_mac: MacAddress) -> Result<Self> {
        let msg = Message::try_from(data)?;
        
        // Check if this is a registration message by destination
        if msg.to()? != MacAddress::from([0xFE; 6]) {
            return Err(anyhow!("Not a registration message"));
        }

        match msg.get_packet_type() {
            PacketType::Data(Data::Upstream(upstream)) => {
                let reg_data = upstream.data();
                if reg_data.is_empty() || reg_data[0] != 0xFF {
                    return Err(anyhow!("Invalid registration data format"));
                }

                let num_obus = u32::from_be_bytes(
                    reg_data.get(1..5)
                        .ok_or_else(|| anyhow!("Invalid OBU count"))?
                        .try_into()
                        .map_err(|_| anyhow!("Invalid OBU count format"))?
                ) as usize;

                let mut connected_obus = HashSet::new();
                let mut offset = 5;
                for _ in 0..num_obus {
                    if offset + 6 > reg_data.len() {
                        return Err(anyhow!("Truncated registration data"));
                    }
                    let obu_bytes: [u8; 6] = reg_data[offset..offset + 6]
                        .try_into()
                        .map_err(|_| anyhow!("Invalid OBU MAC"))?;
                    connected_obus.insert(MacAddress::from(obu_bytes));
                    offset += 6;
                }

                Ok(Self {
                    rsu_mac,
                    connected_obus,
                })
            }
            _ => Err(anyhow!("Invalid message type for RSU registration")),
        }
    }
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
    pub async fn new(bind_addr: SocketAddr, server_ip: Ipv4Addr, tun: Arc<Tun>, device: Arc<Device>) -> Result<Arc<Self>> {
        let socket = UdpSocket::bind(bind_addr).await?;
        info!("Server bound to {} with IP {}", bind_addr, server_ip);

        let server = Arc::new(Self {
            socket: Arc::new(socket),
            cache: Arc::new(ClientCache::default()),
            rsu_addresses: Arc::new(RwLock::new(HashMap::new())),
            obu_to_rsu: Arc::new(RwLock::new(HashMap::new())),
            rsu_to_obus: Arc::new(RwLock::new(HashMap::new())),
            tun,
            device,
            ip_address: server_ip,
        });

        // Start the server task
        let server_clone = server.clone();
        tokio::spawn(async move {
            if let Err(e) = server_clone.run().await {
                error!("Server error: {:?}", e);
            }
        });

        // Start the network interface task for handling ICMP and other IP traffic
        let server_clone2 = server.clone();
        tokio::spawn(async move {
            if let Err(e) = server_clone2.run_network_interface().await {
                error!("Server network interface error: {:?}", e);
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

    /// Handle network interface traffic (ICMP pings, etc.)
    async fn run_network_interface(&self) -> Result<()> {
        let mut buffer = [0u8; 2048];

        info!("Server network interface listening for IP traffic on {}", self.ip_address);

        loop {
            match self.tun.recv(&mut buffer).await {
                Ok(n) => {
                    if n == 0 {
                        debug!("TUN interface closed");
                        break;
                    }

                    let packet = &buffer[..n];
                    debug!("Server received {} bytes on TUN interface", n);
                    
                    // Handle IP packets - specifically ICMP ping requests
                    if let Err(e) = self.handle_ip_packet(packet).await {
                        debug!("Error handling IP packet: {:?}", e);
                    }
                }
                Err(e) => {
                    error!("Error reading from TUN interface: {:?}", e);
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                }
            }
        }
        Ok(())
    }

    /// Handle IP packets received on the TUN interface
    async fn handle_ip_packet(&self, packet: &[u8]) -> Result<()> {
        // Basic IP header parsing
        if packet.len() < 20 {
            return Ok(()); // Too short for IP header
        }

        let ip_version = (packet[0] >> 4) & 0xF;
        if ip_version != 4 {
            return Ok(()); // Only handle IPv4
        }

        let protocol = packet[9];
        if protocol != 1 {
            return Ok(()); // Only handle ICMP (protocol 1)
        }

        let src_ip = u32::from_be_bytes([packet[12], packet[13], packet[14], packet[15]]);
        let dst_ip = u32::from_be_bytes([packet[16], packet[17], packet[18], packet[19]]);

        // Check if this is for our server IP
        if Ipv4Addr::from(dst_ip) != self.ip_address {
            return Ok(());
        }

        let header_len = ((packet[0] & 0xF) * 4) as usize;
        if packet.len() < header_len + 8 {
            return Ok(()); // Too short for ICMP
        }

        let icmp_type = packet[header_len];
        let icmp_code = packet[header_len + 1];

        // Handle ICMP Echo Request (ping)
        if icmp_type == 8 && icmp_code == 0 {
            debug!("Server received ping from {}", Ipv4Addr::from(src_ip));
            
            // Create ICMP Echo Reply
            let mut reply = packet.to_vec();
            
            // Swap source and destination IPs
            reply[12..16].copy_from_slice(&dst_ip.to_be_bytes());
            reply[16..20].copy_from_slice(&src_ip.to_be_bytes());
            
            // Change ICMP type to Echo Reply (0)
            reply[header_len] = 0;
            
            // Recalculate IP header checksum
            reply[10] = 0;
            reply[11] = 0;
            let ip_checksum = self.calculate_ip_checksum(&reply[..header_len]);
            reply[10..12].copy_from_slice(&ip_checksum.to_be_bytes());
            
            // Recalculate ICMP checksum
            reply[header_len + 2] = 0;
            reply[header_len + 3] = 0;
            let icmp_checksum = self.calculate_icmp_checksum(&reply[header_len..]);
            reply[header_len + 2..header_len + 4].copy_from_slice(&icmp_checksum.to_be_bytes());
            
            // Send reply
            if let Err(e) = self.tun.send_all(&reply).await {
                warn!("Failed to send ping reply: {:?}", e);
            } else {
                debug!("Server sent ping reply to {}", Ipv4Addr::from(src_ip));
            }
        }

        Ok(())
    }

    /// Calculate IP header checksum
    fn calculate_ip_checksum(&self, header: &[u8]) -> u16 {
        let mut sum = 0u32;
        
        for chunk in header.chunks(2) {
            if chunk.len() == 2 {
                sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
            } else {
                sum += (chunk[0] as u32) << 8;
            }
        }
        
        while sum >> 16 != 0 {
            sum = (sum & 0xFFFF) + (sum >> 16);
        }
        
        !sum as u16
    }

    /// Calculate ICMP checksum
    fn calculate_icmp_checksum(&self, icmp_data: &[u8]) -> u16 {
        let mut sum = 0u32;
        
        for chunk in icmp_data.chunks(2) {
            if chunk.len() == 2 {
                sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
            } else {
                sum += (chunk[0] as u32) << 8;
            }
        }
        
        while sum >> 16 != 0 {
            sum = (sum & 0xFFFF) + (sum >> 16);
        }
        
        !sum as u16
    }

    async fn handle_message(&self, data: &[u8], from_addr: SocketAddr) -> Result<()> {
        // First try to parse as a registration message
        if let Ok(registration) = self.try_parse_registration(data, from_addr).await {
            return Ok(registration);
        }

        // Parse as regular RSU to server message
        let rsu_mac = self
            .rsu_addresses
            .read()
            .unwrap()
            .iter()
            .find(|(_, &addr)| addr == from_addr)
            .map(|(&mac, _)| mac)
            .ok_or_else(|| anyhow!("Unknown RSU - must register first"))?;

        let rsu_message = RsuToServerMessage::from_wire(data, rsu_mac)?;

        debug!(
            "Received traffic message from RSU {:?} at {}",
            rsu_message.rsu_mac, from_addr
        );

        // Verify RSU is registered
        if rsu_message.rsu_mac != rsu_mac {
            return Err(anyhow!("RSU MAC mismatch"));
        }

        // Update OBU tracking - the source of this message is connected to this RSU
        self.obu_to_rsu
            .write()
            .unwrap()
            .insert(rsu_message.original_source, rsu_message.rsu_mac);

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

        // Determine target RSUs based on proper routing
        let target_rsus: Vec<MacAddress> = if is_multicast {
            // For multicast/broadcast, send to all RSUs that have connected OBUs
            let rsu_to_obus = self.rsu_to_obus.read().unwrap();
            self.rsu_addresses
                .read()
                .unwrap()
                .keys()
                .filter(|&&rsu_mac| {
                    // Don't send back to the sender RSU
                    if rsu_mac == rsu_message.rsu_mac {
                        return false;
                    }
                    // Only send to RSUs that have connected OBUs
                    rsu_to_obus.get(&rsu_mac).map_or(false, |obus| !obus.is_empty())
                })
                .copied()
                .collect()
        } else {
            // For unicast, find which RSU the destination OBU is connected to
            let obu_to_rsu = self.obu_to_rsu.read().unwrap();
            if let Some(&target_rsu) = obu_to_rsu.get(&to) {
                vec![target_rsu]
            } else {
                // If we don't know where the destination is, send to all RSUs except sender
                // This allows the RSU network to discover the destination
                warn!("Unknown destination OBU {:?}, broadcasting to all RSUs", to);
                self.rsu_addresses
                    .read()
                    .unwrap()
                    .keys()
                    .filter(|&&rsu_mac| rsu_mac != rsu_message.rsu_mac)
                    .copied()
                    .collect()
            }
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

    /// Try to parse incoming data as an RSU registration message
    pub async fn try_parse_registration(&self, data: &[u8], from_addr: SocketAddr) -> Result<()> {
        // First try to extract RSU MAC from existing registrations
        let rsu_mac = if let Some((&mac, _)) = self
            .rsu_addresses
            .read()
            .unwrap()
            .iter()
            .find(|(_, &addr)| addr == from_addr)
        {
            mac
        } else {
            // For new RSUs, we need to parse the message to get the MAC
            let msg = Message::try_from(data)?;
            if msg.to()? != MacAddress::from([0xFE; 6]) {
                return Err(anyhow!("Not a registration message"));
            }
            msg.from()?
        };

        let registration = RsuRegistrationMessage::from_wire(data, rsu_mac)?;

        info!(
            "RSU {:?} registered from {} with {} connected OBUs",
            registration.rsu_mac,
            from_addr,
            registration.connected_obus.len()
        );

        // Register the RSU's address
        self.rsu_addresses
            .write()
            .unwrap()
            .insert(registration.rsu_mac, from_addr);

        // Update OBU tracking
        {
            let mut obu_to_rsu = self.obu_to_rsu.write().unwrap();
            let mut rsu_to_obus = self.rsu_to_obus.write().unwrap();

            // Remove old mappings for this RSU
            obu_to_rsu.retain(|_, &mut rsu| rsu != registration.rsu_mac);

            // Add new mappings
            for obu_mac in &registration.connected_obus {
                obu_to_rsu.insert(*obu_mac, registration.rsu_mac);
            }

            // Update RSU to OBUs mapping
            rsu_to_obus.insert(registration.rsu_mac, registration.connected_obus.clone());
        }

        debug!(
            "Updated OBU mappings for RSU {:?}: {:?}",
            registration.rsu_mac, registration.connected_obus
        );

        Ok(())
    }

    /// Get all registered RSUs
    pub fn get_registered_rsus(&self) -> Vec<MacAddress> {
        self.rsu_addresses.read().unwrap().keys().copied().collect()
    }

    /// Get OBUs connected to a specific RSU
    pub fn get_obus_for_rsu(&self, rsu_mac: MacAddress) -> HashSet<MacAddress> {
        self.rsu_to_obus
            .read()
            .unwrap()
            .get(&rsu_mac)
            .cloned()
            .unwrap_or_default()
    }

    /// Get which RSU an OBU is connected to
    pub fn get_rsu_for_obu(&self, obu_mac: MacAddress) -> Option<MacAddress> {
        self.obu_to_rsu.read().unwrap().get(&obu_mac).copied()
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
    use crate::test_helpers::util::mk_shim_pair;

    /// Helper to create a test server with TUN device
    async fn create_test_server(addr: SocketAddr) -> Arc<Server> {
        let server_ip = Ipv4Addr::new(10, 0, 255, 1);
        let (tun, _peer) = mk_shim_pair();
        let device = crate::test_helpers::util::make_test_device([0xFF; 6].into());
        Server::new(addr, server_ip, Arc::new(tun), Arc::new(device))
            .await
            .expect("Failed to create test server")
    }

    #[tokio::test]
    async fn server_creation_and_binding() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
        let server = create_test_server(addr).await;

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

    #[tokio::test]
    async fn server_registration_message_serialization() {
        let rsu_mac = MacAddress::from([1, 2, 3, 4, 5, 6]);
        let mut connected_obus = HashSet::new();
        connected_obus.insert(MacAddress::from([10, 11, 12, 13, 14, 15]));
        connected_obus.insert(MacAddress::from([20, 21, 22, 23, 24, 25]));

        let registration = RsuRegistrationMessage {
            rsu_mac,
            connected_obus: connected_obus.clone(),
        };

        let wire_data = registration.to_wire();
        let deserialized = RsuRegistrationMessage::from_wire(&wire_data, rsu_mac)
            .expect("Failed to deserialize registration");

        assert_eq!(deserialized.rsu_mac, rsu_mac);
        assert_eq!(deserialized.connected_obus, connected_obus);
    }

    #[tokio::test]
    async fn server_tracks_rsu_registrations() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
        let server = create_test_server(addr).await;

        // Initially no RSUs registered
        assert!(server.get_registered_rsus().is_empty());

        // Simulate RSU registration by calling try_parse_registration directly
        let rsu_mac = MacAddress::from([1, 2, 3, 4, 5, 6]);
        let mut connected_obus = HashSet::new();
        connected_obus.insert(MacAddress::from([10, 11, 12, 13, 14, 15]));

        let registration = RsuRegistrationMessage {
            rsu_mac,
            connected_obus: connected_obus.clone(),
        };

        let wire_data = registration.to_wire();
        let from_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 12345);

        // Register the RSU
        server
            .try_parse_registration(&wire_data, from_addr)
            .await
            .expect("Failed to parse registration");

        // Verify RSU is registered
        let registered_rsus = server.get_registered_rsus();
        assert_eq!(registered_rsus.len(), 1);
        assert!(registered_rsus.contains(&rsu_mac));

        // Verify OBU mappings
        assert_eq!(server.get_obus_for_rsu(rsu_mac), connected_obus);
        
        for obu_mac in &connected_obus {
            assert_eq!(server.get_rsu_for_obu(*obu_mac), Some(rsu_mac));
        }
    }

    #[tokio::test]
    async fn server_handles_obu_tracking_updates() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
        let server = create_test_server(addr).await;

        let rsu_mac = MacAddress::from([1, 2, 3, 4, 5, 6]);
        let from_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 12345);

        // Initial registration with one OBU
        let mut connected_obus = HashSet::new();
        let obu1 = MacAddress::from([10, 11, 12, 13, 14, 15]);
        connected_obus.insert(obu1);

        let registration = RsuRegistrationMessage {
            rsu_mac,
            connected_obus: connected_obus.clone(),
        };

        server
            .try_parse_registration(&registration.to_wire(), from_addr)
            .await
            .expect("Failed to parse initial registration");

        assert_eq!(server.get_obus_for_rsu(rsu_mac), connected_obus);
        assert_eq!(server.get_rsu_for_obu(obu1), Some(rsu_mac));

        // Update registration with different OBUs
        let mut new_connected_obus = HashSet::new();
        let obu2 = MacAddress::from([20, 21, 22, 23, 24, 25]);
        let obu3 = MacAddress::from([30, 31, 32, 33, 34, 35]);
        new_connected_obus.insert(obu2);
        new_connected_obus.insert(obu3);

        let updated_registration = RsuRegistrationMessage {
            rsu_mac,
            connected_obus: new_connected_obus.clone(),
        };

        server
            .try_parse_registration(&updated_registration.to_wire(), from_addr)
            .await
            .expect("Failed to parse updated registration");

        // Verify old OBU is no longer mapped to this RSU
        assert_eq!(server.get_rsu_for_obu(obu1), None);

        // Verify new OBUs are mapped
        assert_eq!(server.get_obus_for_rsu(rsu_mac), new_connected_obus);
        assert_eq!(server.get_rsu_for_obu(obu2), Some(rsu_mac));
        assert_eq!(server.get_rsu_for_obu(obu3), Some(rsu_mac));
    }
}
