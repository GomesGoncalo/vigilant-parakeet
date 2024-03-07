mod routing;
mod session;

use super::node::{Node, ReplyType};
use crate::{
    dev::Device,
    messages::{ControlType, Data, DownstreamData, Message, PacketType, SessionRequest},
    Args,
};
use anyhow::{bail, Result};
use itertools::Itertools;
use mac_address::MacAddress;
use routing::Routing;
use std::{
    io::IoSlice,
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};
use tracing::Instrument;

pub struct Obu {
    args: Args,
    routing: Arc<RwLock<Routing>>,
    mac: MacAddress,
}

impl Obu {
    pub fn new(args: Args, mac: MacAddress) -> Result<Self> {
        let boot = Instant::now();
        let obu = Self {
            routing: Arc::new(RwLock::new(Routing::new(&args, &boot)?)),
            args,
            mac,
        };

        tracing::info!(?obu.args, %obu.mac, "Setup Obu");
        Ok(obu)
    }
}

impl Node for Obu {
    fn handle_msg(&self, msg: Message) -> Result<Option<Vec<ReplyType>>> {
        match msg.next_layer() {
            Ok(PacketType::Data(Data::Downstream(buf))) => {
                let routing = self.routing.read().unwrap();
                let Some(upstream) = routing.get_route_to(None) else {
                    return Ok(None);
                };

                Ok(Some(vec![ReplyType::Wire(
                    Message::new(
                        self.mac.bytes(),
                        upstream.mac.bytes(),
                        &PacketType::Data(Data::Downstream(buf)),
                    )
                    .into(),
                )]))
            }
            Ok(PacketType::Data(Data::Upstream(buf))) => {
                let mut reply = vec![];
                if buf.destination == self.get_mac() || buf.destination == [255; 6].into() {
                    reply.push(ReplyType::Tap(vec![buf.data.into()]));
                    return Ok(Some(reply));
                }

                let target = buf.destination;
                let routing = self.routing.read().unwrap();
                reply.extend(if target == [255; 6].into() {
                    routing
                        .iter_next_hops()
                        .filter_map(|x| routing.get_route_to(Some(*x)))
                        .map(|x| x.mac)
                        .unique()
                        .map(|next_hop| {
                            ReplyType::Wire(
                                Message::new(
                                    self.mac.bytes(),
                                    next_hop.bytes(),
                                    &PacketType::Data(Data::Upstream(buf.clone())),
                                )
                                .into(),
                            )
                        })
                        .collect_vec()
                } else {
                    let Some(next_hop) = routing.get_route_to(Some(target)) else {
                        return Ok(None);
                    };

                    vec![ReplyType::Wire(
                        Message::new(
                            self.mac.bytes(),
                            next_hop.mac.bytes(),
                            &PacketType::Data(Data::Upstream(buf)),
                        )
                        .into(),
                    )]
                });
                Ok(Some(reply))
            }
            Ok(PacketType::Control(ControlType::HeartBeat(_))) => self
                .routing
                .write()
                .unwrap()
                .handle_heartbeat(&msg, self.mac),
            Ok(PacketType::Control(ControlType::HeartBeatReply(_))) => self
                .routing
                .write()
                .unwrap()
                .handle_heartbeat_reply(&msg, self.mac),
            Ok(PacketType::Control(ControlType::SessionRequest(session_req))) => {
                let routing = self.routing.read().unwrap();
                let Some(upstream) = routing.get_route_to(None) else {
                    return Ok(None);
                };

                Ok(Some(vec![ReplyType::Wire(
                    Message::new(
                        self.mac.bytes(),
                        upstream.mac.bytes(),
                        &PacketType::Control(ControlType::SessionRequest(session_req)),
                    )
                    .into(),
                )]))
            }
            Ok(PacketType::Control(ControlType::SessionResponse(session_res))) => {
                if session_res.source == self.get_mac() {
                    tracing::info!(?session_res, "Got the response");
                    return Ok(None);
                }

                let routing = self.routing.read().unwrap();
                let Some(next_hop) = routing.get_route_to(Some(session_res.source)) else {
                    return Ok(None);
                };

                Ok(Some(vec![ReplyType::Wire(
                    Message::new(
                        self.mac.bytes(),
                        next_hop.mac.bytes(),
                        &PacketType::Control(ControlType::SessionResponse(session_res)),
                    )
                    .into(),
                )]))
            }
            Err(e) => {
                tracing::error!(?e, "error getting message layer");
                bail!(e)
            }
        }
    }

    fn generate(&self, dev: Arc<Device>) {
        let mac = self.mac;
        let routing = self.routing.clone();
        tokio::spawn(
            async move {
                loop {
                    let msg = if let Some(upstream) = routing.read().unwrap().get_route_to(None) {
                        Some(Message::new(
                            mac.bytes(),
                            upstream.mac.bytes(),
                            &PacketType::Control(ControlType::SessionRequest(SessionRequest::new(
                                mac,
                                Duration::from_secs(1800),
                            ))),
                        ))
                    } else {
                        None
                    };

                    if let Some(msg) = msg {
                        tracing::info!(%mac, "renewing session");
                        let msg: Vec<Arc<[u8]>> = msg.into();
                        let vec: Vec<IoSlice> = msg.iter().map(|x| IoSlice::new(x)).collect();
                        match dev.send_vectored(&vec).await {
                            Ok(_) => tracing::trace!("sent hello"),
                            Err(e) => tracing::error!(?e, "error sending hello"),
                        };
                    }
                    tokio::time::sleep(Duration::from_secs(10)).await;
                }
            }
            .in_current_span(),
        );
    }

    fn get_route_to(&self, mac: Option<MacAddress>) -> Option<MacAddress> {
        self.routing
            .read()
            .unwrap()
            .get_route_to(mac)
            .map(|x| x.mac)
    }

    fn tap_traffic(&self, msg: Arc<DownstreamData>) -> Result<Option<Vec<ReplyType>>> {
        let Some(upstream) = self.routing.read().unwrap().get_route_to(None) else {
            return Ok(None);
        };

        Ok(Some(vec![ReplyType::Wire(
            Message::new(
                self.mac.bytes(),
                upstream.mac.bytes(),
                &PacketType::Data(Data::Downstream(msg)),
            )
            .into(),
        )]))
    }

    fn get_mac(&self) -> MacAddress {
        self.mac
    }
}
