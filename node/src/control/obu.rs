use super::{
    node::{Node, ReplyType},
    Args,
};
use crate::{
    dev::Device,
    messages::{ControlType, HeartBeatReply, Message, PacketType},
};
use anyhow::{bail, Result};
use indexmap::IndexMap;
use mac_address::MacAddress;
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};

#[derive(Debug)]
struct RoutingTarget {
    hops: u32,
    mac: MacAddress,
    latency: Duration,
}

#[derive(Debug)]
struct Routing {
    args: Args,
    boot: Instant,
    upstream: HashMap<MacAddress, IndexMap<u32, (Duration, MacAddress, u32)>>,
    downstream:
        HashMap<MacAddress, IndexMap<u32, (Duration, HashMap<MacAddress, Vec<RoutingTarget>>)>>,
}

impl Routing {
    fn new(args: &Args, boot: &Instant) -> Result<Self> {
        if args.node_params.hello_history == 0 {
            bail!("we need to be able to store at least 1 hello");
        }
        Ok(Self {
            args: args.clone(),
            boot: boot.clone(),
            upstream: HashMap::default(),
            downstream: HashMap::default(),
        })
    }

    fn handle_heartbeat(
        &mut self,
        pkt: &Message,
        mac: &MacAddress,
    ) -> Result<Option<Vec<ReplyType>>> {
        let Ok(PacketType::Control(ControlType::HeartBeat(message))) = pkt.next_layer() else {
            bail!("this is supposed to be a HeartBeat");
        };

        let entry = self
            .upstream
            .entry(message.source)
            .or_insert(IndexMap::with_capacity(usize::try_from(
                self.args.node_params.hello_history,
            )?));

        if entry.first().is_some_and(|(x, _)| x > &message.id) {
            entry.clear();
        }

        if entry.len() == entry.capacity() && entry.capacity() > 0 {
            entry.swap_remove_index(0);
        }

        if let Some((_, _, hops)) = entry.get(&message.id) {
            if hops <= &message.hops {
                return Ok(None);
            }
        }

        let from: [u8; 6] = pkt.from().try_into()?;
        entry.insert(
            message.id,
            (
                Instant::now().duration_since(self.boot),
                from.into(),
                message.hops,
            ),
        );

        Ok(Some(vec![
            ReplyType::Wire(
                Message::new(
                    mac.bytes(),
                    [255; 6],
                    &PacketType::Control(ControlType::HeartBeat(message.clone())),
                )
                .into(),
            ),
            ReplyType::Wire(
                Message::new(
                    mac.bytes(),
                    from,
                    &PacketType::Control(ControlType::HeartBeatReply(HeartBeatReply::from_sender(
                        message, *mac,
                    ))),
                )
                .into(),
            ),
        ]))
    }

    fn handle_heartbeat_reply(
        &mut self,
        pkt: &Message,
        mac: &MacAddress,
    ) -> Result<Option<Vec<ReplyType>>> {
        let Ok(PacketType::Control(ControlType::HeartBeatReply(message))) = pkt.next_layer() else {
            bail!("this is supposed to be a HeartBeat Reply");
        };

        let Some(source_entries) = self.upstream.get(&message.source) else {
            bail!("we don't know how to reach that source");
        };

        let Some(source) = source_entries.get(&message.id) else {
            bail!("no recollection of the next hop for this route");
        };

        Ok(Some(vec![ReplyType::Wire(
            Message::new(
                mac.bytes(),
                source.1.bytes(),
                &PacketType::Control(ControlType::HeartBeatReply(message)),
            )
            .into(),
        )]))
    }

    fn get_route_to(&self, mac: &MacAddress) -> Option<MacAddress> {
        todo!()
    }
}

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
    fn handle_msg(&self, msg: &Message) -> Result<Option<Vec<ReplyType>>> {
        match msg.next_layer() {
            Ok(PacketType::Data(buf)) => Ok(Some(vec![ReplyType::Tap(vec![buf.into()])])),
            Ok(PacketType::Control(ControlType::HeartBeat(_))) => self
                .routing
                .write()
                .unwrap()
                .handle_heartbeat(msg, &self.mac),
            Ok(PacketType::Control(ControlType::HeartBeatReply(_))) => self
                .routing
                .write()
                .unwrap()
                .handle_heartbeat_reply(msg, &self.mac),
            Err(e) => {
                tracing::error!(?e, "error getting message layer");
                bail!(e)
            }
        }
    }

    fn generate(&self, _dev: Arc<Device>) {}

    fn get_route_to(&self, mac: &MacAddress) -> Option<MacAddress> {
        self.routing.read().unwrap().get_route_to(mac)
    }
}
