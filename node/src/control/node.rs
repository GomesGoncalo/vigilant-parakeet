use std::{
    sync::{atomic::AtomicUsize, Arc},
    time::{Duration, Instant},
};

use super::args::Args;
use crate::{
    dev::{Device, OutgoingMessage},
    messages::{ControlType, HeartBeat, HeartBeatReply, Message, PacketType},
};
use anyhow::Result;
use mac_address::MacAddress;

pub struct Node {
    args: Args,
    boot: Instant,
    mac_address: MacAddress,
    hb_seq: AtomicUsize,
}

pub enum ReplyType {
    Wire(Vec<Arc<[u8]>>),
    Tap(Vec<Arc<[u8]>>),
}

impl Node {
    pub fn new(args: Args, mac_address: &MacAddress) -> Self {
        Self {
            args,
            boot: Instant::now(),
            mac_address: mac_address.clone(),
            hb_seq: AtomicUsize::new(0),
        }
    }

    pub fn handle_msg(&self, msg: &Message) -> Result<Option<ReplyType>> {
        Ok(match msg.next_layer() {
            Ok(PacketType::Data(buf)) => Some(ReplyType::Tap(vec![buf.into()])),
            Ok(PacketType::Control(ControlType::HeartBeat(hb))) => {
                tracing::info!(?hb, "received hb");
                Some(ReplyType::Wire(
                    Message::new(
                        self.mac_address.bytes(),
                        msg.from().try_into()?,
                        &PacketType::Control(ControlType::HeartBeatReply(HeartBeatReply::new(
                            Instant::now().duration_since(self.boot),
                        ))),
                    )
                    .into(),
                ))
            }
            Ok(PacketType::Control(ControlType::HeartBeatReply(hbr))) => {
                tracing::info!(?hbr, "received heartbeat reply");
                None
            }
            Err(_) => {
                todo!()
            }
        })
    }

    pub fn generate(&self, dev: Arc<Device>) {
        let boot = self.boot;
        let mac_address = self.mac_address.bytes();
        tokio::spawn(async move {
            loop {
                let message = HeartBeat::new(Instant::now().duration_since(boot));
                let message = Message::new(
                    mac_address,
                    [255; 6],
                    &PacketType::Control(ControlType::HeartBeat(message)),
                );
                let _ = dev.tx.send(OutgoingMessage::Vectored(message.into())).await;

                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        });
    }
}
