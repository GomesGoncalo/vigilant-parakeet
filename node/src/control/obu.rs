use super::{
    node::{Node, ReplyType},
    routing::Routing,
    Args,
};
use crate::{
    dev::Device,
    messages::{ControlType, Message, PacketType},
};
use anyhow::{bail, Context, Result};
use mac_address::MacAddress;
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};

pub struct Obu {
    args: Args,
    boot: Instant,
    mac_address: MacAddress,
    upstream_state: Arc<RwLock<(HashMap<u32, (MacAddress, Duration, u32)>, Vec<u32>)>>,
    hello_seq: Arc<
        RwLock<(
            HashMap<(MacAddress, u32), HashMap<MacAddress, (Duration, u32)>>,
            Vec<(MacAddress, u32)>,
        )>,
    >,
    routing: Routing,
}

impl Obu {
    pub fn new(args: Args, mac_address: MacAddress) -> Self {
        let obu = Self {
            args: args.clone(),
            boot: Instant::now(),
            mac_address,
            upstream_state: Arc::new(RwLock::new((HashMap::new(), Vec::new()))),
            hello_seq: Arc::new(RwLock::new((HashMap::new(), Vec::new()))),
            routing: Routing::new(args, mac_address),
        };

        tracing::info!(?obu.args, %obu.mac_address, "Setup Obu");
        obu
    }
}

impl Node for Obu {
    fn generate(&self, _dev: Arc<Device>) {}

    fn handle_msg(&self, msg: &Message) -> Result<Option<Vec<ReplyType>>> {
        let span = tracing::info_span!("self mac", ?self.mac_address);
        let _guard = span.enter();
        match msg.next_layer() {
            Ok(PacketType::Data(_)) => self.routing.handle_msg(msg),
            Ok(PacketType::Control(ControlType::HeartBeat(hb))) => {
                let mut hello_state_guard = self.upstream_state.write().unwrap();
                let (ref mut map, ref mut vec) = *hello_state_guard;

                if !vec.is_empty()
                    && hb.hops
                        > map
                            .get(vec.last().context("no last")?)
                            .context("no upstream")?
                            .2
                {
                    return Ok(None);
                }

                let contained = map.contains_key(&hb.id);

                map.entry(hb.id).or_insert((
                    MacAddress::new(msg.from().try_into()?),
                    Instant::now().duration_since(self.boot),
                    hb.hops,
                ));

                if !contained {
                    if vec.len() >= self.args.node_params.hello_history.try_into()? {
                        enum Result {
                            Replaced(u32),
                            AddAfter(u32),
                        }
                        match match vec.get_mut(0) {
                            Some(old_id) => Result::Replaced(std::mem::replace(old_id, hb.id)),
                            None => Result::AddAfter(hb.id),
                        } {
                            Result::AddAfter(id) => {
                                vec.push(id);
                            }
                            Result::Replaced(id) => {
                                map.remove(&id);
                            }
                        };
                    } else {
                        vec.push(hb.id);
                    }
                    vec.sort_unstable();
                }

                Ok(vec![
                    ReplyType::Wire(
                        Message::new(
                            self.mac_address.bytes(),
                            [255; 6],
                            &PacketType::Control(ControlType::HeartBeat(hb.clone())),
                        )
                        .into(),
                    ),
                    ReplyType::Wire(
                        Message::new(
                            self.mac_address.bytes(),
                            msg.from().try_into()?,
                            &PacketType::Control(ControlType::HeartBeatReply(hb.into())),
                        )
                        .into(),
                    ),
                ]
                .into())
            }
            Ok(PacketType::Control(ControlType::HeartBeatReply(hbr))) => {
                let mut hello_state_guard = self.upstream_state.write().unwrap();
                let (ref mut map, _) = *hello_state_guard;

                let (mac, dur, _) = map.get(&hbr.id).context("no upstream")?;
                let mac = *mac;
                let dur = *dur;

                let mut hello_state_guard = self.hello_seq.write().unwrap();
                let (ref mut map, ref mut vec) = *hello_state_guard;

                let contained = map.contains_key(&(hbr.source, hbr.id));

                map.entry((hbr.source, hbr.id))
                    .or_insert_with(|| HashMap::with_capacity(1))
                    .entry(MacAddress::new(msg.from().try_into()?))
                    .or_insert_with(|| (Instant::now().duration_since(self.boot) - dur, hbr.hops));

                if !contained {
                    if vec.len() >= self.args.node_params.hello_history.try_into()? {
                        enum Result {
                            Replaced((MacAddress, u32)),
                            AddAfter((MacAddress, u32)),
                        }
                        match match vec.get_mut(0) {
                            Some(old_id) => {
                                Result::Replaced(std::mem::replace(old_id, (hbr.source, hbr.id)))
                            }
                            None => Result::AddAfter((hbr.source, hbr.id)),
                        } {
                            Result::AddAfter(id) => {
                                vec.push(id);
                            }
                            Result::Replaced(id) => {
                                map.remove(&id);
                            }
                        };
                    } else {
                        vec.push((hbr.source, hbr.id));
                    }

                    vec.sort();
                }

                Ok(vec![ReplyType::Wire(
                    Message::new(
                        msg.from().try_into()?,
                        mac.bytes(),
                        &PacketType::Control(ControlType::HeartBeatReply(hbr)),
                    )
                    .into(),
                )]
                .into())
            }
            Err(e) => {
                tracing::error!(?e, "error getting message layer");
                bail!(e)
            }
        }
    }
}
