mod routing;

use super::{client_cache::ClientCache, node::ReplyType};
use crate::{
    control::node,
    dev::Device,
    messages::{
        control::Control,
        data::{Data, ToDownstream},
        message::Message,
        packet_type::PacketType,
    },
    Args,
};
use anyhow::{anyhow, bail, Result};
use itertools::Itertools;
use mac_address::MacAddress;
use routing::Routing;
use std::{
    io::IoSlice,
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};
use tokio_tun::Tun;

pub struct Rsu {
    args: Args,
    mac: MacAddress,
    routing: Arc<RwLock<Routing>>,
    tun: Arc<Tun>,
    device: Arc<Device>,
    cache: ClientCache,
    next_hello: RwLock<Option<Instant>>,
}

impl Rsu {
    pub fn new(args: Args, mac: MacAddress, tun: Arc<Tun>, device: Arc<Device>) -> Result<Self> {
        let rsu = Self {
            routing: Arc::new(RwLock::new(Routing::new(&args, mac.clone())?)),
            args,
            mac,
            tun,
            device,
            cache: ClientCache::default(),
            next_hello: None.into(),
        };

        tracing::info!(?rsu.args, "Setup Rsu");
        Ok(rsu)
    }

    pub async fn process(&self) -> Result<()> {
        if let Some(p) = self.args.node_params.hello_periodicity {
            tokio::select! {
                _ = self.generate_hello() => {
                    let mut next_hello = self.next_hello.write().unwrap();
                    *next_hello = Some(Instant::now() + Duration::from_millis(p.into()));
                },
                m = node::tap_traffic(&self.tun, |pkt, size| {
                    async move {
                        let data: &[u8] = &pkt[..size];
                        let to: [u8; 6] = data[0..6].try_into()?;
                        let to: MacAddress = to.into();
                        let target = self.cache.get(to);
                        let from: [u8; 6] = data[6..12].try_into()?;
                        let from: MacAddress = from.into();
                        let source_mac = self.mac.bytes();
                        self.cache.store_mac(from, self.mac);
                        match target {
                            Some(target) => {
                                let routing = self.routing.read().unwrap();
                                let Some(hop) = routing.get_route_to(Some(target)) else {
                                    bail!("no route");
                                };
                                let msg = Message::new(
                                    self.mac,
                                    hop.mac,
                                    PacketType::Data(Data::Downstream(ToDownstream::new(
                                        &source_mac,
                                        target,
                                        data,
                                    ))),
                                );

                                let outgoing = vec![ReplyType::Wire((&msg).into())];
                                tracing::trace!(?outgoing, "outgoing from tap");
                                Ok(Some(outgoing))
                            }
                            None => {
                                let routing = self.routing.read().unwrap();
                                let outgoing = routing
                                    .iter_next_hops()
                                    .filter(|x| x != &&self.mac)
                                    .filter_map(|x| {
                                        let Some(dest) = routing.get_route_to(Some(*x)) else {
                                            return None;
                                        };
                                        Some((x, dest))
                                    })
                                    .map(|(x, y)| (x, y.mac))
                                    .unique_by(|(x, _)| *x)
                                    .map(|(x, next_hop)| {
                                        let msg = Message::new(
                                            self.mac,
                                            next_hop,
                                            PacketType::Data(Data::Downstream(ToDownstream::new(
                                                &source_mac,
                                                *x,
                                                data,
                                            ))),
                                        );
                                        ReplyType::Wire((&msg).into())
                                    })
                                    .collect_vec();

                                tracing::trace!(?outgoing, "outgoing from tap");
                                Ok(Some(outgoing))
                            }
                        }
                    }
                }) => {
                    if let Ok(Some(messages)) = m {
                        let _ = node::handle_messages(messages, &self.tun, &self.device).await;
                    }
                },
                m = node::wire_traffic(&self.device, |pkt, size| {
                    async move {
                        let Ok(msg) = Message::try_from(&pkt[..size]) else {
                            return Ok(None);
                        };

                        let response = self.handle_msg(&msg).await;
                        tracing::trace!(incoming = ?msg, outgoing = ?node::get_msgs(&response), "transaction");
                        response
                    }
                }) => {
                    if let Ok(Some(messages)) = m {
                        let _ = node::handle_messages(messages, &self.tun, &self.device).await;
                    }
                }
            };
            Ok(())
        } else {
            bail!("we cannot process anything without heartbeats")
        }
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
                if bcast_or_mcast || target.is_some_and(|x| x == self.mac) {
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
                                    self.mac,
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
                            self.mac,
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
                if hbr.source() == self.mac {
                    self.routing
                        .write()
                        .unwrap()
                        .handle_heartbeat_reply(msg, self.mac)
                } else {
                    Ok(None)
                }
            }
            PacketType::Data(Data::Downstream(_))
            | PacketType::Control(Control::Heartbeat(_))
            | PacketType::Control(Control::HeartbeatAck(_)) => Ok(None),
        }
    }

    async fn generate_hello(&self) {
        let duration = match *self.next_hello.read().unwrap() {
            None => None,
            Some(instant) => {
                let sleeping = instant.duration_since(Instant::now());
                if sleeping.is_zero() {
                    None
                } else {
                    Some(sleeping)
                }
            }
        };

        if let Some(duration) = duration {
            tracing::trace!(?duration, "sleeping for");
            let _ = tokio_timerfd::sleep(duration).await;
        }

        let msg: Vec<Vec<u8>> = {
            let mut routing = self.routing.write().unwrap();
            let msg = routing.send_heartbeat(self.mac);
            tracing::trace!(?msg, "generated hello");
            (&msg).into()
        };
        let vec: Vec<IoSlice> = msg.iter().map(|x| IoSlice::new(x)).collect();
        let _ = self
            .device
            .send_vectored(&vec)
            .await
            .inspect_err(|e| tracing::error!(?e, "error sending hello"));
    }
}
