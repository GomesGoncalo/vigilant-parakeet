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
                        match Message::try_from(&pkt[..size]) {
                            Ok(msg) => {
                                tracing::trace!(parsed = ?msg, "rsu wire_traffic parsed message");
                                let response = rsu.handle_msg(&msg).await;
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
                    let outgoing = if let Some(target) = target {
                        let Some(hop) = routing.get_route_to(Some(target)) else {
                            bail!("no route");
                        };

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
                                let msg = Message::new(
                                    devicec.mac_address(),
                                    next_hop,
                                    PacketType::Data(Data::Downstream(ToDownstream::new(
                                        &source_mac,
                                        *x,
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
