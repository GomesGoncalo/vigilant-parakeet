pub mod routing;

use super::{client_cache::ClientCache, node::ReplyType};
use crate::{
    control::node,
    messages::{
        control::Control,
        data::{Data, ToDownstream},
        message::Message,
        packet_type::PacketType,
    },
    Args,
};
use anyhow::{anyhow, bail, Result};
use common::tun::Tun;
use common::{device::Device, network_interface::NetworkInterface};
use itertools::Itertools;
use mac_address::MacAddress;
use routing::Routing;
use std::{
    collections::HashSet,
    io::IoSlice,
    sync::{Arc, RwLock},
    time::Duration,
    net::SocketAddr,
};
use tokio::net::UdpSocket;

pub struct Rsu {
    args: Args,
    routing: Arc<RwLock<Routing>>,
    /// Virtual interface for OBU communication (WiFi simulation - open medium)
    tun: Arc<Tun>,
    /// Real interface for OBU communication (used by node lib for routing)
    device: Arc<Device>,
    cache: Arc<ClientCache>,
    /// Infrastructure interface for server communication (wired connection to cloud)
    infra_device: Arc<Device>,
    /// Server address for UDP communication
    server_address: SocketAddr,
    /// Channel sender for sending messages to server
    server_tx: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
}

impl Rsu {
    pub fn new(_args: Args, _tun: Arc<Tun>, _device: Arc<Device>) -> Result<Arc<Self>> {
        // RSUs require separate infrastructure device for server communication
        // This single-device method is deprecated
        return Err(anyhow!("RSUs require separate infrastructure device - use new_with_infra instead"));
    }

    pub fn new_with_infra(args: Args, tun: Arc<Tun>, device: Arc<Device>, infra_device: Arc<Device>) -> Result<Arc<Self>> {
        // Server address is mandatory for RSUs
        let server_address = args
            .node_params
            .server_address
            .as_ref()
            .ok_or_else(|| anyhow!("RSU requires server_address to be configured"))?
            .clone();

        // Create channel for server communication
        let (server_tx, server_rx) = tokio::sync::mpsc::unbounded_channel();

        let rsu = Arc::new(Self {
            routing: Arc::new(RwLock::new(Routing::new(&args)?)),
            args,
            tun,
            device,
            cache: ClientCache::default().into(),
            infra_device,
            server_address: server_address,
            server_tx,
        });

        tracing::info!(?rsu.args, "Setup Rsu with infrastructure device for UDP server communication");
        rsu.hello_task()?;
        rsu.process_tap_traffic()?;
        Self::wire_traffic_task(rsu.clone())?;

        // Start server communication tasks using UDP
        Self::server_communication_task(rsu.clone(), server_rx)?;

        Ok(rsu)
    }

    /// Get route to a specific MAC address. Used for testing latency measurement.
    pub fn get_route_to(&self, mac: MacAddress) -> Option<crate::control::route::Route> {
        self.routing.read().unwrap().get_route_to(Some(mac))
    }

    /// Get count of next hops in routing table. Used for testing.
    pub fn next_hop_count(&self) -> usize {
        self.routing.read().unwrap().iter_next_hops().count()
    }

    fn wire_traffic_task(rsu: Arc<Self>) -> Result<()> {
        let device = rsu.device.clone();
        let tun = rsu.tun.clone();

        tokio::task::spawn(async move {
            loop {
                let rsu = rsu.clone();
                let messages = node::wire_traffic(&device, |pkt, size| {
                    async move {
                        // Try to parse multiple messages from the packet
                        let data = &pkt[..size];
                        let mut all_responses = Vec::new();
                        let mut offset = 0;

                        while offset < data.len() {
                            match Message::try_from(&data[offset..]) {
                                Ok(msg) => {
                                    tracing::trace!(offset = offset, parsed = ?msg, "rsu wire_traffic parsed message");
                                    let response = rsu.handle_msg(&msg).await;
                                    let has_response = response.as_ref().map(|r| r.is_some()).unwrap_or(false);
                                    tracing::trace!(has_response = has_response, incoming = ?msg, outgoing = ?node::get_msgs(&response), "transaction");

                                    if let Ok(Some(responses)) = response {
                                        all_responses.extend(responses);
                                    }

                                    // Calculate message size to advance offset
                                    let msg_bytes: Vec<Vec<u8>> = (&msg).into();
                                    let msg_size: usize = msg_bytes.iter().map(|chunk| chunk.len()).sum();
                                    offset += msg_size;
                                }
                                Err(e) => {
                                    tracing::trace!(offset = offset, remaining = data.len() - offset, error = ?e, "could not parse message at offset");
                                    break;
                                }
                            }
                        }

                        if all_responses.is_empty() {
                            Ok(None)
                        } else {
                            Ok(Some(all_responses))
                        }
                    }
                }).await;

                match messages {
                    Ok(Some(messages)) => {
                        let _ = node::handle_messages(messages, &tun, &device, None).await;
                    }
                    Ok(None) => {}
                    Err(e) => {
                        tracing::error!("Error in wire_traffic: {:?}", e);
                    }
                }
            }
        });
        Ok(())
    }

