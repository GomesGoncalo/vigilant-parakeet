mod routing;
mod session;

use self::session::Session;

use super::{
    client_cache::ClientCache,
    node::{Node, ReplyType},
};
use crate::{
    dev::Device,
    messages::{
        ControlType, Data, DownstreamData, Message, PacketType, SessionResponse, UpstreamData,
    },
    Args,
};
use anyhow::{bail, Result};
use itertools::Itertools;
use mac_address::MacAddress;
use routing::Routing;
use std::{
    collections::HashMap,
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
    sessions: HashMap<MacAddress, Session>,
}

impl Rsu {
    pub fn new(args: Args, mac: MacAddress) -> Result<Self> {
        let rsu = Self {
            routing: Arc::new(RwLock::new(Routing::new(&args)?)),
            args,
            mac,
            cache: ClientCache::default(),
            sessions: HashMap::default(),
        };

        tracing::info!(?rsu.args, %rsu.mac, "Setup Rsu");
        Ok(rsu)
    }
}

impl Node for Rsu {
    fn handle_msg(&self, msg: Message) -> Result<Option<Vec<ReplyType>>> {
        match msg.next_layer() {
            Ok(PacketType::Data(Data::Downstream(buf))) => {
                let to: [u8; 6] = buf.data[0..6].try_into()?;
                let to: MacAddress = to.into();
                let from: [u8; 6] = buf.data[6..12].try_into()?;
                let from: MacAddress = from.into();
                self.cache.store_mac(from, buf.source);
                let bcast_or_mcast = to == [255; 6].into() || to.bytes()[0] & 0x1 != 0;
                let mut target = self.cache.get(to);
                let mut messages = Vec::with_capacity(1);
                if bcast_or_mcast || target.is_some_and(|x| x == self.mac) {
                    messages.push(ReplyType::Tap(vec![buf.data.into()]));
                    target = None;
                }

                let routing = self.routing.read().unwrap();
                messages.extend(if bcast_or_mcast {
                    routing
                        .iter_next_hops()
                        .filter(|x| x != &&buf.source)
                        .filter_map(|x| {
                            let route = routing.get_route_to(Some(*x))?;
                            Some((*x, route.mac))
                        })
                        .map(|(target, next_hop)| {
                            ReplyType::Wire(
                                Message::new(
                                    self.mac.bytes(),
                                    next_hop.bytes(),
                                    &PacketType::Data(Data::Upstream(Arc::new(UpstreamData::new(
                                        buf.source, target, buf.data,
                                    )))),
                                )
                                .into(),
                            )
                        })
                        .collect_vec()
                } else if let Some(target) = target {
                    let Some(next_hop) = routing.get_route_to(Some(target)) else {
                        return Ok(None);
                    };

                    vec![ReplyType::Wire(
                        Message::new(
                            self.mac.bytes(),
                            next_hop.mac.bytes(),
                            &PacketType::Data(Data::Upstream(Arc::new(UpstreamData::new(
                                buf.source, target, buf.data,
                            )))),
                        )
                        .into(),
                    )]
                } else {
                    vec![]
                });

                Ok(Some(messages))
            }
            Ok(PacketType::Control(ControlType::HeartBeatReply(hbr))) => {
                if hbr.source == self.mac {
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
            Ok(PacketType::Control(ControlType::SessionRequest(session_req))) => {
                let routing = self.routing.read().unwrap();
                let Some(next_hop) = routing.get_route_to(Some(session_req.source)) else {
                    return Ok(None);
                };

                Ok(Some(vec![ReplyType::Wire(
                    Message::new(
                        self.mac.bytes(),
                        next_hop.mac.bytes(),
                        &PacketType::Control(ControlType::SessionResponse(SessionResponse::new(
                            session_req.source,
                            session_req.duration,
                        ))),
                    )
                    .into(),
                )]))
            }
            Ok(
                PacketType::Control(ControlType::SessionResponse(_))
                | PacketType::Data(Data::Upstream(_))
                | PacketType::Control(ControlType::HeartBeat(_)),
            ) => Ok(None),
            Err(e) => {
                tracing::error!(?e, "error getting message layer");
                bail!(e)
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
                        let msg = routing.write().unwrap().send_heartbeat(mac);
                        tracing::trace!(target: "pkt", ?msg, "pkt");
                        let msg: Vec<Arc<[u8]>> = msg.into();
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

    fn tap_traffic(&self, msg: Arc<DownstreamData>) -> Result<Option<Vec<ReplyType>>> {
        let to: [u8; 6] = msg.data[0..6].try_into()?;
        let to: MacAddress = to.into();
        let target = self.cache.get(to);
        let from: [u8; 6] = msg.data[6..12].try_into()?;
        let from: MacAddress = from.into();
        self.cache.store_mac(from, self.mac);
        let msg = Arc::new(UpstreamData::new(
            msg.source,
            target.unwrap_or([255; 6].into()),
            msg.data,
        ));
        let routing = self.routing.read().unwrap();
        Ok(Some(
            routing
                .iter_next_hops()
                .filter(|x| x != &&msg.source)
                .filter_map(|x| routing.get_route_to(Some(*x)))
                .map(|x| x.mac)
                .unique()
                .map(|next_hop| {
                    ReplyType::Wire(
                        Message::new(
                            self.mac.bytes(),
                            next_hop.bytes(),
                            &PacketType::Data(Data::Upstream(msg.clone())),
                        )
                        .into(),
                    )
                })
                .collect_vec(),
        ))
    }

    fn get_mac(&self) -> MacAddress {
        self.mac
    }
}
