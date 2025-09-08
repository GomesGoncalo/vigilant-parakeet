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
    io::IoSlice,
    sync::{Arc, RwLock},
    time::Duration,
};
use tokio::net::UdpSocket;

pub struct Rsu {
    args: Args,
    routing: Arc<RwLock<Routing>>,
    tun: Arc<Tun>,
    device: Arc<Device>,
    cache: Arc<ClientCache>,
    server_socket: Option<Arc<UdpSocket>>,
}

impl Rsu {
    pub fn new(args: Args, tun: Arc<Tun>, device: Arc<Device>) -> Result<Arc<Self>> {
        // Create UDP socket for server communication if server address is provided
        let server_socket = if args.node_params.server_address.is_some() {
            let socket = std::net::UdpSocket::bind("0.0.0.0:0")?;
            socket.set_nonblocking(true)?;
            Some(Arc::new(UdpSocket::from_std(socket)?))
        } else {
            None
        };

        let rsu = Arc::new(Self {
            routing: Arc::new(RwLock::new(Routing::new(&args)?)),
            args,
            tun,
            device,
            cache: ClientCache::default().into(),
            server_socket,
        });

        tracing::info!(?rsu.args, "Setup Rsu");
        rsu.hello_task()?;
        rsu.process_tap_traffic()?;
        Self::wire_traffic_task(rsu.clone())?;

        // Start server response handling task if server socket exists
        if rsu.server_socket.is_some() {
            Self::server_response_task(rsu.clone())?;
        }

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

    fn server_response_task(rsu: Arc<Self>) -> Result<()> {
        let Some(ref server_socket) = rsu.server_socket else {
            return Ok(()); // No server socket, nothing to do
        };

        let socket = server_socket.clone();
        let tun = rsu.tun.clone();
        let device = rsu.device.clone();
        let routing = rsu.routing.clone();
        let cache = rsu.cache.clone();
        let enable_encryption = rsu.args.node_params.enable_encryption;

        tokio::task::spawn(async move {
            let mut buffer = vec![0u8; 65536];

            loop {
                match socket.recv(&mut buffer).await {
                    Ok(len) => {
                        let data = &buffer[..len];
                        if let Ok(server_msg) =
                            bincode::deserialize::<crate::server::ServerToRsuMessage>(data)
                        {
                            tracing::debug!(
                                "Received server response: destination={:?}",
                                server_msg.destination_mac
                            );

                            // Process the decrypted payload similar to original RSU logic
                            let destination_mac: MacAddress = server_msg.destination_mac.into();
                            let source_mac: MacAddress = server_msg.source_mac.into();

                            let is_broadcast = destination_mac == [255; 6].into()
                                || destination_mac.bytes()[0] & 0x1 != 0;
                            let target = cache.get(destination_mac);
                            let mut messages = Vec::new();

                            // Send to tap if broadcast/multicast or if we're the target
                            if is_broadcast || target.is_some_and(|x| x == device.mac_address()) {
                                messages.push(ReplyType::Tap(vec![server_msg
                                    .decrypted_payload
                                    .clone()]));
                            }

                            // Forward to other nodes based on routing
                            let forwards: Vec<_> = {
                                let routing = routing.read().unwrap();
                                if is_broadcast {
                                    // For broadcast, forward to next hops except source
                                    routing
                                        .iter_next_hops()
                                        .filter(|&&mac| mac != source_mac)
                                        .filter_map(|&next_hop_mac| {
                                            let route = routing.get_route_to(Some(next_hop_mac))?;

                                            // Re-encrypt if encryption is enabled
                                            let downstream_data = if enable_encryption {
                                                match crate::crypto::encrypt_payload(
                                                    &server_msg.decrypted_payload,
                                                ) {
                                                    Ok(encrypted_data) => encrypted_data,
                                                    Err(_) => return None,
                                                }
                                            } else {
                                                server_msg.decrypted_payload.clone()
                                            };

                                            Some(ReplyType::Wire(
                                                (&Message::new(
                                                    device.mac_address(),
                                                    route.mac,
                                                    PacketType::Data(Data::Downstream(
                                                        ToDownstream::new(
                                                            &source_mac.bytes(),
                                                            destination_mac,
                                                            &downstream_data,
                                                        ),
                                                    )),
                                                ))
                                                    .into(),
                                            ))
                                        })
                                        .collect()
                                } else if let Some(target_mac) = target {
                                    // For unicast, forward to specific target
                                    if let Some(route) = routing.get_route_to(Some(target_mac)) {
                                        let downstream_data = if enable_encryption {
                                            match crate::crypto::encrypt_payload(
                                                &server_msg.decrypted_payload,
                                            ) {
                                                Ok(encrypted_data) => encrypted_data,
                                                Err(_) => {
                                                    tracing::warn!("Failed to encrypt payload for unicast forwarding");
                                                    continue;
                                                }
                                            }
                                        } else {
                                            server_msg.decrypted_payload.clone()
                                        };

                                        vec![ReplyType::Wire(
                                            (&Message::new(
                                                device.mac_address(),
                                                route.mac,
                                                PacketType::Data(Data::Downstream(
                                                    ToDownstream::new(
                                                        &source_mac.bytes(),
                                                        target_mac,
                                                        &downstream_data,
                                                    ),
                                                )),
                                            ))
                                                .into(),
                                        )]
                                    } else {
                                        Vec::new()
                                    }
                                } else {
                                    Vec::new()
                                }
                            };
                            messages.extend(forwards);

                            if !messages.is_empty() {
                                let _ = node::handle_messages(messages, &tun, &device, None).await;
                            }
                        } else {
                            tracing::warn!("Failed to deserialize server response");
                        }
                    }
                    Err(e) => {
                        tracing::error!("Error receiving from server socket: {:?}", e);
                        break;
                    }
                }
            }
        });
        Ok(())
    }

    async fn handle_msg(&self, msg: &Message<'_>) -> Result<Option<Vec<ReplyType>>> {
        match msg.get_packet_type() {
            PacketType::Data(Data::Upstream(buf)) => {
                // Check if we should forward to server or handle locally
                if let Some(server_addr) = self.args.node_params.server_address {
                    // Forward encrypted upstream traffic to server
                    if let Some(ref server_socket) = self.server_socket {
                        let source: [u8; 6] = buf
                            .source()
                            .get(0..6)
                            .ok_or_else(|| anyhow!("message source too short"))?
                            .try_into()?;

                        let server_msg = crate::server::RsuToServerMessage {
                            rsu_mac: self.device.mac_address().bytes(),
                            encrypted_data: buf.data().to_vec(),
                            original_source: source,
                        };

                        let serialized = bincode::serialize(&server_msg)
                            .map_err(|e| anyhow!("Failed to serialize server message: {}", e))?;

                        // Send to server (fire and forget)
                        if let Err(e) = server_socket.send_to(&serialized, server_addr).await {
                            tracing::warn!("Failed to send to server: {:?}", e);
                        }

                        // Return None since server will handle the processing
                        return Ok(None);
                    } else {
                        tracing::warn!("Server address configured but no server socket available");
                        return Ok(None);
                    }
                }

                // Legacy mode: decrypt locally (when no server address is configured)
                let decrypted_payload = if self.args.node_params.enable_encryption {
                    match crate::crypto::decrypt_payload(buf.data()) {
                        Ok(decrypted_data) => decrypted_data,
                        Err(_) => return Ok(None),
                    }
                } else {
                    buf.data().to_vec()
                };

                // Extract MAC addresses from decrypted data
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
                let source: [u8; 6] = buf
                    .source()
                    .get(0..6)
                    .ok_or_else(|| anyhow!("message source too short"))?
                    .try_into()?;
                let source: MacAddress = source.into();
                self.cache.store_mac(from, source);
                let bcast_or_mcast = to == [255; 6].into() || to.bytes()[0] & 0x1 != 0;
                let mut target = self.cache.get(to);
                let mut messages = Vec::with_capacity(1);

                if bcast_or_mcast || target.is_some_and(|x| x == self.device.mac_address()) {
                    messages.push(ReplyType::Tap(vec![decrypted_payload.clone()]));
                    target = None;
                }

                let routing = self.routing.read().unwrap();
                messages.extend(if bcast_or_mcast {
                    routing
                        .iter_next_hops()
                        .filter(|x| x != &&source)
                        .filter_map(|x| {
                            let route = routing.get_route_to(Some(*x))?;
                            Some((*x, route.mac))
                        })
                        .filter_map(|(_target, next_hop)| {
                            // For broadcast traffic, encrypt the entire decrypted frame individually for each recipient
                            let downstream_data = if self.args.node_params.enable_encryption {
                                match crate::crypto::encrypt_payload(&decrypted_payload) {
                                    Ok(encrypted_data) => encrypted_data,
                                    Err(_) => return None, // Skip this recipient on encryption failure
                                }
                            } else {
                                decrypted_payload.clone()
                            };

                            // For broadcast distribution, use the original destination (broadcast) not the target OBU
                            Some(ReplyType::Wire(
                                (&Message::new(
                                    self.device.mac_address(),
                                    next_hop,
                                    PacketType::Data(Data::Downstream(ToDownstream::new(
                                        buf.source(),
                                        to, // Use original broadcast destination, not target OBU
                                        &downstream_data,
                                    ))),
                                ))
                                    .into(),
                            ))
                        })
                        .collect_vec()
                } else if let Some(target) = target {
                    let Some(next_hop) = routing.get_route_to(Some(target)) else {
                        return Ok(None);
                    };

                    // For unicast traffic, encrypt the entire decrypted frame for the specific recipient
                    let downstream_data = if self.args.node_params.enable_encryption {
                        match crate::crypto::encrypt_payload(&decrypted_payload) {
                            Ok(encrypted_data) => encrypted_data,
                            Err(_) => return Ok(None),
                        }
                    } else {
                        decrypted_payload.clone()
                    };

                    vec![ReplyType::Wire(
                        (&Message::new(
                            self.device.mac_address(),
                            next_hop.mac,
                            PacketType::Data(Data::Downstream(ToDownstream::new(
                                buf.source(),
                                target,
                                &downstream_data,
                            ))),
                        ))
                            .into(),
                    )]
                } else {
                    vec![]
                });

                Ok(Some(messages))
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
        let enable_encryption = self.args.node_params.enable_encryption;
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
                        // For multicast from RSU's TUN interface, encrypt individually for each recipient
                        routing
                            .iter_next_hops()
                            .filter(|x| x != &&devicec.mac_address())
                            .filter_map(|x| {
                                let dest = routing.get_route_to(Some(*x))?;
                                Some((x, dest))
                            })
                            .map(|(x, y)| (x, y.mac))
                            .unique_by(|(x, _)| *x)
                            .filter_map(|(_x, next_hop)| {
                                // Encrypt the entire frame individually for each recipient when encryption is enabled
                                let downstream_data = if enable_encryption {
                                    match crate::crypto::encrypt_payload(data) {
                                        Ok(encrypted_data) => encrypted_data,
                                        Err(_) => return None, // Skip this recipient on encryption failure
                                    }
                                } else {
                                    data.to_vec()
                                };

                                let msg = Message::new(
                                    devicec.mac_address(),
                                    next_hop,
                                    PacketType::Data(Data::Downstream(ToDownstream::new(
                                        &source_mac,
                                        to, // Use original broadcast destination, not target OBU MAC
                                        &downstream_data,
                                    ))),
                                );
                                Some(ReplyType::Wire((&msg).into()))
                            })
                            .collect_vec()
                    } else if let Some(target) = target {
                        // Encrypt entire frame for unicast traffic if encryption is enabled
                        let downstream_data = if enable_encryption {
                            match crate::crypto::encrypt_payload(data) {
                                Ok(encrypted_data) => encrypted_data,
                                Err(_) => return Ok(None),
                            }
                        } else {
                            data.to_vec()
                        };

                        // Unicast traffic with known target
                        if let Some(hop) = routing.get_route_to(Some(target)) {
                            vec![ReplyType::Wire(
                                (&Message::new(
                                    devicec.mac_address(),
                                    hop.mac,
                                    PacketType::Data(Data::Downstream(ToDownstream::new(
                                        &source_mac,
                                        target,
                                        &downstream_data,
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
                                            &downstream_data,
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
                                    .filter_map(|(_x, next_hop)| {
                                        // Encrypt the entire frame individually for each recipient when encryption is enabled
                                        let downstream_data = if enable_encryption {
                                            match crate::crypto::encrypt_payload(data) {
                                                Ok(encrypted_data) => encrypted_data,
                                                Err(_) => return None, // Skip this recipient on encryption failure
                                            }
                                        } else {
                                            data.to_vec()
                                        };

                                        let msg = Message::new(
                                            devicec.mac_address(),
                                            next_hop,
                                            PacketType::Data(Data::Downstream(ToDownstream::new(
                                                &source_mac,
                                                target,
                                                &downstream_data,
                                            ))),
                                        );
                                        Some(ReplyType::Wire((&msg).into()))
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
                            .filter_map(|(_x, next_hop)| {
                                // Encrypt the entire frame individually for each recipient when encryption is enabled
                                let downstream_data = if enable_encryption {
                                    match crate::crypto::encrypt_payload(data) {
                                        Ok(encrypted_data) => encrypted_data,
                                        Err(_) => return None, // Skip this recipient on encryption failure
                                    }
                                } else {
                                    data.to_vec()
                                };

                                let msg = Message::new(
                                    devicec.mac_address(),
                                    next_hop,
                                    PacketType::Data(Data::Downstream(ToDownstream::new(
                                        &source_mac,
                                        to,
                                        &downstream_data,
                                    ))),
                                );
                                Some(ReplyType::Wire((&msg).into()))
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
    cache: std::sync::Arc<crate::control::client_cache::ClientCache>,
    msg: &crate::messages::message::Message<'_>,
) -> anyhow::Result<Option<Vec<ReplyType>>> {
    use crate::messages::{control::Control, data::Data, packet_type::PacketType};

    match msg.get_packet_type() {
        PacketType::Data(Data::Upstream(buf)) => {
            let to: [u8; 6] = buf
                .data()
                .get(0..6)
                .ok_or_else(|| anyhow::anyhow!("error"))?
                .try_into()?;
            let to: mac_address::MacAddress = to.into();
            let from: [u8; 6] = buf
                .data()
                .get(6..12)
                .ok_or_else(|| anyhow::anyhow!("error"))?
                .try_into()?;
            let from: mac_address::MacAddress = from.into();
            let source: [u8; 6] = buf
                .source()
                .get(0..6)
                .ok_or_else(|| anyhow::anyhow!("error"))?
                .try_into()?;
            let source: mac_address::MacAddress = source.into();
            cache.store_mac(from, source);
            let bcast_or_mcast = to == [255; 6].into() || to.bytes()[0] & 0x1 != 0;
            let mut target = cache.get(to);
            let mut messages = Vec::with_capacity(1);
            if bcast_or_mcast || target.is_some_and(|x| x == device_mac) {
                messages.push(ReplyType::Tap(vec![buf.data().to_vec()]));
                target = None;
            }

            let routing = routing.read().unwrap();
            messages.extend(if bcast_or_mcast {
                routing
                    .iter_next_hops()
                    .filter(|x| x != &&source)
                    .filter_map(|x| {
                        let route = routing.get_route_to(Some(*x))?;
                        Some((*x, route.mac))
                    })
                    .map(|(target, next_hop)| {
                        ReplyType::Wire(
                            (&crate::messages::message::Message::new(
                                device_mac,
                                next_hop,
                                PacketType::Data(Data::Downstream(
                                    crate::messages::data::ToDownstream::new(
                                        buf.source(),
                                        target,
                                        buf.data(),
                                    ),
                                )),
                            ))
                                .into(),
                        )
                    })
                    .collect::<Vec<_>>()
            } else if let Some(target) = target {
                let Some(next_hop) = routing.get_route_to(Some(target)) else {
                    return Ok(None);
                };

                vec![ReplyType::Wire(
                    (&crate::messages::message::Message::new(
                        device_mac,
                        next_hop.mac,
                        PacketType::Data(Data::Downstream(
                            crate::messages::data::ToDownstream::new(
                                buf.source(),
                                target,
                                buf.data(),
                            ),
                        )),
                    ))
                        .into(),
                )]
            } else {
                vec![]
            });

            Ok(Some(messages))
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
    use super::ReplyType;
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
    fn upstream_broadcast_generates_tap() {
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
        assert!(res.is_some());
        let v = res.unwrap();
        // should at least have Tap entry for broadcast
        assert!(!v.is_empty());
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
    fn upstream_unicast_to_self_yields_tap_only() {
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
        assert!(res.is_some());
        let msgs = res.unwrap();
        // Expect exactly one Tap and no Wire messages
        assert_eq!(msgs.len(), 1);
        match &msgs[0] {
            ReplyType::Tap(_) => {}
            _ => panic!("expected Tap only"),
        }
    }

    #[test]
    fn upstream_unicast_forwards_via_route() {
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
        assert!(res.is_some());
        let msgs = res.unwrap();
        // Expect exactly one Wire message (no Tap), forwarding toward next_hop
        assert_eq!(msgs.len(), 1);
        if let ReplyType::Wire(v) = &msgs[0] {
            // Deserialize to inspect destination MAC
            let flat: Vec<u8> = v.iter().flat_map(|x| x.iter()).cloned().collect();
            let out = Message::try_from(&flat[..]).expect("parse out");
            assert_eq!(out.to().unwrap(), next_hop);
        } else {
            panic!("expected Wire reply");
        }
    }
}
