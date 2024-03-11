mod routing;

use super::node::{Node, ReplyType};
use crate::{
    dev::Device,
    messages::{
        control::Control,
        data::{Data, ToUpstream},
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
    sync::{Arc, RwLock},
    time::Instant,
};

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
        match msg.get_packet_type() {
            PacketType::Data(Data::Upstream(buf)) => {
                let routing = self.routing.read().unwrap();
                let Some(upstream) = routing.get_route_to(None) else {
                    return Ok(None);
                };

                Ok(Some(vec![ReplyType::Wire(
                    (&Message::new(
                        self.mac,
                        upstream.mac,
                        PacketType::Data(Data::Upstream(buf.clone())),
                    ))
                        .into(),
                )]))
            }
            PacketType::Data(Data::Downstream(buf)) => {
                let mut reply = vec![];
                let destination: [u8; 6] = buf
                    .destination()
                    .get(0..6)
                    .ok_or_else(|| anyhow!("error"))?
                    .try_into()?;
                let destination: MacAddress = destination.into();
                if destination == self.get_mac() || destination == [255; 6].into() {
                    reply.push(ReplyType::Tap(vec![buf.data().to_vec()]));
                    return Ok(Some(reply));
                }

                let target = destination;
                let routing = self.routing.read().unwrap();
                reply.extend(if target == [255; 6].into() {
                    routing
                        .iter_next_hops()
                        .filter_map(|x| routing.get_route_to(Some(*x)))
                        .map(|x| x.mac)
                        .unique()
                        .map(|next_hop| {
                            let msg = (&Message::new(
                                self.mac,
                                next_hop,
                                PacketType::Data(Data::Downstream(buf.clone())),
                            ))
                                .into();
                            ReplyType::Wire(msg)
                        })
                        .collect_vec()
                } else {
                    let Some(next_hop) = routing.get_route_to(Some(target)) else {
                        return Ok(None);
                    };

                    vec![ReplyType::Wire(
                        (&Message::new(
                            self.mac,
                            next_hop.mac,
                            PacketType::Data(Data::Downstream(buf.clone())),
                        ))
                            .into(),
                    )]
                });
                Ok(Some(reply))
            }
            PacketType::Control(Control::Heartbeat(_)) => self
                .routing
                .write()
                .unwrap()
                .handle_heartbeat(&msg, self.mac),
            PacketType::Control(Control::HeartbeatReply(_)) => self
                .routing
                .write()
                .unwrap()
                .handle_heartbeat_reply(&msg, self.mac),
        }
    }

    fn generate(&self, _dev: Arc<Device>) {}

    fn get_route_to(&self, mac: Option<MacAddress>) -> Option<MacAddress> {
        self.routing
            .read()
            .unwrap()
            .get_route_to(mac)
            .map(|x| x.mac)
    }

    fn tap_traffic(&self, msg: ToUpstream) -> Result<Option<Vec<ReplyType>>> {
        let Some(upstream) = self.routing.read().unwrap().get_route_to(None) else {
            return Ok(None);
        };

        Ok(Some(vec![ReplyType::Wire(
            (&Message::new(
                self.mac,
                upstream.mac,
                PacketType::Data(Data::Upstream(msg)),
            ))
                .into(),
        )]))
    }

    fn get_mac(&self) -> MacAddress {
        self.mac
    }
}
