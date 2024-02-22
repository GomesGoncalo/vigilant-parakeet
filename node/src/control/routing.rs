use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
    time::Instant,
};

use anyhow::{bail, Result};
use mac_address::MacAddress;

use crate::{
    dev::Device,
    messages::{ControlType, HeartBeat, Message, PacketType},
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
            Ok(PacketType::Control(ControlType::HeartBeat(hb))) => {
                let upstream = self.upstream.write().unwrap();
                Some(
                    vec![
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
                    .into(),
                )
            }
            Ok(PacketType::Control(ControlType::HeartBeatReply(_hb))) => todo!(),
            Err(e) => bail!(e),
        })
    }

    fn generate(&self, _dev: Arc<Device>) {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use mac_address::MacAddress;

    use crate::{
        control::{
            args::{NodeParameters, NodeType},
            node::{Node, ReplyType},
            Args,
        },
        messages::{ControlType, HeartBeat, Message, PacketType},
    };

    use super::Routing;

    #[test]
    fn routing_forwards_data_message_to_tap() {
        let args = Args {
            bind: String::default(),
            tap_name: None,
            ip: None,
            node_params: NodeParameters {
                node_type: NodeType::Obu,
                hello_history: 1,
                hello_periodicity: None,
            },
        };

        let routing = Routing::new(args, MacAddress::default());
        let message = Message::new([0; 6], [0; 6], &PacketType::Data(&vec![1u8, 2u8, 3u8]));
        let forward = routing.handle_msg(&message);

        let ReplyType::Tap(ref inner) = forward.unwrap().unwrap()[0] else {
            panic!("not the right kind of message");
        };

        let expect = vec![[1u8, 2u8, 3u8].into()];
        assert_eq!(inner, &expect);
    }

    #[test]
    fn routing_when_receive_heartbeat_sets_upstream_route_retransmits_hello_with_one_more_hop_and_replies(
    ) {
        let args = Args {
            bind: String::default(),
            tap_name: None,
            ip: None,
            node_params: NodeParameters {
                node_type: NodeType::Obu,
                hello_history: 1,
                hello_periodicity: None,
            },
        };

        let routing = Routing::new(args, [9; 6].into());
        let heartbeat = HeartBeat::new([1; 6].into(), Duration::from_millis(4), 0);
        let message = Message::new(
            [1; 6],
            [255; 6],
            &PacketType::Control(ControlType::HeartBeat(heartbeat)),
        );

        let forward = routing.handle_msg(&message);

        let messages = forward.unwrap().unwrap();
        assert_eq!(messages.len(), 2);

        let ReplyType::Wire(ref first_message) = messages[0] else {
            panic!("not the right message");
        };
        let ReplyType::Wire(ref second_message) = messages[1] else {
            panic!("not the right message");
        };

        let first_message: Message = first_message.clone().try_into().unwrap();
        let second_message: Message = second_message.clone().try_into().unwrap();

        match (
            (first_message.next_layer(), &first_message),
            (second_message.next_layer(), &second_message),
        ) {
            (
                (Ok(PacketType::Control(ControlType::HeartBeat(hb))), fm),
                (Ok(PacketType::Control(ControlType::HeartBeatReply(hbr))), sm),
            )
            | (
                (Ok(PacketType::Control(ControlType::HeartBeatReply(hbr))), sm),
                (Ok(PacketType::Control(ControlType::HeartBeat(hb))), fm),
            ) => {
                assert_eq!(hb.hops, 2);
                assert_eq!(hbr.hops, 1);
                assert_eq!(hb.source, [1; 6].into());
                assert_eq!(hbr.source, [1; 6].into());
                assert_eq!(hb.now, Duration::from_millis(4));
                assert_eq!(hbr.now, Duration::from_millis(4));
                assert_eq!(fm.from(), [9; 6]);
                assert_eq!(sm.from(), [9; 6]);
                assert_eq!(fm.to(), [255; 6]);
                assert_eq!(sm.to(), [1; 6]);
            }
            _ => panic!("not correct response"),
        }
    }
}
