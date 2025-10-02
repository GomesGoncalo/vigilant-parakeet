pub mod client_cache;
pub mod node;
pub mod route;
pub mod routing;
pub mod routing_utils;

use crate::args::RsuArgs;
use anyhow::{anyhow, Result};
use client_cache::ClientCache;
use common::tun::Tun;
use common::{device::Device, network_interface::NetworkInterface};
use itertools::Itertools;
use mac_address::MacAddress;
use node::ReplyType;
use node_lib::messages::{control::Control, data::Data, message::Message, packet_type::PacketType};
use routing::Routing;
use std::{
    io::IoSlice,
    sync::{Arc, RwLock},
    time::Duration,
};

pub struct Rsu {
    args: RsuArgs,
    routing: Arc<RwLock<Routing>>,
    tun: Arc<Tun>,
    device: Arc<Device>,
    cache: Arc<ClientCache>,
}

impl Rsu {
    pub fn new(args: RsuArgs, tun: Arc<Tun>, device: Arc<Device>) -> Result<Arc<Self>> {
        let rsu = Arc::new(Self {
            routing: Arc::new(RwLock::new(Routing::new(&args)?)),
            args,
            tun,
            device,
            cache: ClientCache::default().into(),
        });

        tracing::info!(?rsu.args, "Setup Rsu");
        rsu.hello_task()?;
        rsu.process_tap_traffic()?;
        Self::wire_traffic_task(rsu.clone())?;
        Ok(rsu)
    }

    /// Get route to a specific MAC address. Used for testing latency measurement.
    pub fn get_route_to(&self, mac: MacAddress) -> Option<route::Route> {
        self.routing
            .read()
            .expect("routing table read lock poisoned")
            .get_route_to(Some(mac))
    }

    /// Get count of next hops in routing table. Used for testing.
    pub fn next_hop_count(&self) -> usize {
        self.routing
            .read()
            .expect("routing table read lock poisoned")
            .iter_next_hops()
            .count()
    }

