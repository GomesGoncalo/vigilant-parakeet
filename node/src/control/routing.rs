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
            Ok(PacketType::Control(ControlType::HeartBeat(_hb))) => {
                let upstream = self.upstream.write().unwrap();
                todo!()
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

    // #[test]
    // fn routing_when_receive_heartbeat_sets_upstream_route() {
    //     let args = Args {
    //         bind: String::default(),
    //         tap_name: None,
    //         ip: None,
    //         node_params: NodeParameters {
    //             node_type: NodeType::Obu,
    //             hello_history: 1,
    //             hello_periodicity: None,
    //         },
    //     };
    //
    //     let routing = Routing::new(args, MacAddress::default());
    //     let heartbeat = HeartBeat::new([1; 6].into(), Duration::from_millis(1), 0);
    //     let message = Message::new(
    //         [0; 6],
    //         [0; 6],
    //         &PacketType::Control(ControlType::HeartBeat(heartbeat)),
    //     );
    //
    //     let forward = routing.handle_msg(&message);
    //
    //     let ReplyType::Tap(ref inner) = forward.unwrap().unwrap()[0] else {
    //         panic!("not the right kind of message");
    //     };
    //
    //     let expect = vec![[1u8, 2u8, 3u8].into()];
    //     assert_eq!(inner, &expect);
    // }
}