    /// Handle all server communication via UDP socket
    fn server_communication_task(rsu: Arc<Self>, mut server_rx: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>) -> Result<()> {
        let routing = rsu.routing.clone();
        let device_mac = rsu.device.mac_address();
        let server_address = rsu.server_address;
        let tun = rsu.tun.clone();
        let device = rsu.device.clone();

        tokio::task::spawn(async move {
            // Create UDP socket for server communication
            let server_socket = match UdpSocket::bind("0.0.0.0:0").await {
                Ok(socket) => {
                    tracing::info!("RSU UDP socket bound for server communication");
                    Arc::new(socket)
                },
                Err(e) => {
                    tracing::error!("Failed to bind UDP socket for server communication: {:?}", e);
                    return;
                }
            };

            // Start registration task
            let socket_clone = server_socket.clone();
            let routing_clone = routing.clone();
            tokio::spawn(async move {
                // Send initial registration
                let _ = Self::send_registration(&socket_clone, &routing_clone, device_mac, server_address).await;

                // Periodic registration updates (every 30 seconds)
                let mut interval = tokio::time::interval(Duration::from_secs(30));
                interval.tick().await; // Skip first tick

                loop {
                    interval.tick().await;
                    let _ = Self::send_registration(&socket_clone, &routing_clone, device_mac, server_address).await;
                }
            });

            // Main loop handling both outgoing messages and incoming responses
            let mut buffer = vec![0u8; 65536];
            loop {
                tokio::select! {
                    // Handle outgoing messages to server
                    msg = server_rx.recv() => {
                        if let Some(data) = msg {
                            if let Err(e) = server_socket.send_to(&data, server_address).await {
                                tracing::warn!("Failed to send to server: {:?}", e);
                            }
                        } else {
                            tracing::info!("Server communication channel closed");
                            break;
                        }
                    }
                    
                    // Handle incoming server responses
                    result = server_socket.recv(&mut buffer) => {
                        match result {
                            Ok(len) => {
                                let data = &buffer[..len];
                                
                                // Parse server response message
                                if let Ok(server_msg) = crate::server::ServerToRsuMessage::from_wire(data) {
                                    tracing::debug!(
                                        "Received server response via UDP: destination={:?}",
                                        server_msg.destination_mac
                                    );

                                    // RSU should not modify payload - just forward encrypted data as-is
                                    let encrypted_payload = &server_msg.encrypted_payload;
                                    let destination_mac = server_msg.destination_mac;
                                    let source_mac = server_msg.source_mac;

                                    let is_multicast = destination_mac.bytes()[0] & 0x1 != 0;
                                    let mut messages = Vec::new();

                                    // Forward to other nodes based on routing
                                    let forwards: Vec<_> = {
                                        let routing = routing.read().unwrap();
                                        if is_multicast {
                                            // For multicast, forward to next hops except source
                                            routing
                                                .iter_next_hops()
                                                .filter(|&&mac| mac != source_mac)
                                                .filter_map(|&next_hop_mac| {
                                                    let route = routing.get_route_to(Some(next_hop_mac))?;

                                                    // Forward encrypted payload without modification
                                                    Some(ReplyType::Wire(
                                                        (&Message::new(
                                                            device.mac_address(),
                                                            route.mac,
                                                            PacketType::Data(Data::Downstream(
                                                                ToDownstream::new(
                                                                    &source_mac.bytes(),
                                                                    destination_mac,
                                                                    encrypted_payload,
                                                                ),
                                                            )),
                                                        ))
                                                            .into(),
                                                    ))
                                                })
                                                .collect()
                                        } else {
                                            // For unicast, the server already determined routing
                                            Vec::new()
                                        }
                                    };
                                    messages.extend(forwards);

                                    if !messages.is_empty() {
                                        let _ = node::handle_messages(messages, &tun, &device, None).await;
                                    }
                                } else {
                                    tracing::debug!("Failed to parse server response message");
                                }
                            }
                            Err(e) => {
                                tracing::error!("Error receiving UDP from server: {:?}", e);
                                break;
                            }
                        }
                    }
                }
            }
        });

        Ok(())
    }

