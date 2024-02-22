use super::{
    node::{Node, ReplyType},
    routing::Routing,
    Args,
};
use crate::{
    dev::{Device, OutgoingMessage},
    messages::{ControlType, HeartBeat, Message, PacketType},
};
use anyhow::{bail, Result};
use mac_address::MacAddress;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc, RwLock,
    },
    time::{Duration, Instant},
};
use tracing::Instrument;

pub struct Rsu {
    args: Args,
    boot: Instant,
    mac_address: MacAddress,
    hb_seq: Arc<AtomicU32>,
    hello_seq: Arc<RwLock<(HashMap<u32, HashMap<MacAddress, (Duration, u32)>>, Vec<u32>)>>,
    routing: Routing,
}

impl Rsu {
    pub fn new(args: Args, mac_address: MacAddress) -> Self {
        let rsu = Self {
            args: args.clone(),
            boot: Instant::now(),
            mac_address: mac_address.clone(),
            hb_seq: AtomicU32::new(0).into(),
            hello_seq: Arc::new(RwLock::new((HashMap::new(), Vec::new()))),
            routing: Routing::new(args, mac_address),
        };

        tracing::info!(?rsu.args, %rsu.mac_address, "Setup Rsu");
        rsu
    }
}

impl Node for Rsu {
    fn generate(&self, dev: Arc<Device>) {
        let boot = self.boot;
        let mac_address = self.mac_address;
        let counter = self.hb_seq.clone();
        let hello_periodicity = self.args.node_params.hello_periodicity;

        if let Some(hello_periodicity) = hello_periodicity {
            let span = tracing::debug_span!(target: "hello", "hello task", %mac_address);
            tokio::spawn(
                async move {
                    loop {
                        let message = HeartBeat::new(
                            &mac_address,
                            Instant::now().duration_since(boot),
                            counter.fetch_add(1, Ordering::AcqRel),
                        );

                        tracing::debug!(?message, "sending hello");
                        let message = Message::new(
                            mac_address.bytes(),
                            [255; 6],
                            &PacketType::Control(ControlType::HeartBeat(message)),
                        );

                        let _ = dev.tx.send(OutgoingMessage::Vectored(message.into())).await;
                        tokio::time::sleep(Duration::from_millis(hello_periodicity.into())).await;
                    }
                }
                .instrument(span),
            );
        } else {
            tracing::error!(%mac_address, ?self.args, "Rsu configured without hello_periodicity parameter");
        }
    }

    fn handle_msg(&self, msg: &Message) -> Result<Option<Vec<ReplyType>>> {
        match msg.next_layer() {
            Ok(PacketType::Data(_)) => self.routing.handle_msg(msg),
            Ok(PacketType::Control(ControlType::HeartBeat(_))) => Ok(None),
            Ok(PacketType::Control(ControlType::HeartBeatReply(hbr))) => {
                let span = tracing::debug_span!(target: "hello", "hello task", %self.mac_address);
                let _g = span.enter();
                let mut hello_state_guard = self.hello_seq.write().unwrap();
                let (ref mut map, ref mut vec) = *hello_state_guard;

                let contained = map.contains_key(&hbr.id);

                map.entry(hbr.id)
                    .or_insert_with(|| HashMap::with_capacity(1))
                    .entry(MacAddress::new(msg.from().try_into()?))
                    .or_insert_with(|| {
                        (Instant::now().duration_since(self.boot) - hbr.now, hbr.hops)
                    });

                if !contained {
                    if vec.len() >= self.args.node_params.hello_history.try_into()? {
                        enum Result {
                            Replaced(u32),
                            AddAfter(u32),
                        }
                        match match vec.get_mut(0) {
                            Some(old_id) => {
                                let repl = old_id.clone();
                                *old_id = hbr.id;
                                Result::Replaced(repl)
                            }
                            None => Result::AddAfter(hbr.id),
                        } {
                            Result::AddAfter(id) => {
                                vec.push(id);
                            }
                            Result::Replaced(id) => {
                                map.remove(&id);
                            }
                        };
                    } else {
                        vec.push(hbr.id);
                    }

                    vec.sort();
                }
                Ok(None)
            }
            Err(e) => {
                tracing::error!(?e, "error getting message layer");
                bail!(e)
            }
        }
    }
}
