mod routing;

use super::{
    client_cache::ClientCache,
    node::{Node, ReplyType},
};
use crate::{
    dev::Device,
    messages::{
        control::Control,
        data::{Data, ToDownstream, ToUpstream},
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
use tracing::Instrument;

pub struct Rsu {
    args: Args,
    mac: MacAddress,
    routing: Arc<RwLock<Routing>>,
    cache: ClientCache,
}

impl Rsu {
    pub fn new(args: Args, mac: MacAddress) -> Result<Self> {
        let rsu = Self {
            routing: Arc::new(RwLock::new(Routing::new(&args)?)),
            args,
            mac,
            cache: ClientCache::default(),
        };

        tracing::info!(?rsu.args, %rsu.mac, "Setup Rsu");
        Ok(rsu)
    }
}

impl Node for Rsu {
    fn handle_msg(&self, msg: Message) -> Result<Option<Vec<ReplyType>>> {
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

    fn generate(&self, dev: Arc<Device>) {
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
                        match dev.send_vectored(&vec).await {
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

    fn get_route_to(&self, mac: Option<MacAddress>) -> Option<MacAddress> {
        self.routing
            .read()
            .unwrap()
            .get_route_to(mac)
            .map(|x| x.mac)
    }

    fn tap_traffic(&self, msg: ToUpstream) -> Result<Option<Vec<ReplyType>>> {
        let data = msg.data();
        let to: [u8; 6] = data[0..6].try_into()?;
        let to: MacAddress = to.into();
        let target = self.cache.get(to);
        let from: [u8; 6] = data[6..12].try_into()?;
        let from: MacAddress = from.into();
        let msg = ToDownstream::new(msg.source(), target.unwrap_or([255; 6].into()), data);
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

    fn get_mac(&self) -> MacAddress {
        self.mac
    }
}