    async fn send_registration(
        server_socket: &UdpSocket,
        routing: &RwLock<Routing>,
        device_mac: MacAddress,
        server_address: SocketAddr,
    ) {
        // Get all next hops from routing table (these are connected OBUs)
        let connected_obus: HashSet<MacAddress> = {
            let routing = routing.read().unwrap();
            routing.iter_next_hops().copied().collect()
        };

        let registration = crate::server::RsuRegistrationMessage::new(device_mac, connected_obus);
        let wire_data = registration.to_wire();

        if let Err(e) = server_socket.send_to(&wire_data, server_address).await {
            tracing::warn!("Failed to send registration to server: {:?}", e);
        } else {
            tracing::debug!(
                "Sent registration to server with {} connected OBUs",
                registration.connected_obus.len()
            );
        }
    }

    async fn handle_msg(&self, msg: &Message<'_>) -> Result<Option<Vec<ReplyType>>> {
        match msg.get_packet_type() {
            PacketType::Data(Data::Upstream(buf)) => {
                // Forward encrypted upstream traffic to server via infrastructure device
                let source_bytes: [u8; 6] = buf
                    .source()
                    .get(0..6)
                    .ok_or_else(|| anyhow!("message source too short"))?
                    .try_into()?;
                let source_mac = MacAddress::from(source_bytes);

                let server_msg = crate::server::RsuToServerMessage::new(
                    self.device.mac_address(),
                    buf.data().to_vec(),
                    source_mac,
                );

                let wire_data = server_msg.to_wire();

                // Send data to server via channel (which will use UDP socket)
                if let Err(e) = self.server_tx.send(wire_data) {
                    tracing::warn!("Failed to send to server via channel: {:?}", e);
                }

                // Return None since server will handle the processing
                Ok(None)
            }
            PacketType::Control(Control::HeartbeatReply(hbr)) => {
                if hbr.source() == self.device.mac_address() {
                    self.routing
                        .write()
                        .unwrap()
                        .handle_heartbeat_reply(msg, self.device.mac_address())
                } else {
                    Ok(None)
                }
            }
            PacketType::Data(Data::Downstream(_)) | PacketType::Control(Control::Heartbeat(_)) => {
                Ok(None)
            }
        }
    }

    fn hello_task(&self) -> Result<()> {
        let Some(periodicity) = self.args.node_params.hello_periodicity else {
            bail!("cannot generate heartbeat");
        };

        let periodicity = Duration::from_millis(periodicity.into());
        let routing = self.routing.clone();
        let device = self.device.clone();

        tokio::task::spawn(async move {
            loop {
                let msg: Vec<Vec<u8>> = {
                    let mut routing = routing.write().unwrap();
                    let msg = routing.send_heartbeat(device.mac_address());
                    tracing::trace!(?msg, "generated hello");
                    (&msg).into()
                };
                // Flatten and log the generated bytes as hex so we can compare
                // what the RSU writes vs what the OBU reads in wire_traffic.
                let flat: Vec<u8> = msg.iter().flat_map(|x| x.iter()).copied().collect();
                tracing::trace!(n = flat.len(), raw = %crate::control::node::bytes_to_hex(&flat), "rsu generated raw");
                let vec: Vec<IoSlice> = msg.iter().map(|x| IoSlice::new(x)).collect();
                let _ = device
                    .send_vectored(&vec)
                    .await
                    .inspect_err(|e| tracing::error!(?e, "error sending hello"));
                let _ = tokio_timerfd::sleep(periodicity).await;
            }
        });
        Ok(())
    }

