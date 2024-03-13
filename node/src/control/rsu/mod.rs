mod routing;

use super::{client_cache::ClientCache, node::ReplyType};
use crate::{
    control::node::{tap_traffic, wire_traffic},
    dev::Device,
    messages::{
        control::Control,
        data::{Data, ToDownstream},
        message::Message,
        packet_type::PacketType,
    },
    Args,
};
use anyhow::{anyhow, Result};
use itertools::Itertools;
use mac_address::MacAddress;
use routing::Routing;
use std::{
    io::IoSlice,
    sync::{Arc, RwLock},
    time::Duration,
};
use tokio_tun::Tun;
use tracing::Instrument;

pub struct Rsu {
    args: Args,
    mac: MacAddress,
    routing: Arc<RwLock<Routing>>,
    tun: Arc<Tun>,
    device: Arc<Device>,
    cache: ClientCache,
}

impl Rsu {
    pub fn new(args: Args, mac: MacAddress, tun: Arc<Tun>, device: Arc<Device>) -> Result<Self> {
        let rsu = Self {
            routing: Arc::new(RwLock::new(Routing::new(&args)?)),
            args,
            mac,
            tun,
            device,
            cache: ClientCache::default(),
        };

        rsu.generate();
        tracing::info!(?rsu.args, %rsu.mac, "Setup Rsu");
        Ok(rsu)
    }

    pub async fn process(&self) {
        tokio::select! {
            _ = wire_traffic(&self.tun, &self.device, |pkt, size| {
                async move {
                    let Ok(msg) = Message::try_from(&pkt[..size]) else {
                        return Ok(None);
                    };

                    self.handle_msg(&msg).await
                }
            }) => {},
            _ = tap_traffic(&self.tun, &self.device, |x, size| {
                async move {
                    let data: &[u8] = &x[..size];
                    let to: [u8; 6] = data[0..6].try_into()?;
                    let to: MacAddress = to.into();
                    let target = self.cache.get(to);
                    let from: [u8; 6] = data[6..12].try_into()?;
                    let from: MacAddress = from.into();
                    let source_mac = self.mac.bytes();
                    let msg = ToDownstream::new(&source_mac, target.unwrap_or([255; 6].into()), data);
                    let source_mac: [u8; 6] = msg.source().get(0..6).unwrap().try_into()?;
                    let source_mac: MacAddress = source_mac.into();
                    self.cache.store_mac(from, self.mac);
                    let routing = self.routing.read().unwrap();
                    Ok(Some(
                        routing
                            .iter_next_hops()
                            .filter(|x| x != &&source_mac)
                            .filter_map(|x| routing.get_route_to(Some(*x)))
                            .map(|x| x.mac)
                            .unique()
                            .map(|next_hop| {
                                let msg = Message::new(
                                    self.mac,
                                    next_hop,
                                    PacketType::Data(Data::Downstream(msg.clone())),
                                );
                                ReplyType::Wire((&msg).into())
                            })
                            .collect_vec(),
                    ))
                }
            }) => {}
        };
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
                    let span =
                        tracing::debug_span!(target: "hello", "hello task", rsu.mac=%self.mac);
                    let _g = span.enter();
                    self.routing
                        .write()
                        .unwrap()
                        .handle_heartbeat_reply(&msg, self.mac)?;
                }

                Ok(None)
            }
            PacketType::Data(Data::Downstream(_)) | PacketType::Control(Control::Heartbeat(_)) => {
                Ok(None)
            }
        }
    }

    fn generate(&self) {
        let device = self.device.clone();
        let mac = self.mac;
        let routing = self.routing.clone();
        let hello_periodicity = self.args.node_params.hello_periodicity;
        let span = tracing::error_span!(target: "hello", "hello task", rsu.mac=%mac);
        let _g = span.enter();
        if let Some(hello_periodicity) = hello_periodicity {
            tokio::spawn(
                async move {
                    loop {
                        tokio::time::sleep(Duration::from_millis(hello_periodicity.into())).await;
                        let msg: Vec<Vec<u8>> = {
                            let mut routing = routing.write().unwrap();
                            let msg = routing.send_heartbeat(mac);
                            tracing::trace!(target: "pkt", ?msg, "pkt");
                            (&msg).into()
                        };
                        let vec: Vec<IoSlice> = msg.iter().map(|x| IoSlice::new(x)).collect();
                        match device.send_vectored(&vec).await {
                            Ok(_) => tracing::trace!("sent hello"),
                            Err(e) => tracing::error!(?e, "error sending hello"),
                        };
                    }
                }
                .in_current_span(),
            );
        } else {
            tracing::error!(?self.args, "Rsu configured without hello_periodicity parameter");
        }
    }
}
