use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
    time::Instant,
};

use anyhow::{bail, Result};
use mac_address::MacAddress;

use crate::{
    dev::Device,
    messages::{ControlType, Message, PacketType},
};

use super::{
    node::{Node, ReplyType},
    Args,
};

#[derive(Default)]
struct UpstreamState {
    state: HashMap<u32, (MacAddress, u32)>,
    sequences: Vec<u32>,
}

#[derive(Default)]
struct DownstreamState {
    state: HashMap<u32, (MacAddress, u32)>,
    sequences: Vec<u32>,
}

pub struct Routing {
    args: Args,
    boot: Arc<Instant>,
    mac_address: MacAddress,
    upstream: RwLock<UpstreamState>,
    downstream: RwLock<DownstreamState>,
}

impl Routing {
    pub fn new(args: Args, mac_address: MacAddress) -> Self {
        Self {
            args,
            mac_address,
            boot: Instant::now().into(),
            upstream: UpstreamState::default().into(),
            downstream: DownstreamState::default().into(),
        }
    }
}

impl Node for Routing {
    fn handle_msg(&self, msg: &Message) -> Result<Option<Vec<ReplyType>>> {
        Ok(match msg.next_layer() {
            Ok(PacketType::Data(buf)) => Some(vec![ReplyType::Tap(vec![buf.into()])]),
            Ok(PacketType::Control(ControlType::HeartBeat(hb))) => todo!(),
            Ok(PacketType::Control(ControlType::HeartBeatReply(hb))) => todo!(),
            Err(e) => bail!(e),
        })
    }

    fn generate(&self, _dev: Arc<Device>) {
        todo!()
    }
}