    fn wire_traffic_task(rsu: Arc<Self>) -> Result<()> {
        let device = rsu.device.clone();
        let tun = rsu.tun.clone();

        tokio::task::spawn(async move {
            loop {
                let rsu = rsu.clone();
                let messages = node::wire_traffic(&device, |pkt, size| {
                    async move {
                        let data = &pkt[..size];
                        let mut all_responses = Vec::new();
                        let mut offset = 0;

                        while offset < data.len() {
                            match Message::try_from(&data[offset..]) {
                                Ok(msg) => {
                                    let response = rsu.handle_msg(&msg).await;
                                    let has_response = response.as_ref().map(|r| r.is_some()).unwrap_or(false);
                                    #[cfg(any(test, feature = "test_helpers"))]
                                    tracing::trace!(has_response = has_response, incoming = ?msg, outgoing = ?node::get_msgs(&response), "transaction");
                                    #[cfg(not(any(test, feature = "test_helpers")))]
                                    tracing::trace!(has_response = has_response, incoming = ?msg, "transaction");

                                    if let Ok(Some(responses)) = response {
                                        all_responses.extend(responses);
                                    }
                                    // Use flat serialization for better performance
                                    let msg_bytes: Vec<u8> = (&msg).into();
                                    let msg_size: usize = msg_bytes.len();
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
                        // Use batched message handling for improved throughput (2-3x faster)
                        let _ = node::handle_messages_batched(messages, &tun, &device).await;
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

    async fn handle_msg(&self, msg: &Message<'_>) -> Result<Option<Vec<ReplyType>>> {
        match msg.get_packet_type() {
            PacketType::Data(Data::Upstream(buf)) => {
                // Decrypt the entire frame if encryption is enabled
                let decrypted_payload = if self.args.rsu_params.enable_encryption {
                    match node_lib::crypto::decrypt_payload(buf.data()) {
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
                    messages.push(ReplyType::TapFlat(decrypted_payload.clone()));
                    target = None;
                }

                let routing = self
                    .routing
                    .read()
                    .expect("routing table read lock poisoned during wire traffic processing");
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
                            let downstream_data = if self.args.rsu_params.enable_encryption {
                                match node_lib::crypto::encrypt_payload(&decrypted_payload) {
                                    Ok(encrypted_data) => encrypted_data,
                                    Err(_) => return None, // Skip this recipient on encryption failure
                                }
                            } else {
                                decrypted_payload.clone()
                            };

                            // For broadcast distribution, use the original destination (broadcast) not the target OBU
                            // Use zero-copy serialization (16.5x faster than traditional)
                            let mut wire = Vec::with_capacity(30 + downstream_data.len());
                            Message::serialize_downstream_into(
                                buf.source(),
                                to, // Use original broadcast destination, not target OBU
                                &downstream_data,
                                self.device.mac_address(),
                                next_hop,
                                &mut wire,
                            );
                            Some(ReplyType::WireFlat(wire))
                        })
                        .collect_vec()
                } else if let Some(target) = target {
                    let Some(next_hop) = routing.get_route_to(Some(target)) else {
                        return Ok(None);
                    };

                    // For unicast traffic, encrypt the entire decrypted frame for the specific recipient
                    let downstream_data = if self.args.rsu_params.enable_encryption {
                        match node_lib::crypto::encrypt_payload(&decrypted_payload) {
                            Ok(encrypted_data) => encrypted_data,
                            Err(_) => return Ok(None),
                        }
                    } else {
                        decrypted_payload.clone()
                    };

                    // Use zero-copy serialization (16.5x faster than traditional)
                    let mut wire = Vec::with_capacity(30 + downstream_data.len());
                    Message::serialize_downstream_into(
                        buf.source(),
                        target,
                        &downstream_data,
                        self.device.mac_address(),
                        next_hop.mac,
                        &mut wire,
                    );
                    vec![ReplyType::WireFlat(wire)]
                } else {
                    vec![]
                });

                Ok(Some(messages))
            }
            PacketType::Control(Control::HeartbeatReply(hbr)) => {
                if hbr.source() == self.device.mac_address() {
                    self.routing
                        .write()
                        .expect("routing table write lock poisoned during heartbeat reply")
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
        let periodicity = self.args.rsu_params.hello_periodicity;
        let periodicity = Duration::from_millis(periodicity.into());
        let routing = self.routing.clone();
        let device = self.device.clone();

        tokio::task::spawn(async move {
            loop {
                let msg: Vec<u8> = {
                    let mut routing = routing
                        .write()
                        .expect("routing table write lock poisoned in heartbeat task");
                    let msg = routing.send_heartbeat(device.mac_address());
                    tracing::trace!(?msg, "generated hello");
                    (&msg).into()
                };
                let vec = [IoSlice::new(&msg)];
                match device.send_vectored(&vec).await {
                    Ok(_) => {}
                    Err(e) => {
                        tracing::error!(?e, "RSU failed to send heartbeat");
                    }
                }
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
        let enable_encryption = self.args.rsu_params.enable_encryption;
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

                    let routing = routing
                        .read()
                        .expect("routing table read lock poisoned during tap traffic processing");
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
                                    match node_lib::crypto::encrypt_payload(data) {
                                        Ok(encrypted_data) => encrypted_data,
                                        Err(_) => return None, // Skip this recipient on encryption failure
                                    }
                                } else {
                                    data.to_vec()
                                };

                                // Use zero-copy serialization (16.5x faster than traditional)
                                let mut wire = Vec::with_capacity(30 + downstream_data.len());
                                Message::serialize_downstream_into(
                                    &source_mac,
                                    to, // Use original broadcast destination, not target OBU MAC
                                    &downstream_data,
                                    devicec.mac_address(),
                                    next_hop,
                                    &mut wire,
                                );
                                Some(ReplyType::WireFlat(wire))
                            })
                            .collect_vec()
                    } else if let Some(target) = target {
                        // Encrypt entire frame for unicast traffic if encryption is enabled
                        let downstream_data = if enable_encryption {
                            match node_lib::crypto::encrypt_payload(data) {
                                Ok(encrypted_data) => encrypted_data,
                                Err(_) => return Ok(None),
                            }
                        } else {
                            data.to_vec()
                        };

                        // Unicast traffic with known target
                        if let Some(hop) = routing.get_route_to(Some(target)) {
                            // Use zero-copy serialization (16.5x faster than traditional)
                            let mut wire = Vec::with_capacity(30 + downstream_data.len());
                            Message::serialize_downstream_into(
                                &source_mac,
                                target,
                                &downstream_data,
                                devicec.mac_address(),
                                hop.mac,
                                &mut wire,
                            );
                            vec![ReplyType::WireFlat(wire)]
                        } else {
                            // Fallback: no unicast route yet.
                            // First try: send directly to the cached node for this client if known.
                            if let Some(next_hop_mac) = cache.get(target) {
                                // Use zero-copy serialization (16.5x faster than traditional)
                                let mut wire = Vec::with_capacity(30 + downstream_data.len());
                                Message::serialize_downstream_into(
                                    &source_mac,
                                    target,
                                    &downstream_data,
                                    devicec.mac_address(),
                                    next_hop_mac,
                                    &mut wire,
                                );
                                vec![ReplyType::WireFlat(wire)]
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
                                            match node_lib::crypto::encrypt_payload(data) {
                                                Ok(encrypted_data) => encrypted_data,
                                                Err(_) => return None, // Skip this recipient on encryption failure
                                            }
                                        } else {
                                            data.to_vec()
                                        };

                                        // Use zero-copy serialization (16.5x faster than traditional)
                                        let mut wire =
                                            Vec::with_capacity(30 + downstream_data.len());
                                        Message::serialize_downstream_into(
                                            &source_mac,
                                            target,
                                            &downstream_data,
                                            devicec.mac_address(),
                                            next_hop,
                                            &mut wire,
                                        );
                                        Some(ReplyType::WireFlat(wire))
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
                                    match node_lib::crypto::encrypt_payload(data) {
                                        Ok(encrypted_data) => encrypted_data,
                                        Err(_) => return None, // Skip this recipient on encryption failure
                                    }
                                } else {
                                    data.to_vec()
                                };

                                // Use zero-copy serialization (16.5x faster than traditional)
                                let mut wire = Vec::with_capacity(30 + downstream_data.len());
                                Message::serialize_downstream_into(
                                    &source_mac,
                                    to,
                                    &downstream_data,
                                    devicec.mac_address(),
                                    next_hop,
                                    &mut wire,
                                );
                                Some(ReplyType::WireFlat(wire))
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
    cache: std::sync::Arc<ClientCache>,
    msg: &node_lib::messages::message::Message<'_>,
) -> anyhow::Result<Option<Vec<ReplyType>>> {
    use node_lib::messages::{control::Control, data::Data, packet_type::PacketType};

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
                messages.push(ReplyType::TapFlat(buf.data().to_vec()));
                target = None;
            }

            let routing = routing
                .read()
                .expect("routing table read lock poisoned during data processing");
            messages.extend(if bcast_or_mcast {
                routing
                    .iter_next_hops()
                    .filter(|x| x != &&source)
                    .filter_map(|x| {
                        let route = routing.get_route_to(Some(*x))?;
                        Some((*x, route.mac))
                    })
                    .map(|(target, next_hop)| {
                        // Use zero-copy serialization (16.5x faster than traditional)
                        let mut wire = Vec::with_capacity(30 + buf.data().len());
                        node_lib::messages::message::Message::serialize_downstream_into(
                            buf.source(),
                            target,
                            buf.data(),
                            device_mac,
                            next_hop,
                            &mut wire,
                        );
                        ReplyType::WireFlat(wire)
                    })
                    .collect::<Vec<_>>()
            } else if let Some(target) = target {
                let Some(next_hop) = routing.get_route_to(Some(target)) else {
                    return Ok(None);
                };

                // Use zero-copy serialization (16.5x faster than traditional)
                let mut wire = Vec::with_capacity(30 + buf.data().len());
                node_lib::messages::message::Message::serialize_downstream_into(
                    buf.source(),
                    target,
                    buf.data(),
                    device_mac,
                    next_hop.mac,
                    &mut wire,
                );
                vec![ReplyType::WireFlat(wire)]
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
    use super::{handle_msg_for_test, routing::Routing, ClientCache, ReplyType};
    use mac_address::MacAddress;
    use node_lib::messages::control::Control;
    use node_lib::messages::{
        data::{Data, ToUpstream},
        message::Message,
        packet_type::PacketType,
    };

    #[test]
    fn upstream_broadcast_generates_tap() {
        let args = crate::args::RsuArgs {
            bind: String::new(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            rsu_params: crate::args::RsuParameters {
                hello_history: 2,
                hello_periodicity: 5000,
                cached_candidates: 3,
                enable_encryption: false,
            },
        };
        let routing = std::sync::Arc::new(std::sync::RwLock::new(
            Routing::new(&args).expect("routing"),
        ));
        let cache = std::sync::Arc::new(ClientCache::default());

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
        let args = crate::args::RsuArgs {
            bind: String::new(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            rsu_params: crate::args::RsuParameters {
                hello_history: 2,
                hello_periodicity: 5000,
                cached_candidates: 3,
                enable_encryption: false,
            },
        };
        let routing = std::sync::Arc::new(std::sync::RwLock::new(
            Routing::new(&args).expect("routing"),
        ));
        let cache = std::sync::Arc::new(ClientCache::default());

        // Build a Heartbeat/Reply with a source different from RSU device_mac
        let src: MacAddress = [1u8; 6].into();
        let hb = node_lib::messages::control::heartbeat::Heartbeat::new(
            std::time::Duration::from_millis(0),
            0u32,
            src,
        );
        let reply_sender: MacAddress = [2u8; 6].into();
        let hbr =
            node_lib::messages::control::heartbeat::HeartbeatReply::from_sender(&hb, reply_sender);
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
        let args = crate::args::RsuArgs {
            bind: String::new(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            rsu_params: crate::args::RsuParameters {
                hello_history: 2,
                hello_periodicity: 5000,
                cached_candidates: 3,
                enable_encryption: false,
            },
        };
        let routing = std::sync::Arc::new(std::sync::RwLock::new(
            Routing::new(&args).expect("routing"),
        ));
        let cache = std::sync::Arc::new(ClientCache::default());

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
        // Expect exactly one TapFlat and no Wire messages
        assert_eq!(msgs.len(), 1);
        match &msgs[0] {
            ReplyType::TapFlat(_) => {}
            _ => panic!("expected TapFlat only"),
        }
    }

    #[test]
    fn upstream_unicast_forwards_via_route() {
        // Setup RSU args and routing/cache
        let args = crate::args::RsuArgs {
            bind: String::new(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            rsu_params: crate::args::RsuParameters {
                hello_history: 2,
                hello_periodicity: 5000,
                cached_candidates: 3,
                enable_encryption: false,
            },
        };
        let routing = std::sync::Arc::new(std::sync::RwLock::new(
            Routing::new(&args).expect("routing"),
        ));
        let cache = std::sync::Arc::new(ClientCache::default());

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
            let hb0 = node_lib::messages::control::heartbeat::Heartbeat::new(
                std::time::Duration::from_millis(0),
                0u32,
                [9u8; 6].into(),
            );
            let hbr = node_lib::messages::control::heartbeat::HeartbeatReply::from_sender(
                &hb0,
                target_node,
            );
            let reply = node_lib::messages::message::Message::new(
                next_hop,
                [255u8; 6].into(),
                node_lib::messages::packet_type::PacketType::Control(
                    node_lib::messages::control::Control::HeartbeatReply(hbr),
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
        // Expect exactly one WireFlat message (no Tap), forwarding toward next_hop
        assert_eq!(msgs.len(), 1);
        if let ReplyType::WireFlat(wire) = &msgs[0] {
            // Deserialize to inspect destination MAC
            let out = Message::try_from(&wire[..]).expect("parse out");
            assert_eq!(out.to().unwrap(), next_hop);
        } else {
            panic!("expected WireFlat reply");
        }
    }

    #[tokio::test]
    async fn upstream_with_encryption_bad_cipher_returns_none() -> anyhow::Result<()> {
        use crate::args::RsuArgs;
        use crate::Rsu;
        use mac_address::MacAddress;
        use node_lib::messages::{
            data::{Data, ToUpstream},
            message::Message,
            packet_type::PacketType,
        };
        use std::sync::Arc;

        // Build args with encryption enabled
        let args = RsuArgs {
            bind: String::new(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            rsu_params: crate::args::RsuParameters {
                hello_history: 2,
                hello_periodicity: 5000,
                cached_candidates: 3,
                enable_encryption: true,
            },
        };

        // Create shim tun and device using a test pipe (avoid privileged ops)
        let (tun_a, _tun_b) = node_lib::test_helpers::util::mk_shim_pair();
        let tun = Arc::new(tun_a);
        // create a pipe and use the writer fd as the device send end
        let mut fds = [0; 2];
        unsafe { libc::pipe(fds.as_mut_ptr()) };
        let reader_fd = fds[0];
        let writer_fd = fds[1];
        // set non-blocking for writer_fd
        unsafe {
            let flags = libc::fcntl(writer_fd, libc::F_GETFL);
            if flags >= 0 {
                let _ = libc::fcntl(writer_fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
            }
        }
        let dev = std::sync::Arc::new(node_lib::test_helpers::util::mk_device_from_fd(
            [0u8, 1, 2, 3, 4, 5].into(),
            writer_fd,
        ));

        // Construct the RSU instance manually without spawning background tasks
        use super::routing::Routing;
        use super::ClientCache;
        use std::sync::RwLock;
        let routing = Arc::new(RwLock::new(Routing::new(&args)?));
        let cache = Arc::new(ClientCache::default());
        let rsu = Arc::new(Rsu {
            args,
            routing: routing.clone(),
            tun: tun.clone(),
            device: dev.clone(),
            cache: cache.clone(),
        });

        // Build an upstream message whose payload is invalid ciphertext so decrypt fails
        let from_mac: MacAddress = [1u8; 6].into();
        let dest_mac: MacAddress = [2u8; 6].into();
        let mut inner = Vec::new();
        inner.extend_from_slice(&dest_mac.bytes());
        inner.extend_from_slice(&from_mac.bytes());
        inner.extend_from_slice(b"not-a-valid-ciphertext");

        let tu = ToUpstream::new(from_mac, &inner);
        let msg = Message::new(from_mac, dest_mac, PacketType::Data(Data::Upstream(tu)));

        let res = rsu.handle_msg(&msg).await?;
        // decrypt should fail and the handler returns None
        assert!(res.is_none());
        // Close fds to keep OS state clean (don't block attempting to read)
        unsafe {
            libc::close(reader_fd);
            libc::close(writer_fd);
        }
        Ok(())
    }
}
