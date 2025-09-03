pub mod routing;

use super::{client_cache::ClientCache, node::ReplyType};
use crate::control::node::BufPart;
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

pub struct Rsu {
    args: Args,
    routing: Arc<RwLock<Routing>>,
    tun: Arc<Tun>,
    device: Arc<Device>,
    cache: Arc<ClientCache>,
}

impl Rsu {
    pub fn new(args: Args, tun: Arc<Tun>, device: Arc<Device>) -> Result<Arc<Self>> {
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

    fn wire_traffic_task(rsu: Arc<Self>) -> Result<()> {
        let device = rsu.device.clone();
        let tun = rsu.tun.clone();

    tokio::task::spawn(async move {
            loop {
                let rsu = rsu.clone();
                let messages = node::wire_traffic(&device, |pkt, size| {
                    async move {
            // pkt is Arc<[u8]> with length `size` already
            match Message::try_from(&pkt[..]) {
                            Ok(msg) => {
                                tracing::trace!(parsed = ?msg, "rsu wire_traffic parsed message");
                                // Prefer zero-copy reply assembly using the received backing
                                let response = match rsu.handle_msg_with_backing(&msg, &pkt).await {
                                    Ok(v) => Ok(v),
                                    Err(_) => rsu.handle_msg(&msg).await,
                                };
                                let has_response = response.as_ref().map(|r| r.is_some()).unwrap_or(false);
                                tracing::trace!(has_response = has_response, incoming = ?msg, outgoing = ?node::get_msgs(&response), "transaction");
                                response
                            }
                            Err(e) => {
                                tracing::trace!(error = ?e, raw = %crate::control::node::bytes_to_hex(&pkt[..size]), "rsu wire_traffic failed to parse message");
                                Ok(None)
                            }
                        }
                    }
                }).await;
                if let Ok(Some(messages)) = messages {
                    let _ = node::handle_messages(messages, &tun, &device).await;
                }
            }
        });
        Ok(())
    }

    async fn handle_msg(&self, msg: &Message<'_>) -> Result<Option<Vec<ReplyType>>> {
        match msg.get_packet_type() {
            PacketType::Data(Data::Upstream(buf)) => {
                let to: [u8; 6] = buf
                    .data()
                    .get(0..6)
                    .ok_or_else(|| anyhow!("error"))?
                    .try_into()?;
                let to: MacAddress = to.into();
                let from: [u8; 6] = buf
                    .data()
                    .get(6..12)
                    .ok_or_else(|| anyhow!("error"))?
                    .try_into()?;
                let from: MacAddress = from.into();
                let source: [u8; 6] = buf
                    .source()
                    .get(0..6)
                    .ok_or_else(|| anyhow!("error"))?
                    .try_into()?;
                let source: MacAddress = source.into();
                self.cache.store_mac(from, source);
                let bcast_or_mcast = to == [255; 6].into() || to.bytes()[0] & 0x1 != 0;
                let mut target = self.cache.get(to);
                let mut messages = Vec::with_capacity(1);
                if bcast_or_mcast || target.is_some_and(|x| x == self.device.mac_address()) {
                    messages.push(ReplyType::Tap(vec![buf.data().to_vec()]));
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
                        .map(|(target, next_hop)| {
                            ReplyType::Wire(
                                (&Message::new(
                                    self.device.mac_address(),
                                    next_hop,
                                    PacketType::Data(Data::Downstream(ToDownstream::new(
                                        buf.source(),
                                        target,
                                        buf.data(),
                                    ))),
                                ))
                                    .into(),
                            )
                        })
                        .collect_vec()
                } else if let Some(target) = target {
                    let Some(next_hop) = routing.get_route_to(Some(target)) else {
                        return Ok(None);
                    };

                    vec![ReplyType::Wire(
                        (&Message::new(
                            self.device.mac_address(),
                            next_hop.mac,
                            PacketType::Data(Data::Downstream(ToDownstream::new(
                                buf.source(),
                                target,
                                buf.data(),
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

    /// Zero-copy reply assembly for messages received from wire using `backing` Arc.
    async fn handle_msg_with_backing(
        &self,
        msg: &Message<'_>,
        backing: &Arc<[u8]>,
    ) -> Result<Option<Vec<ReplyType>>> {
        match msg.get_packet_type() {
            PacketType::Data(Data::Upstream(buf)) => {
                let data_slice = buf.data().as_ref();
                let to: [u8; 6] = data_slice
                    .get(0..6)
                    .ok_or_else(|| anyhow!("error"))?
                    .try_into()?;
                let to: MacAddress = to.into();
                let from: [u8; 6] = data_slice
                    .get(6..12)
                    .ok_or_else(|| anyhow!("error"))?
                    .try_into()?;
                let from: MacAddress = from.into();
                let source_slice = buf.source().as_ref();

                let source_arr: [u8; 6] = source_slice
                    .get(0..6)
                    .ok_or_else(|| anyhow!("error"))?
                    .try_into()?;
                let source_mac: MacAddress = source_arr.into();
                self.cache.store_mac(from, source_mac);
                let bcast_or_mcast = to == [255; 6].into() || to.bytes()[0] & 0x1 != 0;
                let mut target = self.cache.get(to);

                // Helper to compute an ArcSlice BufPart from a sub-slice of `backing`.
                let arc_part = |sub: &[u8]| -> BufPart {
                    let base = backing.as_ptr() as usize;
                    let ptr = sub.as_ptr() as usize;
                    let len = sub.len();
                    if ptr >= base && ptr + len <= base + backing.len() {
                        BufPart::ArcSlice {
                            data: backing.clone(),
                            offset: ptr - base,
                            len,
                        }
                    } else {
                        BufPart::Owned(sub.to_vec())
                    }
                };

                let mut messages: Vec<ReplyType> = Vec::new();
                if bcast_or_mcast || target.is_some_and(|x| x == self.device.mac_address()) {
                    messages.push(ReplyType::TapParts(vec![arc_part(data_slice)]));
                    target = None;
                }

                let routing = self.routing.read().unwrap();
        messages.extend(if bcast_or_mcast {
                    routing
                        .iter_next_hops()
            .filter(|x| x != &&source_mac)
                        .filter_map(|x| {
                            let route = routing.get_route_to(Some(*x))?;
                            Some((*x, route.mac))
                        })
                        .map(|(target_mac, next_hop)| {
                            // Build downstream frame parts
                            let mut parts: Vec<BufPart> = Vec::with_capacity(8);
                            parts.push(BufPart::Owned(next_hop.bytes().to_vec()));
                            parts.push(BufPart::Owned(self.device.mac_address().bytes().to_vec()));
                            parts.push(BufPart::Owned(vec![0x30, 0x30]));
                            parts.push(BufPart::Owned(vec![1u8])); // PacketType::Data
                            parts.push(BufPart::Owned(vec![1u8])); // Data::Downstream
                            parts.push(arc_part(source_slice)); // origin
                            parts.push(BufPart::Owned(target_mac.bytes().to_vec())); // destination
                            parts.push(arc_part(data_slice)); // payload from upstream
                            ReplyType::WireParts(parts)
                        })
                        .collect_vec()
                } else if let Some(target_mac) = target {
                    let Some(next_hop) = routing.get_route_to(Some(target_mac)) else {
                        return Ok(None);
                    };
                    vec![{
                        let mut parts: Vec<BufPart> = Vec::with_capacity(8);
                        parts.push(BufPart::Owned(next_hop.mac.bytes().to_vec()));
                        parts.push(BufPart::Owned(self.device.mac_address().bytes().to_vec()));
                        parts.push(BufPart::Owned(vec![0x30, 0x30]));
                        parts.push(BufPart::Owned(vec![1u8]));
                        parts.push(BufPart::Owned(vec![1u8]));
                        parts.push(arc_part(source_slice));
                        parts.push(BufPart::Owned(target_mac.bytes().to_vec()));
                        parts.push(arc_part(data_slice));
                        ReplyType::WireParts(parts)
                    }]
                } else {
                    vec![]
                });

                Ok(Some(messages))
            }
            PacketType::Control(_) | PacketType::Data(Data::Downstream(_)) => {
                // For other types, fall back to existing logic (small messages)
                self.handle_msg(msg).await
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
                let messages = node::tap_traffic(&tun, |pkt, _size| async move {
                    let data_arc = pkt; // contains exactly the TAP frame bytes
                    let data: &[u8] = &data_arc[..];
                    let to: [u8; 6] = data[0..6].try_into()?;
                    let to: MacAddress = to.into();
                    let target = cache.get(to);
                    let from: [u8; 6] = data[6..12].try_into()?;
                    let from: MacAddress = from.into();
                    let source_mac = devicec.mac_address().bytes();
                    cache.store_mac(from, devicec.mac_address());
                    let routing = routing.read().unwrap();
                    let outgoing = if let Some(target) = target {
                        let Some(hop) = routing.get_route_to(Some(target)) else {
                            bail!("no route");
                        };

                        // Build zero-copy parts for the downstream frame: headers owned, payload Arc-slice
                        let mut parts = Vec::with_capacity(6);
                        parts.push(BufPart::Owned(hop.mac.bytes().to_vec())); // L2 dest = next hop
                        parts.push(BufPart::Owned(devicec.mac_address().bytes().to_vec())); // L2 src
                        parts.push(BufPart::Owned(vec![0x30, 0x30])); // ethertype-like marker
                        parts.push(BufPart::Owned(vec![1u8])); // PacketType::Data
                        parts.push(BufPart::Owned(vec![1u8])); // Data::Downstream
                        parts.push(BufPart::Owned(source_mac.to_vec())); // origin (6)
                        parts.push(BufPart::Owned(target.bytes().to_vec())); // destination (6)
                        // payload: all remaining bytes of the original TAP frame starting at 12
                        parts.push(BufPart::ArcSlice { data: data_arc.clone(), offset: 0, len: data_arc.len() });
                        vec![ReplyType::WireParts(parts)]
                    } else {
                        routing
                            .iter_next_hops()
                            .filter(|x| x != &&devicec.mac_address())
                            .filter_map(|x| {
                                let dest = routing.get_route_to(Some(*x))?;
                                Some((x, dest))
                            })
                            .map(|(x, y)| (x, y.mac))
                            .unique_by(|(x, _)| *x)
                            .map(|(x, next_hop)| {
                                let mut parts = Vec::with_capacity(6);
                                parts.push(BufPart::Owned(next_hop.bytes().to_vec()));
                                parts.push(BufPart::Owned(devicec.mac_address().bytes().to_vec()));
                                parts.push(BufPart::Owned(vec![0x30, 0x30]));
                                parts.push(BufPart::Owned(vec![1u8])); // PacketType::Data
                                parts.push(BufPart::Owned(vec![1u8])); // Data::Downstream
                                parts.push(BufPart::Owned(source_mac.to_vec()));
                                parts.push(BufPart::Owned((*x).bytes().to_vec()));
                                parts.push(BufPart::ArcSlice { data: data_arc.clone(), offset: 0, len: data_arc.len() });
                                ReplyType::WireParts(parts)
                            })
                            .collect_vec()
                    };
                    tracing::trace!(?outgoing, "outgoing from tap");
                    Ok(Some(outgoing))
                })
                .await;

                if let Ok(Some(messages)) = messages {
                    let _ = node::handle_messages(messages, &tun, &device).await;
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
    use std::os::unix::io::FromRawFd;
    use std::sync::Arc;
    use tokio::io::unix::AsyncFd;
    use tokio::time::{sleep, Duration};

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
        let hbr = crate::messages::control::heartbeat::HeartbeatReply::from_sender(&hb, reply_sender);
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
        let msg = Message::new(from_client, dest_client, PacketType::Data(Data::Upstream(tu)));

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

    // --- New tests to cover RSU handle_msg_with_backing zero-copy paths ---
    #[tokio::test]
    async fn handle_msg_with_backing_broadcast_tap_parts() {
        use common::device::{Device, DeviceIo};
        use common::tun::{test_tun::TokioTun, Tun};

        let args = Args {
            bind: String::new(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            node_params: NodeParameters { node_type: NodeType::Rsu, hello_history: 2, hello_periodicity: Some(100) },
        };
        let routing = Arc::new(std::sync::RwLock::new(super::routing::Routing::new(&args).expect("routing")));
        let cache = Arc::new(crate::control::client_cache::ClientCache::default());

        let (a, _b) = TokioTun::new_pair();
        let tun = Arc::new(Tun::new_shim(a));

        // device backed by pipe
        let mut fds = [0; 2];
        unsafe { libc::pipe(fds.as_mut_ptr()) };
        let writer_fd = fds[1];
        // make writer non-blocking
        unsafe {
            let flags = libc::fcntl(writer_fd, libc::F_GETFL);
            if flags >= 0 { let _ = libc::fcntl(writer_fd, libc::F_SETFL, flags | libc::O_NONBLOCK); }
        }
        let dev_mac: MacAddress = [9u8; 6].into();
        let device = Arc::new(Device::from_asyncfd_for_bench(
            dev_mac,
            AsyncFd::new(unsafe { DeviceIo::from_raw_fd(writer_fd) }).unwrap(),
        ));

        // Construct RSU directly
        let rsu = super::Rsu { args, routing, tun, device, cache };

        // Build an upstream broadcast frame: dest ff:ff.., include from and payload in data
        let from: MacAddress = [1u8; 6].into();
        let dest = [255u8; 6];
        let mut inner = Vec::new();
        inner.extend_from_slice(&dest);
        inner.extend_from_slice(&from.bytes());
        inner.extend_from_slice(b"hello");
        let tu = crate::messages::data::ToUpstream::new(from, &inner);
        let msg_struct = crate::messages::message::Message::new(
            from,
            dest.into(),
            PacketType::Data(Data::Upstream(tu)),
        );
        let wire: Vec<Vec<u8>> = (&msg_struct).into();
        let raw: Vec<u8> = wire.iter().flat_map(|v| v.iter()).copied().collect();
        let arc: Arc<[u8]> = raw.into_boxed_slice().into();
        let parsed = crate::messages::message::Message::try_from(&arc[..]).expect("parse");

        let out = rsu
            .handle_msg_with_backing(&parsed, &arc)
            .await
            .expect("ok")
            .expect("some");
        assert!(!out.is_empty());
        // First entry should be TapParts containing ArcSlice
        let has_tap_parts = out.iter().any(|r| match r {
            super::ReplyType::TapParts(parts) => parts.iter().any(|p| matches!(p, super::BufPart::ArcSlice { .. })),
            _ => false,
        });
        assert!(has_tap_parts);

        // close read end
        unsafe { libc::close(fds[0]) };
    }

    #[tokio::test]
    async fn handle_msg_with_backing_unicast_to_self_tap_parts() {
        use common::device::{Device, DeviceIo};
        use common::tun::{test_tun::TokioTun, Tun};

        let args = Args {
            bind: String::new(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            node_params: NodeParameters { node_type: NodeType::Rsu, hello_history: 2, hello_periodicity: Some(100) },
        };
        let routing = Arc::new(std::sync::RwLock::new(super::routing::Routing::new(&args).expect("routing")));
        let cache = Arc::new(crate::control::client_cache::ClientCache::default());

        let (a, _b) = TokioTun::new_pair();
        let tun = Arc::new(Tun::new_shim(a));

        let mut fds = [0; 2];
        unsafe { libc::pipe(fds.as_mut_ptr()) };
        let writer_fd = fds[1];
        unsafe {
            let flags = libc::fcntl(writer_fd, libc::F_GETFL);
            if flags >= 0 { let _ = libc::fcntl(writer_fd, libc::F_SETFL, flags | libc::O_NONBLOCK); }
        }
        let dev_mac: MacAddress = [10u8; 6].into();
        let device = Arc::new(Device::from_asyncfd_for_bench(
            dev_mac,
            AsyncFd::new(unsafe { DeviceIo::from_raw_fd(writer_fd) }).unwrap(),
        ));

        // Map a client MAC to RSU itself so unicast to that client yields TapParts
        let client: MacAddress = [42u8; 6].into();
        cache.store_mac(client, dev_mac);

        let rsu = super::Rsu { args, routing, tun, device, cache };

        // create an upstream unicast to client
        let from: MacAddress = [1u8; 6].into();
        let mut inner = Vec::new();
        inner.extend_from_slice(&client.bytes());
        inner.extend_from_slice(&from.bytes());
        inner.extend_from_slice(b"payload");
        let tu = crate::messages::data::ToUpstream::new(from, &inner);
        let msg_struct = crate::messages::message::Message::new(
            from,
            client,
            PacketType::Data(Data::Upstream(tu)),
        );
        let wire: Vec<Vec<u8>> = (&msg_struct).into();
        let raw: Vec<u8> = wire.iter().flat_map(|v| v.iter()).copied().collect();
        let arc: Arc<[u8]> = raw.into_boxed_slice().into();
        let parsed = crate::messages::message::Message::try_from(&arc[..]).expect("parse");

        let out = rsu
            .handle_msg_with_backing(&parsed, &arc)
            .await
            .expect("ok")
            .expect("some");
        assert!(!out.is_empty());
        assert!(matches!(&out[0], super::ReplyType::TapParts(_)));
        // ensure payload part is ArcSlice
        match &out[0] {
            super::ReplyType::TapParts(parts) => {
                assert!(parts.iter().any(|p| matches!(p, super::BufPart::ArcSlice { .. })));
            }
            _ => unreachable!(),
        }

        unsafe { libc::close(fds[0]) };
    }

    #[tokio::test]
    async fn handle_msg_with_backing_unicast_forwards_wire_parts() {
        use common::device::{Device, DeviceIo};
        use common::tun::{test_tun::TokioTun, Tun};

        let args = Args {
            bind: String::new(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            node_params: NodeParameters { node_type: NodeType::Rsu, hello_history: 2, hello_periodicity: Some(100) },
        };
        let routing = Arc::new(std::sync::RwLock::new(super::routing::Routing::new(&args).expect("routing")));
        let cache = Arc::new(crate::control::client_cache::ClientCache::default());

        let (a, _b) = TokioTun::new_pair();
        let tun = Arc::new(Tun::new_shim(a));

        let mut fds = [0; 2];
        unsafe { libc::pipe(fds.as_mut_ptr()) };
        let writer_fd = fds[1];
        unsafe {
            let flags = libc::fcntl(writer_fd, libc::F_GETFL);
            if flags >= 0 { let _ = libc::fcntl(writer_fd, libc::F_SETFL, flags | libc::O_NONBLOCK); }
        }
        let dev_mac: MacAddress = [11u8; 6].into();
        let device = Arc::new(Device::from_asyncfd_for_bench(
            dev_mac,
            AsyncFd::new(unsafe { DeviceIo::from_raw_fd(writer_fd) }).unwrap(),
        ));

        // Seed routing with a next-hop for target_node
        let target_node: MacAddress = [77u8; 6].into();
        let next_hop: MacAddress = [88u8; 6].into();
        {
            let mut w = routing.write().unwrap();
            let _ = w.send_heartbeat(dev_mac); // id 0
        }
        {
            let hb0 = crate::messages::control::heartbeat::Heartbeat::new(
                std::time::Duration::from_millis(0),
                0u32,
                dev_mac,
            );
            let hbr = crate::messages::control::heartbeat::HeartbeatReply::from_sender(&hb0, target_node);
            let reply = crate::messages::message::Message::new(
                next_hop,
                [255u8; 6].into(),
                crate::messages::packet_type::PacketType::Control(
                    crate::messages::control::Control::HeartbeatReply(hbr),
                ),
            );
            let mut w = routing.write().unwrap();
            let _ = w.handle_heartbeat_reply(&reply, dev_mac).expect("hb reply ok");
        }

        // Map client to target_node so cache.get(to) returns Some(target_node)
        let client: MacAddress = [42u8; 6].into();
        cache.store_mac(client, target_node);

        let rsu = super::Rsu { args, routing, tun, device, cache };

        // Build upstream unicast frame to client
        let from: MacAddress = [1u8; 6].into();
        let mut inner = Vec::new();
        inner.extend_from_slice(&client.bytes());
        inner.extend_from_slice(&from.bytes());
        inner.extend_from_slice(b"xyz");
        let tu = crate::messages::data::ToUpstream::new(from, &inner);
        let msg_struct = crate::messages::message::Message::new(from, client, PacketType::Data(Data::Upstream(tu)));
        let wire: Vec<Vec<u8>> = (&msg_struct).into();
        let raw: Vec<u8> = wire.iter().flat_map(|v| v.iter()).copied().collect();
        let arc: Arc<[u8]> = raw.into_boxed_slice().into();
        let parsed = crate::messages::message::Message::try_from(&arc[..]).expect("parse");

        let out = rsu
            .handle_msg_with_backing(&parsed, &arc)
            .await
            .expect("ok")
            .expect("some");
        // Expect one or more WireParts replies
        assert!(out.iter().any(|r| matches!(r, super::ReplyType::WireParts(_))));
        // And at least two ArcSlices (origin and payload)
        let arc_slice_count: usize = out
            .iter()
            .filter_map(|r| match r { super::ReplyType::WireParts(parts) => Some(parts), _ => None })
            .flat_map(|parts| parts.iter())
            .filter(|p| matches!(p, super::BufPart::ArcSlice { .. }))
            .count();
        assert!(arc_slice_count >= 2);
    }

    #[tokio::test]
    async fn process_tap_traffic_unicast_produces_wireparts_and_writes_device() {
        use common::device::{Device, DeviceIo};
        use common::tun::{test_tun::TokioTun, Tun};

        // Build RSU components
        let args = Args {
            bind: String::new(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            node_params: crate::args::NodeParameters { node_type: crate::args::NodeType::Rsu, hello_history: 2, hello_periodicity: Some(50) },
        };

        let routing = Arc::new(std::sync::RwLock::new(super::routing::Routing::new(&args).expect("routing")));
        let cache = Arc::new(crate::control::client_cache::ClientCache::default());
        let (a, b) = TokioTun::new_pair();
        let tun = Arc::new(Tun::new_shim(a));

        // device writes to writer; we'll read from reader
        let mut fds = [0; 2];
        unsafe { libc::pipe(fds.as_mut_ptr()) };
        let reader_fd = fds[0];
        let writer_fd = fds[1];
        // non-blocking writer for AsyncFd; reader too for polling
        unsafe {
            let flags_w = libc::fcntl(writer_fd, libc::F_GETFL);
            if flags_w >= 0 { let _ = libc::fcntl(writer_fd, libc::F_SETFL, flags_w | libc::O_NONBLOCK); }
            let flags_r = libc::fcntl(reader_fd, libc::F_GETFL);
            if flags_r >= 0 { let _ = libc::fcntl(reader_fd, libc::F_SETFL, flags_r | libc::O_NONBLOCK); }
        }

        let dev_mac: MacAddress = [9u8; 6].into();
        let device = Arc::new(Device::from_asyncfd_for_bench(
            dev_mac,
            AsyncFd::new(unsafe { DeviceIo::from_raw_fd(writer_fd) }).unwrap(),
        ));

        let rsu = super::Rsu { args, routing: routing.clone(), tun: tun.clone(), device: device.clone(), cache: cache.clone() };
        // Spawn the processing task
        rsu.process_tap_traffic().expect("spawned");

        // Seed routing and cache so unicast forwarding is possible
        let target_node: MacAddress = [77u8; 6].into();
        let next_hop: MacAddress = [88u8; 6].into();
        // Send heartbeat from RSU to create sent-entry id 0
        {
            let mut w = routing.write().unwrap();
            let _ = w.send_heartbeat(dev_mac);
        }
        // Register HeartbeatReply to map sender=target_node via next_hop
        {
            let hb0 = crate::messages::control::heartbeat::Heartbeat::new(Duration::from_millis(0), 0u32, dev_mac);
            let hbr = crate::messages::control::heartbeat::HeartbeatReply::from_sender(&hb0, target_node);
            let reply = crate::messages::message::Message::new(
                next_hop,
                [255u8; 6].into(),
                crate::messages::packet_type::PacketType::Control(
                    crate::messages::control::Control::HeartbeatReply(hbr),
                ),
            );
            let mut w = routing.write().unwrap();
            let _ = w.handle_heartbeat_reply(&reply, dev_mac).expect("hb reply ok");
        }

        // Map a client dest to target_node
        let dest_client: MacAddress = [10u8; 6].into();
        cache.store_mac(dest_client, target_node);

        // Build a TAP frame: dest(6) + src(6) + payload
        let src_client = [1u8; 6];
        let payload = b"payload";
        let mut tap_frame = Vec::new();
        tap_frame.extend_from_slice(&dest_client.bytes());
        tap_frame.extend_from_slice(&src_client);
        tap_frame.extend_from_slice(payload);

        // Send into the RSU via peer side of the tun pair
        b.send_all(&tap_frame).await.expect("peer send");

        // Poll the reader fd for bytes written by device
        let mut out = vec![0u8; 2048];
        let mut total = 0;
        for _ in 0..50 { // up to ~50 * 2ms = 100ms
            let n = unsafe { libc::read(reader_fd, out.as_mut_ptr().cast(), out.len()) };
            if n > 0 {
                total = n as usize;
                break;
            }
            sleep(Duration::from_millis(2)).await;
        }
        assert!(total > 0, "no bytes forwarded to device");

        // Try to parse as a Message
        let msg = crate::messages::message::Message::try_from(&out[..total]).expect("parse forwarded msg");
        // Should be a downstream data frame to our next_hop
        assert_eq!(msg.to().unwrap(), next_hop);
        match msg.get_packet_type() { crate::messages::packet_type::PacketType::Data(_) => {}, _ => panic!("expected data") }
    }
}
