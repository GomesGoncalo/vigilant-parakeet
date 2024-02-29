mod routing;

use super::node::{Node, ReplyType};
use crate::{
    dev::{Device, OutgoingMessage},
    messages::{ControlType, Data, Message, PacketType},
    Args,
};
use anyhow::{bail, Result};
use itertools::Itertools;
use mac_address::MacAddress;
use routing::Routing;
use std::{
    sync::{Arc, RwLock},
    time::Duration,
};
use tracing::Instrument;

pub struct Rsu {
    args: Args,
    mac: MacAddress,
    routing: Arc<RwLock<Routing>>,
}

impl Rsu {
    pub fn new(args: Args, mac: MacAddress) -> Result<Self> {
        let rsu = Self {
            routing: Arc::new(RwLock::new(Routing::new(&args)?)),
            args,
            mac,
        };

        tracing::info!(?rsu.args, %rsu.mac, "Setup Rsu");
        Ok(rsu)
    }
}

impl Node for Rsu {
    fn handle_msg(&self, msg: Message) -> Result<Option<Vec<ReplyType>>> {
        match msg.next_layer() {
            Ok(PacketType::Data(buf)) => {
                let mut messages = vec![ReplyType::Tap(vec![buf.data.into()])];
                if let Ok(Some(more)) = self.tap_traffic(buf) {
                    messages.extend(more);
                };

                Ok(Some(messages))
            }
            Ok(PacketType::Control(ControlType::HeartBeat(_))) => Ok(None),
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
                        let message = routing.write().unwrap().send_heartbeat(mac);
                        tracing::trace!(target: "pkt", ?message, "pkt");
                        match dev.tx.send(OutgoingMessage::Vectored(message.into())).await {
                            Ok(()) => tracing::trace!("sent hello"),
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

    fn tap_traffic(&self, msg: Arc<Data>) -> Result<Option<Vec<ReplyType>>> {
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
                            &PacketType::Data(msg.clone()),
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
