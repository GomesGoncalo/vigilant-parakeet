mod routing;

use super::node::{Node, ReplyType};
use crate::{
    dev::Device,
    messages::{ControlType, Data, Message, PacketType},
    Args,
};
use anyhow::{bail, Result};
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
        match msg.next_layer() {
            Ok(PacketType::Data(buf)) => {
                let routing = self.routing.read().unwrap();
                if routing
                    .iter_upstream()
                    .any(|upstream| upstream == &msg.from())
                {
                    let mut reply = routing
                        .iter_next_hops()
                        .filter_map(|x| routing.get_route_to(Some(*x)))
                        .map(|x| x.mac)
                        .unique()
                        .map(|next_hop| {
                            ReplyType::Wire(
                                Message::new(
                                    self.mac.bytes(),
                                    next_hop.bytes(),
                                    &PacketType::Data(buf.clone()),
                                )
                                .into(),
                            )
                        })
                        .collect_vec();
                    if buf.source != self.mac {
                        reply.push(ReplyType::Tap(vec![buf.data.into()]));
                    }
                    Ok(Some(reply))
                } else {
                    let Some(upstream) = routing.get_route_to(None) else {
                        return Ok(None);
                    };

                    Ok(Some(vec![ReplyType::Wire(
                        Message::new(
                            self.mac.bytes(),
                            upstream.mac.bytes(),
                            &PacketType::Data(buf),
                        )
                        .into(),
                    )]))
                }
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
            Err(e) => {
                tracing::error!(?e, "error getting message layer");
                bail!(e)
            }
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

    fn tap_traffic(&self, msg: Arc<Data>) -> Result<Option<Vec<ReplyType>>> {
        let Some(upstream) = self.routing.read().unwrap().get_route_to(None) else {
            return Ok(None);
        };

        Ok(Some(vec![ReplyType::Wire(
            Message::new(
                self.mac.bytes(),
                upstream.mac.bytes(),
                &PacketType::Data(msg),
            )
            .into(),
        )]))
    }

    fn get_mac(&self) -> MacAddress {
        self.mac
    }
}