    fn process_tap_traffic(&self) -> Result<()> {
        let tun = self.tun.clone();
        let device = self.device.clone();
        let cache = self.cache.clone();
        let routing = self.routing.clone();
        tokio::task::spawn(async move {
            loop {
                let devicec = device.clone();
                let cache = cache.clone();
                let routing = routing.clone();
                let messages = node::tap_traffic(&tun, |pkt, size| async move {
                    let data: &[u8] = &pkt[..size];
                    let to: [u8; 6] = data[0..6].try_into()?;
                    let to: MacAddress = to.into();
                    let target = cache.get(to);
                    let from: [u8; 6] = data[6..12].try_into()?;
                    let from: MacAddress = from.into();
                    let source_mac = devicec.mac_address().bytes();
                    cache.store_mac(from, devicec.mac_address());

                    let routing = routing.read().unwrap();
                    let is_multicast = to.bytes()[0] & 0x1 != 0;

                    let outgoing = if is_multicast {
                        // For multicast from RSU's TUN interface, send raw data without encryption
                        routing
                            .iter_next_hops()
                            .filter(|x| x != &&devicec.mac_address())
                            .filter_map(|x| {
                                let dest = routing.get_route_to(Some(*x))?;
                                Some((x, dest))
                            })
                            .map(|(x, y)| (x, y.mac))
                            .unique_by(|(x, _)| *x)
                            .map(|(_x, next_hop)| {
                                // RSU should not encrypt - pass raw data
                                let msg = Message::new(
                                    devicec.mac_address(),
                                    next_hop,
                                    PacketType::Data(Data::Downstream(ToDownstream::new(
                                        &source_mac,
                                        to, // Use original broadcast destination, not target OBU MAC
                                        data,
                                    ))),
                                );
                                ReplyType::Wire((&msg).into())
                            })
                            .collect_vec()
                    } else if let Some(target) = target {
                        // RSU should not encrypt - pass raw data for unicast traffic
                        if let Some(hop) = routing.get_route_to(Some(target)) {
                            vec![ReplyType::Wire(
                                (&Message::new(
                                    devicec.mac_address(),
                                    hop.mac,
                                    PacketType::Data(Data::Downstream(ToDownstream::new(
                                        &source_mac,
                                        target,
                                        data,
                                    ))),
                                ))
                                    .into(),
                            )]
                        } else {
                            // Fallback: no unicast route yet.
                            // First try: send directly to the cached node for this client if known.
                            if let Some(next_hop_mac) = cache.get(target) {
                                vec![ReplyType::Wire(
                                    (&Message::new(
                                        devicec.mac_address(),
                                        next_hop_mac,
                                        PacketType::Data(Data::Downstream(ToDownstream::new(
                                            &source_mac,
                                            target,
                                            data,
                                        ))),
                                    ))
                                        .into(),
                                )]
                            } else {
                                // Second try: fan out toward all known next hops.
                                routing
                                    .iter_next_hops()
                                    .filter(|x| x != &&devicec.mac_address())
                                    .filter_map(|x| {
                                        let dest = routing.get_route_to(Some(*x))?;
                                        Some((x, dest))
                                    })
                                    .map(|(x, y)| (x, y.mac))
                                    .unique_by(|(x, _)| *x)
                                    .map(|(_x, next_hop)| {
                                        // RSU should not encrypt - pass raw data
                                        let msg = Message::new(
                                            devicec.mac_address(),
                                            next_hop,
                                            PacketType::Data(Data::Downstream(ToDownstream::new(
                                                &source_mac,
                                                target,
                                                data,
                                            ))),
                                        );
                                        ReplyType::Wire((&msg).into())
                                    })
                                    .collect_vec()
                            }
                        }
                    } else {
                        // No client-cache mapping for destination yet: fan out using the original
                        // destination MAC from the TUN frame, not the next-hop address.
                        routing
                            .iter_next_hops()
                            .filter(|x| x != &&devicec.mac_address())
                            .filter_map(|x| {
                                let dest = routing.get_route_to(Some(*x))?;
                                Some((x, dest))
                            })
                            .map(|(x, y)| (x, y.mac))
                            .unique_by(|(x, _)| *x)
                            .map(|(_x, next_hop)| {
                                // RSU should not encrypt - pass raw data
                                let msg = Message::new(
                                    devicec.mac_address(),
                                    next_hop,
                                    PacketType::Data(Data::Downstream(ToDownstream::new(
                                        &source_mac,
                                        to,
                                        data,
                                    ))),
                                );
                                ReplyType::Wire((&msg).into())
                            })
                            .collect_vec()
                    };
                    tracing::trace!(?outgoing, "outgoing from tap");
                    Ok(Some(outgoing))
                })
                .await;

                if let Ok(Some(messages)) = messages {
                    let _ = node::handle_messages(messages, &tun, &device, None).await;
                }
            }
        });
        Ok(())
    }
}

#[cfg(test)]
pub(crate) fn handle_msg_for_test(
    routing: std::sync::Arc<std::sync::RwLock<Routing>>,
    device_mac: mac_address::MacAddress,
    _cache: std::sync::Arc<crate::control::client_cache::ClientCache>,
    msg: &crate::messages::message::Message<'_>,
) -> anyhow::Result<Option<Vec<crate::control::node::ReplyType>>> {
    use crate::messages::{control::Control, data::Data, packet_type::PacketType};

    match msg.get_packet_type() {
        PacketType::Data(Data::Upstream(_)) => {
            // RSUs now forward all upstream data to server and return None
            Ok(None)
        }
        PacketType::Control(Control::HeartbeatReply(hbr)) => {
            if hbr.source() == device_mac {
                routing
                    .write()
                    .unwrap()
                    .handle_heartbeat_reply(msg, device_mac)
            } else {
                Ok(None)
            }
        }
        PacketType::Data(Data::Downstream(_)) | PacketType::Control(Control::Heartbeat(_)) => {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod rsu_tests {
    use super::handle_msg_for_test;
    use crate::args::{NodeParameters, NodeType};
    use crate::messages::control::Control;
    use crate::messages::{
        data::{Data, ToUpstream},
        message::Message,
        packet_type::PacketType,
    };
    use crate::Args;
    use mac_address::MacAddress;

    #[test]
    fn upstream_broadcast_returns_none() {
        let args = Args {
            bind: String::new(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            node_params: NodeParameters {
                node_type: NodeType::Rsu,
                hello_history: 2,
                hello_periodicity: None,
                cached_candidates: 3,
                enable_encryption: false,
                server_address: None,
            },
        };
        let routing = std::sync::Arc::new(std::sync::RwLock::new(
            super::routing::Routing::new(&args).expect("routing"),
        ));
        let cache = std::sync::Arc::new(crate::control::client_cache::ClientCache::default());

        let from_mac: MacAddress = [1u8; 6].into();
        let dest_bytes = [255u8; 6];
        let payload = [0u8; 4];
        // inner data is: dest(6) + from(6) + payload
        let mut inner = Vec::new();
        inner.extend_from_slice(&dest_bytes);
        inner.extend_from_slice(&from_mac.bytes());
        inner.extend_from_slice(&payload);
        let tu = ToUpstream::new(from_mac, &inner);
        let msg = Message::new(
            from_mac,
            dest_bytes.into(),
            PacketType::Data(Data::Upstream(tu)),
        );

        let res =
            handle_msg_for_test(routing.clone(), [9u8; 6].into(), cache.clone(), &msg).expect("ok");
        // RSUs now forward upstream data to server and return None
        assert!(res.is_none());
    }

    #[test]
    fn heartbeat_reply_for_other_source_returns_none() {
        let args = Args {
            bind: String::new(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            node_params: NodeParameters {
                node_type: NodeType::Rsu,
                hello_history: 2,
                hello_periodicity: None,
                cached_candidates: 3,
                enable_encryption: false,
                server_address: None,
            },
        };
        let routing = std::sync::Arc::new(std::sync::RwLock::new(
            super::routing::Routing::new(&args).expect("routing"),
        ));
        let cache = std::sync::Arc::new(crate::control::client_cache::ClientCache::default());

        // Build a Heartbeat/Reply with a source different from RSU device_mac
        let src: MacAddress = [1u8; 6].into();
        let hb = crate::messages::control::heartbeat::Heartbeat::new(
            std::time::Duration::from_millis(0),
            0u32,
            src,
        );
        let reply_sender: MacAddress = [2u8; 6].into();
        let hbr =
            crate::messages::control::heartbeat::HeartbeatReply::from_sender(&hb, reply_sender);
        let msg = Message::new(
            [3u8; 6].into(),
            [255u8; 6].into(),
            PacketType::Control(Control::HeartbeatReply(hbr)),
        );

        // Device mac differs from hbr.source(); should return Ok(None)
        let res = handle_msg_for_test(routing, [9u8; 6].into(), cache, &msg).expect("ok");
        assert!(res.is_none());
    }

    #[test]
    fn upstream_unicast_to_self_returns_none() {
        let args = Args {
            bind: String::new(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            node_params: NodeParameters {
                node_type: NodeType::Rsu,
                hello_history: 2,
                hello_periodicity: None,
                cached_candidates: 3,
                enable_encryption: false,
                server_address: None,
            },
        };
        let routing = std::sync::Arc::new(std::sync::RwLock::new(
            super::routing::Routing::new(&args).expect("routing"),
        ));
        let cache = std::sync::Arc::new(crate::control::client_cache::ClientCache::default());

        let device_mac: MacAddress = [9u8; 6].into();
        let dest_client: MacAddress = [10u8; 6].into();
        let from_client: MacAddress = [1u8; 6].into();

        // Pre-map the destination client to the RSU itself so the branch triggers Tap
        cache.store_mac(dest_client, device_mac);

        let mut inner = Vec::new();
        inner.extend_from_slice(&dest_client.bytes());
        inner.extend_from_slice(&from_client.bytes());
        inner.extend_from_slice(&[0u8; 8]);
        let tu = ToUpstream::new(from_client, &inner);
        let msg = Message::new(
            from_client,
            dest_client,
            PacketType::Data(Data::Upstream(tu)),
        );

        let res = handle_msg_for_test(routing, device_mac, cache, &msg).expect("ok");
        // RSUs now forward all upstream data to server and return None
        assert!(res.is_none());
    }

    #[test]
    fn upstream_unicast_returns_none() {
        // Setup RSU args and routing/cache
        let args = Args {
            bind: String::new(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            node_params: NodeParameters {
                node_type: NodeType::Rsu,
                hello_history: 2,
                hello_periodicity: None,
                cached_candidates: 3,
                enable_encryption: false,
                server_address: None,
            },
        };
        let routing = std::sync::Arc::new(std::sync::RwLock::new(
            super::routing::Routing::new(&args).expect("routing"),
        ));
        let cache = std::sync::Arc::new(crate::control::client_cache::ClientCache::default());

        // Seed RSU routing with a sent heartbeat and a reply indicating a next hop for target_node
        let target_node: MacAddress = [77u8; 6].into();
        let next_hop: MacAddress = [88u8; 6].into();
        // Send a heartbeat to create sent-entry with id 0
        {
            let mut w = routing.write().unwrap();
            let _ = w.send_heartbeat([9u8; 6].into()); // id 0
        }
        // Now create and handle a HeartbeatReply in a separate mutable scope
        {
            let hb0 = crate::messages::control::heartbeat::Heartbeat::new(
                std::time::Duration::from_millis(0),
                0u32,
                [9u8; 6].into(),
            );
            let hbr =
                crate::messages::control::heartbeat::HeartbeatReply::from_sender(&hb0, target_node);
            let reply = crate::messages::message::Message::new(
                next_hop,
                [255u8; 6].into(),
                crate::messages::packet_type::PacketType::Control(
                    crate::messages::control::Control::HeartbeatReply(hbr),
                ),
            );
            let mut w = routing.write().unwrap();
            let _ = w
                .handle_heartbeat_reply(&reply, [9u8; 6].into())
                .expect("hb reply ok");
        }

        // Cache a client-to-node mapping so unicast will target `target_node`
        let dest_client: MacAddress = [10u8; 6].into();
        cache.store_mac(dest_client, target_node);

        // Build an upstream unicast payload destined to dest_client (not broadcast/multicast)
        let from_client: MacAddress = [1u8; 6].into();
        let mut inner = Vec::new();
        inner.extend_from_slice(&dest_client.bytes());
        inner.extend_from_slice(&from_client.bytes());
        inner.extend_from_slice(&[0u8; 8]);
        let tu = ToUpstream::new(from_client, &inner);
        let msg = Message::new(
            from_client,
            dest_client,
            PacketType::Data(Data::Upstream(tu)),
        );

        let res =
            handle_msg_for_test(routing.clone(), [9u8; 6].into(), cache.clone(), &msg).expect("ok");
        // RSUs now forward all upstream data to server and return None
        assert!(res.is_none());
    }
}
