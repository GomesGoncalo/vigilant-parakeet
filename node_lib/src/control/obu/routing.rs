use crate::control::node::ReplyType;
use crate::{
    control::route::Route,
    messages::{
        control::{heartbeat::HeartbeatReply, Control},
        message::Message,
        packet_type::PacketType,
    },
    Args,
};
use anyhow::{bail, Result};
use arc_swap::ArcSwapOption;
use indexmap::IndexMap;
use mac_address::MacAddress;
use std::collections::{hash_map::Entry, HashMap};
use tokio::time::{Duration, Instant};
use tracing::Level;

#[derive(Debug)]
struct Target {
    hops: u32,
    mac: MacAddress,
    latency: Option<Duration>,
}

#[cfg(test)]
mod tests {
    use super::Routing;
    use crate::{
        args::{NodeParameters, NodeType},
        messages::{
            control::heartbeat::Heartbeat, control::heartbeat::HeartbeatReply, control::Control,
            message::Message, packet_type::PacketType,
        },
        Args,
    };
    // ReplyType is not used in these test helpers; remove unused import.
    use mac_address::MacAddress;
    use tokio::time::{Duration, Instant};

    #[test]
    fn handle_heartbeat_creates_route_and_returns_replies() {
        let args = Args {
            bind: String::default(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            node_params: NodeParameters {
                node_type: NodeType::Obu,
                hello_history: 2,
                hello_periodicity: None,
            },
        };

        let boot = Instant::now();
        let mut routing = Routing::new(&args, &boot).expect("routing built");

        let hb_source: MacAddress = [2u8; 6].into();
        let pkt_from: MacAddress = [3u8; 6].into();
        let our_mac: MacAddress = [9u8; 6].into();

        let hb = Heartbeat::new(Duration::from_millis(1), 1u32, hb_source);
        let msg = Message::new(
            pkt_from,
            [255u8; 6].into(),
            PacketType::Control(Control::Heartbeat(hb.clone())),
        );

        let res = routing.handle_heartbeat(&msg, our_mac).expect("handled");
        assert!(res.is_some());
        let vec = res.unwrap();
        // Should produce two wire replies (heartbeat forward and reply)
        assert!(vec.len() >= 2);

        // Now we should be able to get a route to hb_source
        let route = routing.get_route_to(Some(hb_source));
        assert!(route.is_some());
    }

    #[test]
    fn handle_heartbeat_reply_updates_downstream_and_replies() {
        let args = Args {
            bind: String::default(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            node_params: NodeParameters {
                node_type: NodeType::Obu,
                hello_history: 2,
                hello_periodicity: None,
            },
        };

        let boot = Instant::now();
        let mut routing = Routing::new(&args, &boot).expect("routing built");

        let hb_source: MacAddress = [20u8; 6].into();
        let pkt_from: MacAddress = [30u8; 6].into();
        let our_mac: MacAddress = [99u8; 6].into();

        // Insert initial heartbeat to create state
        let hb = Heartbeat::new(Duration::from_millis(1), 7u32, hb_source);
        let initial = Message::new(
            pkt_from,
            [255u8; 6].into(),
            PacketType::Control(Control::Heartbeat(hb.clone())),
        );
        let _ = routing
            .handle_heartbeat(&initial, our_mac)
            .expect("handled");

        // Create a HeartbeatReply from some sender (not equal to next_upstream)
        let reply_sender: MacAddress = [42u8; 6].into();
        let hbr = HeartbeatReply::from_sender(&hb, reply_sender);
        let reply_from: MacAddress = [55u8; 6].into();
        let reply_msg = Message::new(
            reply_from,
            [255u8; 6].into(),
            PacketType::Control(Control::HeartbeatReply(hbr.clone())),
        );

        let res = routing
            .handle_heartbeat_reply(&reply_msg, our_mac)
            .expect("handled reply");
        // Should return an Ok(Some(_)) with a wire reply
        assert!(res.is_some());
        let out = res.unwrap();
        assert!(!out.is_empty());
    }
}

#[cfg(test)]
mod cache_tests {
    use super::Routing;
    use crate::{
        args::{NodeParameters, NodeType},
        Args,
    };
    use mac_address::MacAddress;
    use tokio::time::{Duration, Instant};

    #[test]
    fn select_and_cache_upstream_sets_cache() {
        let args = Args {
            bind: String::default(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            node_params: NodeParameters {
                node_type: NodeType::Obu,
                hello_history: 2,
                hello_periodicity: None,
            },
        };

        let boot = Instant::now() - Duration::from_secs(1);
        let mut routing = Routing::new(&args, &boot).expect("routing built");

        // Create a heartbeat to populate routes
        let hb_source: MacAddress = [7u8; 6].into();
        let pkt_from: MacAddress = [8u8; 6].into();
        let our_mac: MacAddress = [9u8; 6].into();
        let hb = crate::messages::control::heartbeat::Heartbeat::new(
            std::time::Duration::from_millis(1),
            1u32,
            hb_source,
        );
        let hb_msg = crate::messages::message::Message::new(
            pkt_from,
            [255u8; 6].into(),
            crate::messages::packet_type::PacketType::Control(
                crate::messages::control::Control::Heartbeat(hb.clone()),
            ),
        );
        // Insert heartbeat via routing handle
        let _ = routing
            .handle_heartbeat(&hb_msg, our_mac)
            .expect("handled hb");

        // Now select and cache the upstream for hb_source
        let selected = routing.select_and_cache_upstream(hb_source);
        assert!(selected.is_some());

        // get_route_to(None) should now return the cached upstream route
        let cached = routing.get_route_to(None);
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().mac, selected.unwrap().mac);
    }
}

#[cfg(test)]
mod regression_tests {
    use super::Routing;
    use crate::messages::control::heartbeat::{Heartbeat, HeartbeatReply};
    use crate::messages::{control::Control, message::Message, packet_type::PacketType};
    use crate::{
        args::{NodeParameters, NodeType},
        Args,
    };
    use mac_address::MacAddress;
    use tokio::time::Instant;

    // Regression test for the case where a HeartbeatReply arrives from the
    // recorded next hop (pkt.from() == next_upstream). Previously the code
    // treated that as a loop and bailed; that's incorrect. We should only
    // bail if the recorded next_upstream equals the HeartbeatReply's
    // reported sender (message.sender()). This test asserts we do not bail
    // when pkt.from() == next_upstream but message.sender() != next_upstream.
    #[test]
    fn heartbeat_reply_from_next_hop_does_not_bail() {
        let args = Args {
            bind: String::default(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            node_params: NodeParameters {
                node_type: NodeType::Obu,
                hello_history: 2,
                hello_periodicity: None,
            },
        };

        let boot = Instant::now();
        let mut routing = Routing::new(&args, &boot).expect("routing built");

        // Heartbeat originates from A, observed via B (pkt.from)
        let hb_source: MacAddress = [1u8; 6].into(); // A
        let pkt_from: MacAddress = [2u8; 6].into(); // B (next hop)
        let our_mac: MacAddress = [9u8; 6].into();

        let hb = Heartbeat::new(std::time::Duration::from_millis(1), 1u32, hb_source);
        let hb_msg = Message::new(
            pkt_from,
            [255u8; 6].into(),
            PacketType::Control(Control::Heartbeat(hb.clone())),
        );

        // Insert heartbeat to establish next_upstream for hb_source = pkt_from
        let _ = routing
            .handle_heartbeat(&hb_msg, our_mac)
            .expect("handled hb");

        // Now construct a HeartbeatReply where the HeartbeatReply::sender() is A
        // but the packet is from B (pkt.from == next_upstream). This should be
        // accepted and not cause bail.
        let reply_sender: MacAddress = hb_source; // A
        let hbr = HeartbeatReply::from_sender(&hb, reply_sender);
        let reply_from: MacAddress = pkt_from; // B
        let reply_msg = Message::new(
            reply_from,
            [255u8; 6].into(),
            PacketType::Control(Control::HeartbeatReply(hbr.clone())),
        );

        // When the reply arrives from the next hop (pkt.from == next_upstream)
        // we should NOT forward it back (to avoid an immediate bounce). The
        // function returns Ok(None) in this case.
        let res = routing
            .handle_heartbeat_reply(&reply_msg, our_mac)
            .expect("handled reply");
        assert!(res.is_none());
    }

    #[test]
    fn heartbeat_reply_from_sender_triggers_bail() {
        let args = Args {
            bind: String::default(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            node_params: NodeParameters {
                node_type: NodeType::Obu,
                hello_history: 2,
                hello_periodicity: None,
            },
        };

        let boot = Instant::now();
        let mut routing = Routing::new(&args, &boot).expect("routing built");

        // Heartbeat originates from A, observed via B (pkt.from)
        let hb_source: MacAddress = [10u8; 6].into(); // A
        let pkt_from: MacAddress = [20u8; 6].into(); // B (next hop)
        let our_mac: MacAddress = [9u8; 6].into();

        let hb = Heartbeat::new(std::time::Duration::from_millis(1), 2u32, hb_source);
        let hb_msg = Message::new(
            pkt_from,
            [255u8; 6].into(),
            PacketType::Control(Control::Heartbeat(hb.clone())),
        );

        // Insert heartbeat to establish next_upstream for hb_source = pkt_from
        let _ = routing
            .handle_heartbeat(&hb_msg, our_mac)
            .expect("handled hb");

        // Now construct a HeartbeatReply where the HeartbeatReply::sender() is
        // equal to our recorded next_upstream (i.e., message.sender == next_upstream)
        let reply_sender: MacAddress = pkt_from; // B == next_upstream
        let hbr = HeartbeatReply::from_sender(&hb, reply_sender);
        let reply_from: MacAddress = [30u8; 6].into(); // some other node forwarded it
        let reply_msg = Message::new(
            reply_from,
            [255u8; 6].into(),
            PacketType::Control(Control::HeartbeatReply(hbr.clone())),
        );

        // This should bail with an error indicating a loop was detected.
        let res = routing.handle_heartbeat_reply(&reply_msg, our_mac);
        assert!(res.is_err());
        let err = res.unwrap_err();
        assert!(format!("{}", err).contains("loop detected"));
    }
}

#[cfg(test)]
mod more_tests {
    use super::Routing;
    use crate::args::{NodeParameters, NodeType};
    use crate::Args;
    use mac_address::MacAddress;
    use tokio::time::{Duration, Instant};

    #[test]
    fn get_route_to_none_when_empty() {
        let args = Args {
            bind: String::default(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            node_params: NodeParameters {
                node_type: NodeType::Obu,
                hello_history: 2,
                hello_periodicity: None,
            },
        };

        let boot = Instant::now();
        let routing = Routing::new(&args, &boot).expect("routing built");

        let unknown: MacAddress = [1u8; 6].into();
        // No routes yet
        assert!(routing.get_route_to(Some(unknown)).is_none());
        // No cached upstream
        assert!(routing.get_route_to(None).is_none());
    }

    #[test]
    fn duplicate_heartbeat_returns_none() {
        let args = Args {
            bind: String::default(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            node_params: NodeParameters {
                node_type: NodeType::Obu,
                hello_history: 4,
                hello_periodicity: None,
            },
        };

        let boot = Instant::now();
        let mut routing = Routing::new(&args, &boot).expect("routing built");

        let hb_source: MacAddress = [5u8; 6].into();
        let pkt_from: MacAddress = [6u8; 6].into();
        let our_mac: MacAddress = [7u8; 6].into();
        let hb = crate::messages::control::heartbeat::Heartbeat::new(
            std::time::Duration::from_millis(1),
            123u32,
            hb_source,
        );
        let hb_msg = crate::messages::message::Message::new(
            pkt_from,
            [255u8; 6].into(),
            crate::messages::packet_type::PacketType::Control(
                crate::messages::control::Control::Heartbeat(hb.clone()),
            ),
        );

        let first = routing.handle_heartbeat(&hb_msg, our_mac).expect("hb1");
        assert!(first.is_some());
        let second = routing.handle_heartbeat(&hb_msg, our_mac).expect("hb2");
        assert!(second.is_none(), "duplicate id should be ignored");
    }

    #[test]
    fn hello_history_eviction_keeps_latest() {
        let args = Args {
            bind: String::default(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            node_params: NodeParameters {
                node_type: NodeType::Obu,
                hello_history: 1, // force capacity 1
                hello_periodicity: None,
            },
        };

        let boot = Instant::now();
        let mut routing = Routing::new(&args, &boot).expect("routing built");

        let hb_source: MacAddress = [1u8, 1, 1, 1, 1, 1].into();
        let pkt_from: MacAddress = [2u8, 2, 2, 2, 2, 2].into();
        let our_mac: MacAddress = [9u8; 6].into();

        let hb1 = crate::messages::control::heartbeat::Heartbeat::new(
            std::time::Duration::from_millis(1),
            1u32,
            hb_source,
        );
        let msg1 = crate::messages::message::Message::new(
            pkt_from,
            [255u8; 6].into(),
            crate::messages::packet_type::PacketType::Control(
                crate::messages::control::Control::Heartbeat(hb1.clone()),
            ),
        );
        let _ = routing.handle_heartbeat(&msg1, our_mac).unwrap();

        let hb2 = crate::messages::control::heartbeat::Heartbeat::new(
            std::time::Duration::from_millis(2),
            2u32,
            hb_source,
        );
        let msg2 = crate::messages::message::Message::new(
            pkt_from,
            [255u8; 6].into(),
            crate::messages::packet_type::PacketType::Control(
                crate::messages::control::Control::Heartbeat(hb2.clone()),
            ),
        );
        let _ = routing.handle_heartbeat(&msg2, our_mac).unwrap();

        // Access internal map to assert capacity behavior
        let entry = routing.routes.get(&hb_source).expect("has entry");
        assert_eq!(entry.len(), 1, "should keep only one id due to capacity");
        let (&only_id, _) = entry.last().expect("one element exists");
        assert_eq!(only_id, 2u32);
    }

    #[test]
    fn out_of_order_id_clears_prior_entries() {
        let args = Args {
            bind: String::default(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            node_params: NodeParameters {
                node_type: NodeType::Obu,
                hello_history: 4,
                hello_periodicity: None,
            },
        };

        let boot = Instant::now();
        let mut routing = Routing::new(&args, &boot).expect("routing built");

        let hb_source: MacAddress = [9u8, 8, 7, 6, 5, 4].into();
        let pkt_from: MacAddress = [1u8, 2, 3, 4, 5, 6].into();
        let our_mac: MacAddress = [0u8; 6].into();

        // Insert id 10 first
        let hb10 = crate::messages::control::heartbeat::Heartbeat::new(
            std::time::Duration::from_millis(10),
            10u32,
            hb_source,
        );
        let msg10 = crate::messages::message::Message::new(
            pkt_from,
            [255u8; 6].into(),
            crate::messages::packet_type::PacketType::Control(
                crate::messages::control::Control::Heartbeat(hb10.clone()),
            ),
        );
        let _ = routing.handle_heartbeat(&msg10, our_mac).unwrap();

        // Now insert smaller id 5, which should clear existing entries
        let hb5 = crate::messages::control::heartbeat::Heartbeat::new(
            std::time::Duration::from_millis(5),
            5u32,
            hb_source,
        );
        let msg5 = crate::messages::message::Message::new(
            pkt_from,
            [255u8; 6].into(),
            crate::messages::packet_type::PacketType::Control(
                crate::messages::control::Control::Heartbeat(hb5.clone()),
            ),
        );
        let _ = routing.handle_heartbeat(&msg5, our_mac).unwrap();

        let entry = routing.routes.get(&hb_source).expect("entry exists");
        assert_eq!(entry.len(), 1);
        let (&only_id, _) = entry.first().expect("one elem");
        assert_eq!(only_id, 5u32);
    }

    #[test]
    fn select_and_cache_upstream_none_when_no_routes() {
        let args = Args {
            bind: String::default(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            node_params: NodeParameters {
                node_type: NodeType::Obu,
                hello_history: 2,
                hello_periodicity: None,
            },
        };

        let boot = Instant::now();
        let routing = Routing::new(&args, &boot).expect("routing built");
        let mac: MacAddress = [1u8; 6].into();
        assert!(routing.select_and_cache_upstream(mac).is_none());
        assert!(routing.get_route_to(None).is_none());
    }

    #[test]
    fn clear_cached_upstream_removes_cache() {
        let args = Args {
            bind: String::default(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            node_params: NodeParameters {
                node_type: NodeType::Obu,
                hello_history: 2,
                hello_periodicity: None,
            },
        };

        let boot = Instant::now();
        let mut routing = Routing::new(&args, &boot).expect("routing built");
        let hb_source: MacAddress = [7u8; 6].into();
        let pkt_from: MacAddress = [8u8; 6].into();
        let our_mac: MacAddress = [9u8; 6].into();

        let hb = crate::messages::control::heartbeat::Heartbeat::new(
            std::time::Duration::from_millis(1),
            1u32,
            hb_source,
        );
        let hb_msg = crate::messages::message::Message::new(
            pkt_from,
            [255u8; 6].into(),
            crate::messages::packet_type::PacketType::Control(
                crate::messages::control::Control::Heartbeat(hb.clone()),
            ),
        );
        let _ = routing.handle_heartbeat(&hb_msg, our_mac).unwrap();
        assert!(routing.select_and_cache_upstream(hb_source).is_some());
        assert!(routing.get_cached_upstream().is_some());
        routing.clear_cached_upstream();
        assert!(routing.get_cached_upstream().is_none());
    }

    #[test]
    fn hysteresis_keeps_cached_when_hops_equal() {
        let args = Args {
            bind: String::default(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            node_params: NodeParameters {
                node_type: NodeType::Obu,
                hello_history: 8,
                hello_periodicity: None,
            },
        };

        let boot = Instant::now();
        let mut routing = Routing::new(&args, &boot).expect("routing built");

        let rsu: MacAddress = [1u8; 6].into();
        let via_b: MacAddress = [2u8; 6].into();
        let via_c: MacAddress = [3u8; 6].into();
        let our_mac: MacAddress = [9u8; 6].into();

        // Heartbeat from RSU via B with 2 hops
        let hb1 = crate::messages::control::heartbeat::Heartbeat::new(
            std::time::Duration::from_millis(1),
            1u32,
            rsu,
        );
        let msg1 = crate::messages::message::Message::new(
            via_b,
            [255u8; 6].into(),
            crate::messages::packet_type::PacketType::Control(
                crate::messages::control::Control::Heartbeat(hb1.clone()),
            ),
        );
        // Insert, then cache selection chooses B
        let _ = routing.handle_heartbeat(&msg1, our_mac).unwrap();
        let sel1 = routing.select_and_cache_upstream(rsu).expect("selected");
        assert_eq!(sel1.mac, via_b);

        // Another Heartbeat from RSU via C with same hops (2)
        let hb2 = crate::messages::control::heartbeat::Heartbeat::new(
            std::time::Duration::from_millis(2),
            2u32,
            rsu,
        );
        let msg2 = crate::messages::message::Message::new(
            via_c,
            [255u8; 6].into(),
            crate::messages::packet_type::PacketType::Control(
                crate::messages::control::Control::Heartbeat(hb2.clone()),
            ),
        );
        let _ = routing.handle_heartbeat(&msg2, our_mac).unwrap();

        // get_route_to(Some) should prefer keeping cached (B) since hops are equal
        let route = routing.get_route_to(Some(rsu)).expect("route");
        assert_eq!(route.mac, via_b, "should keep cached when hops equal");
    }

    #[test]
    fn hysteresis_switches_when_one_fewer_hop() {
        let args = Args {
            bind: String::default(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            node_params: NodeParameters {
                node_type: NodeType::Obu,
                hello_history: 8,
                hello_periodicity: None,
            },
        };

        let boot = Instant::now();
        let mut routing = Routing::new(&args, &boot).expect("routing built");

        let rsu: MacAddress = [10u8; 6].into();
        let via_b: MacAddress = [20u8; 6].into();
        let via_c: MacAddress = [30u8; 6].into();
        let our_mac: MacAddress = [99u8; 6].into();

        // First candidate with 2 hops via B (craft borrowed heartbeat bytes)
        let mut hb1_bytes = Vec::new();
        hb1_bytes.extend_from_slice(&0u128.to_be_bytes()); // duration 16B
        hb1_bytes.extend_from_slice(&1u32.to_be_bytes()); // id
        hb1_bytes.extend_from_slice(&2u32.to_be_bytes()); // hops = 2
        hb1_bytes.extend_from_slice(&rsu.bytes()); // source
        let hb1 = crate::messages::control::heartbeat::Heartbeat::try_from(&hb1_bytes[..])
            .expect("hb1 bytes to heartbeat");
        let msg1 = crate::messages::message::Message::new(
            via_b,
            [255u8; 6].into(),
            crate::messages::packet_type::PacketType::Control(
                crate::messages::control::Control::Heartbeat(hb1.clone()),
            ),
        );
        let _ = routing.handle_heartbeat(&msg1, our_mac).unwrap();
        let sel1 = routing.select_and_cache_upstream(rsu).expect("selected");
        assert_eq!(sel1.mac, via_b);

        // Better candidate with 1 hop via C (craft borrowed heartbeat bytes)
        let mut hb2_bytes = Vec::new();
        hb2_bytes.extend_from_slice(&0u128.to_be_bytes()); // duration 16B
        hb2_bytes.extend_from_slice(&2u32.to_be_bytes()); // id
        hb2_bytes.extend_from_slice(&1u32.to_be_bytes()); // hops = 1
        hb2_bytes.extend_from_slice(&rsu.bytes()); // source
        let hb2 = crate::messages::control::heartbeat::Heartbeat::try_from(&hb2_bytes[..])
            .expect("hb2 bytes to heartbeat");
        let msg2 = crate::messages::message::Message::new(
            via_c,
            [255u8; 6].into(),
            crate::messages::packet_type::PacketType::Control(
                crate::messages::control::Control::Heartbeat(hb2.clone()),
            ),
        );
        let _ = routing.handle_heartbeat(&msg2, our_mac).unwrap();

        // Now get_route_to(Some) should switch to C due to one fewer hop
        let route = routing.get_route_to(Some(rsu)).expect("route");
        assert_eq!(route.mac, via_c, "should switch when one fewer hop");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn hysteresis_latency_improvement_below_10_percent_keeps_cached() {
        let args = Args {
            bind: String::default(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            node_params: NodeParameters {
                node_type: NodeType::Obu,
                hello_history: 8,
                hello_periodicity: None,
            },
        };
        // Use paused time so we can deterministically advance time
        tokio::time::pause();
        let boot = Instant::now();
        let mut routing = Routing::new(&args, &boot).expect("routing built");

        let rsu: MacAddress = [11u8; 6].into();
        let via_b: MacAddress = [21u8; 6].into();
        let via_c: MacAddress = [31u8; 6].into();
        let our_mac: MacAddress = [101u8; 6].into();

        // HB via B (id 1), then cache B
        let mut hb1_bytes = Vec::new();
        hb1_bytes.extend_from_slice(&0u128.to_be_bytes());
        hb1_bytes.extend_from_slice(&1u32.to_be_bytes());
        hb1_bytes.extend_from_slice(&2u32.to_be_bytes()); // higher hops for cached
        hb1_bytes.extend_from_slice(&rsu.bytes());
        let hb1 =
            crate::messages::control::heartbeat::Heartbeat::try_from(&hb1_bytes[..]).expect("hb1");
        let msg1 = crate::messages::message::Message::new(
            via_b,
            [255u8; 6].into(),
            crate::messages::packet_type::PacketType::Control(
                crate::messages::control::Control::Heartbeat(hb1.clone()),
            ),
        );
        let _ = routing.handle_heartbeat(&msg1, our_mac).unwrap();
        routing.select_and_cache_upstream(rsu).expect("cached B");

        // Advance 25ms between HB and HBR for B
        tokio::time::advance(Duration::from_millis(25)).await;
        let hbr1 = crate::messages::control::heartbeat::HeartbeatReply::from_sender(&hb1, rsu);
        let reply1 = crate::messages::message::Message::new(
            via_b,
            [255u8; 6].into(),
            crate::messages::packet_type::PacketType::Control(
                crate::messages::control::Control::HeartbeatReply(hbr1.clone()),
            ),
        );
        let _ = routing
            .handle_heartbeat_reply(&reply1, our_mac)
            .unwrap_or(None);

        // HB via C (id 2) with the SAME number of hops as B to test latency-only hysteresis
        let mut hb2_bytes = Vec::new();
        hb2_bytes.extend_from_slice(&0u128.to_be_bytes());
        hb2_bytes.extend_from_slice(&2u32.to_be_bytes());
        hb2_bytes.extend_from_slice(&2u32.to_be_bytes()); // same hops as cached
        hb2_bytes.extend_from_slice(&rsu.bytes());
        let hb2 =
            crate::messages::control::heartbeat::Heartbeat::try_from(&hb2_bytes[..]).expect("hb2");
        let msg2 = crate::messages::message::Message::new(
            via_c,
            [255u8; 6].into(),
            crate::messages::packet_type::PacketType::Control(
                crate::messages::control::Control::Heartbeat(hb2.clone()),
            ),
        );
        let _ = routing.handle_heartbeat(&msg2, our_mac).unwrap();
        // Advance 23ms for C (less than 10% better than 25ms)
        tokio::time::advance(Duration::from_millis(23)).await;
        let hbr2 = crate::messages::control::heartbeat::HeartbeatReply::from_sender(&hb2, rsu);
        let reply2 = crate::messages::message::Message::new(
            via_c,
            [255u8; 6].into(),
            crate::messages::packet_type::PacketType::Control(
                crate::messages::control::Control::HeartbeatReply(hbr2.clone()),
            ),
        );
        let _ = routing
            .handle_heartbeat_reply(&reply2, our_mac)
            .unwrap_or(None);

        // Now selection should keep cached B since improvement < 10%
        let route = routing.get_route_to(Some(rsu)).expect("route");
        assert_eq!(route.mac, via_b, "should keep cached when <10% better");
    }

    #[tokio::test(flavor = "current_thread")]
    #[ignore]
    async fn hysteresis_latency_improvement_above_10_percent_switches() {
        let args = Args {
            bind: String::default(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            node_params: NodeParameters {
                node_type: NodeType::Obu,
                hello_history: 8,
                hello_periodicity: None,
            },
        };
        // Use paused time so we can deterministically advance time
        tokio::time::pause();
        let boot = Instant::now() - Duration::from_secs(1);
        let mut routing = Routing::new(&args, &boot).expect("routing built");

        let rsu: MacAddress = [12u8; 6].into();
        let via_b: MacAddress = [22u8; 6].into();
        let via_c: MacAddress = [32u8; 6].into();
        let our_mac: MacAddress = [102u8; 6].into();

        // HB via B (id 1), then cache B
        let mut hb1_bytes = Vec::new();
        hb1_bytes.extend_from_slice(&0u128.to_be_bytes());
        hb1_bytes.extend_from_slice(&1u32.to_be_bytes());
        hb1_bytes.extend_from_slice(&0u32.to_be_bytes());
        hb1_bytes.extend_from_slice(&rsu.bytes());
        let hb1 =
            crate::messages::control::heartbeat::Heartbeat::try_from(&hb1_bytes[..]).expect("hb1");
        let msg1 = crate::messages::message::Message::new(
            via_b,
            [255u8; 6].into(),
            crate::messages::packet_type::PacketType::Control(
                crate::messages::control::Control::Heartbeat(hb1.clone()),
            ),
        );
        let _ = routing.handle_heartbeat(&msg1, our_mac).unwrap();
        routing.select_and_cache_upstream(rsu).expect("cached B");

        // Advance 40ms for B
        tokio::time::advance(Duration::from_millis(40)).await;
        let hbr1 = crate::messages::control::heartbeat::HeartbeatReply::from_sender(&hb1, rsu);
        let reply1 = crate::messages::message::Message::new(
            via_b,
            [255u8; 6].into(),
            crate::messages::packet_type::PacketType::Control(
                crate::messages::control::Control::HeartbeatReply(hbr1.clone()),
            ),
        );
        let _ = routing
            .handle_heartbeat_reply(&reply1, our_mac)
            .unwrap_or(None);

        // HB via C (id 2)
        let mut hb2_bytes = Vec::new();
        hb2_bytes.extend_from_slice(&0u128.to_be_bytes());
        hb2_bytes.extend_from_slice(&2u32.to_be_bytes());
        hb2_bytes.extend_from_slice(&0u32.to_be_bytes());
        hb2_bytes.extend_from_slice(&rsu.bytes());
        let hb2 =
            crate::messages::control::heartbeat::Heartbeat::try_from(&hb2_bytes[..]).expect("hb2");
        let msg2 = crate::messages::message::Message::new(
            via_c,
            [255u8; 6].into(),
            crate::messages::packet_type::PacketType::Control(
                crate::messages::control::Control::Heartbeat(hb2.clone()),
            ),
        );
        let _ = routing.handle_heartbeat(&msg2, our_mac).unwrap();

        // Advance 20ms for C (>= 10% better than 40ms)
        tokio::time::advance(Duration::from_millis(20)).await;
        let hbr2 = crate::messages::control::heartbeat::HeartbeatReply::from_sender(&hb2, rsu);
        let reply2 = crate::messages::message::Message::new(
            via_c,
            [255u8; 6].into(),
            crate::messages::packet_type::PacketType::Control(
                crate::messages::control::Control::HeartbeatReply(hbr2.clone()),
            ),
        );
        let _ = routing
            .handle_heartbeat_reply(&reply2, our_mac)
            .unwrap_or(None);

        // Trigger selection and caching now that both latencies are recorded
        let _ = routing.select_and_cache_upstream(rsu);
        // Verify both direct selection and cached reflect the better path
        let route = routing.get_route_to(Some(rsu)).expect("route");
        assert_eq!(route.mac, via_c, "should switch when >=10% better");
        let cached = routing.get_route_to(None).expect("cached route");
        assert_eq!(cached.mac, via_c, "cached should reflect the switch");
    }
}

#[derive(Debug)]
#[allow(clippy::type_complexity)]
pub struct Routing {
    args: Args,
    boot: Instant,
    routes: HashMap<
        MacAddress,
        IndexMap<
            u32,
            (
                Duration,
                MacAddress,
                u32,
                IndexMap<Duration, MacAddress>,
                HashMap<MacAddress, Vec<Target>>,
            ),
        >,
    >,
    cached_upstream: ArcSwapOption<MacAddress>,
}

impl Routing {
    pub fn new(args: &Args, boot: &Instant) -> Result<Self> {
        if args.node_params.hello_history == 0 {
            bail!("we need to be able to store at least 1 hello");
        }
        Ok(Self {
            args: args.clone(),
            boot: *boot,
            routes: HashMap::default(),
            cached_upstream: ArcSwapOption::from(None),
        })
    }

    /// Return the cached upstream MAC if present.
    pub fn get_cached_upstream(&self) -> Option<MacAddress> {
        self.cached_upstream.load().as_ref().map(|m| **m)
    }

    /// Clear the cached upstream (useful when topology changes) and increment metric.
    pub fn clear_cached_upstream(&self) {
        tracing::trace!("clearing cached_upstream");
        self.cached_upstream.store(None);
        #[cfg(feature = "stats")]
        crate::metrics::inc_cache_clear();
    }

    pub fn handle_heartbeat(
        &mut self,
        pkt: &Message,
        mac: MacAddress,
    ) -> Result<Option<Vec<ReplyType>>> {
        let PacketType::Control(Control::Heartbeat(message)) = pkt.get_packet_type() else {
            bail!("this is supposed to be a HeartBeat");
        };

        let old_route = self.get_route_to(Some(message.source()));
        let old_route_from = self.get_route_to(Some(pkt.from()?));
        let entry = self
            .routes
            .entry(message.source())
            .or_insert(IndexMap::with_capacity(usize::try_from(
                self.args.node_params.hello_history,
            )?));

        if entry.first().is_some_and(|(x, _)| x > &message.id()) {
            entry.clear();
        }

        if entry.len() == entry.capacity() && entry.capacity() > 0 {
            entry.swap_remove_index(0);
        }

        if let Some((_, _, _hops, _, _)) = entry.get(&message.id()) {
            return Ok(None);
            // So this makes us prioritize hops instead of latency
            // TODO: Is that preferable
            // if _hops < &message.hops() {
            //     return Ok(None);
            // }
        }

        let duration = Instant::now().duration_since(self.boot);
        entry.insert(
            message.id(),
            (
                duration,
                pkt.from()?,
                message.hops(),
                IndexMap::new(),
                HashMap::default(),
            ),
        );

        let entry_from = self
            .routes
            .entry(pkt.from()?)
            .or_insert(IndexMap::with_capacity(usize::try_from(
                self.args.node_params.hello_history,
            )?));

        if entry_from.first().is_some_and(|(x, _)| x > &message.id()) {
            entry_from.clear();
        }

        if entry_from.len() == entry_from.capacity() && entry_from.capacity() > 0 {
            entry_from.swap_remove_index(0);
        }

        entry_from.insert(
            message.id(),
            (
                duration,
                pkt.from()?,
                1,
                IndexMap::new(),
                HashMap::default(),
            ),
        );

        match (old_route, self.get_route_to(Some(message.source()))) {
            (None, Some(new_route)) => {
                tracing::event!(
                    Level::DEBUG,
                    from = %mac,
                    to = %message.source(),
                    through = %new_route,
                    "route created on heartbeat",
                );
                // Newly discovered route: attempt to select and cache this
                // upstream immediately so runtime components (OBU session)
                // can start using the cached upstream without waiting for
                // a HeartbeatReply cycle.
                let sel = self.select_and_cache_upstream(message.source());
                tracing::debug!(selection = ?sel.as_ref().map(|r| r.mac), "heartbeat: select_and_cache_upstream");
            }
            (_, None) => (),
            (Some(old_route), Some(new_route)) => {
                if old_route.mac != new_route.mac {
                    tracing::event!(
                        Level::DEBUG,
                        from = %mac,
                        to = %message.source(),
                        through = %new_route,
                        was_through = %old_route,
                        "route changed on heartbeat",
                    );
                    // Do not clear cached upstream; hysteresis in get_route_to will decide switching.
                }
            }
        }

        if message.source() != pkt.from()? {
            match (old_route_from, self.get_route_to(Some(pkt.from()?))) {
                (None, Some(new_route)) => {
                    tracing::event!(
                        Level::DEBUG,
                        from = %mac,
                        to = %pkt.from()?,
                        through = %new_route,
                        "route created on heartbeat",
                    );
                }
                (_, None) => (),
                (Some(old_route_from), Some(new_route)) => {
                    if old_route_from.mac != new_route.mac {
                        tracing::event!(
                            Level::DEBUG,
                            from = %mac,
                            to = %pkt.from()?,
                            through = %new_route,
                            was_through = %old_route_from,
                            "route changed on heartbeat",
                        );
                        // Do not clear cached upstream; hysteresis in get_route_to will decide switching.
                    }
                }
            }
        }

        Ok(Some(vec![
            ReplyType::Wire(
                (&Message::new(
                    mac,
                    [255; 6].into(),
                    PacketType::Control(Control::Heartbeat(message.clone())),
                ))
                    .into(),
            ),
            ReplyType::Wire(
                (&Message::new(
                    mac,
                    pkt.from()?,
                    PacketType::Control(Control::HeartbeatReply(HeartbeatReply::from_sender(
                        message, mac,
                    ))),
                ))
                    .into(),
            ),
        ]))
    }

    pub fn handle_heartbeat_reply(
        &mut self,
        pkt: &Message,
        mac: MacAddress,
    ) -> Result<Option<Vec<ReplyType>>> {
        let PacketType::Control(Control::HeartbeatReply(message)) = pkt.get_packet_type() else {
            bail!("this is supposed to be a HeartBeat Reply");
        };

        let old_route = self.get_route_to(Some(message.sender()));
        let old_route_from = self.get_route_to(Some(pkt.from()?));
        let Some(source_entries) = self.routes.get_mut(&message.source()) else {
            bail!("we don't know how to reach that source");
        };

        // Read the recorded duration and next_upstream immutably so we can
        // decide action without holding a mutable borrow of the routing
        // structures. We'll perform downstream updates in a short mutable
        // scope below.
        let next_upstream_copy = {
            let Some((_, next_upstream, _, _, _)) = source_entries.get(&message.id()) else {
                bail!("no recollection of the next hop for this route");
            };
            *next_upstream
        };

        // Note: avoid forwarding the HeartbeatReply back to the node it came
        // from. If `pkt.from()` equals our recorded `next_upstream`, sending a
        // reply to `next_upstream` would immediately bounce the packet back and
        // can create a forwarding loop. We'll still record downstream
        // observations below, but skip forwarding in that case.

        // Decide action and emit a trace-level log so we can inspect decisions
        // in live runs. Action values:
        //  - "bail" : next_upstream == message.sender() (genuine loop)
        //  - "skip_forward" : pkt.from == next_upstream (would bounce)
        //  - "forward" : safe to forward toward next_upstream
        let pkt_from = pkt.from()?;
        let sender = message.sender();
        let action = if next_upstream_copy == sender {
            "bail"
        } else if pkt_from == next_upstream_copy {
            "skip_forward"
        } else {
            "forward"
        };

        tracing::debug!(
            pkt_from = %pkt_from,
            message_sender = %sender,
            next_upstream = %next_upstream_copy,
            action = %action,
            "heartbeat_reply decision"
        );

        if action == "bail" {
            #[cfg(feature = "stats")]
            crate::metrics::inc_loop_detected();
            bail!("loop detected");
        }

        // Update downstream observation lists inside a short mutable scope so
        // we don't hold a mutable borrow across the subsequent `select_and_cache_upstream` call.
        {
            let Some((duration, _next_upstream, _, _, downstream)) =
                source_entries.get_mut(&message.id())
            else {
                bail!("no recollection of the next hop for this route");
            };

            let seen_at = Instant::now().duration_since(self.boot);
            let latency = seen_at - *duration;
            match downstream.entry(message.sender()) {
                Entry::Occupied(mut entry) => {
                    let value = entry.get_mut();

                    value.push(Target {
                        hops: message.hops(),
                        mac: pkt.from()?,
                        latency: Some(latency),
                    });
                }
                Entry::Vacant(entry) => {
                    entry.insert(vec![Target {
                        hops: message.hops(),
                        mac: pkt.from()?,
                        latency: Some(latency),
                    }]);
                }
            };

            match downstream.entry(pkt.from()?) {
                Entry::Occupied(mut entry) => {
                    let value = entry.get_mut();

                    value.push(Target {
                        hops: 1,
                        mac: pkt.from()?,
                        latency: None,
                    });
                }
                Entry::Vacant(entry) => {
                    entry.insert(vec![Target {
                        hops: 1,
                        mac: pkt.from()?,
                        latency: None,
                    }]);
                }
            };
        }

        // Attempt to select and cache an upstream for the original heartbeat
        // source now that we've recorded downstream observations. Do this
        // before the early-return below so replies that would be skipped for
        // forwarding still cause caching.
        let selected = self.select_and_cache_upstream(message.source());
        tracing::debug!(selection = ?selected.as_ref().map(|r| r.mac), "after heartbeat_reply: select_and_cache_upstream");

        // If the reply arrived from the node we'd forward to, don't forward
        // it back: that would produce an immediate bounce. Drop forwarding
        // (but keep the recorded downstream information above).
        if pkt.from()? == next_upstream_copy {
            return Ok(None);
        }

        let sender = message.sender();
        let reply = Ok(Some(vec![ReplyType::Wire(
            (&Message::new(
                mac,
                next_upstream_copy,
                PacketType::Control(Control::HeartbeatReply(message.clone())),
            ))
                .into(),
        )]));

        match (old_route, self.get_route_to(Some(sender))) {
            (None, Some(new_route)) => {
                tracing::event!(
                    Level::DEBUG,
                    from = %mac,
                    to = %sender,
                    through = %new_route,
                    "route created on heartbeat reply",
                );
            }
            (_, None) => (),
            (Some(old_route), Some(new_route)) => {
                if old_route.mac != new_route.mac {
                    tracing::event!(
                        Level::DEBUG,
                        from = %mac,
                        to = %sender,
                        through = %new_route,
                        was_through = %old_route,
                        "route changed on heartbeat reply",
                    );
                    // Do not clear cached upstream; hysteresis in get_route_to will decide switching.
                }
            }
        }

        if sender != pkt.from()? {
            match (old_route_from, self.get_route_to(Some(pkt.from()?))) {
                (None, Some(new_route)) => {
                    tracing::event!(
                        Level::DEBUG,
                        from = %mac,
                        to = %pkt.from()?,
                        through = %new_route,
                        "route created on heartbeat reply",
                    );
                }
                (_, None) => (),
                (Some(old_route_from), Some(new_route)) => {
                    if old_route_from.mac != new_route.mac {
                        tracing::event!(
                            Level::DEBUG,
                            from = %mac,
                            to = %pkt.from()?,
                            through = %new_route,
                            was_through = %old_route_from,
                            "route changed on heartbeat reply",
                        );
                        // Do not clear cached upstream; hysteresis in get_route_to will decide switching.
                    }
                }
            }
        }

        reply
    }

    pub fn get_route_to(&self, mac: Option<MacAddress>) -> Option<Route> {
        let Some(target_mac) = mac else {
            return self.cached_upstream.load().as_ref().map(|mac| Route {
                hops: 1,
                mac: **mac,
                latency: None,
            });
        };
        // Optionally incorporate hysteresis against the currently cached upstream.
        // We will compute the usual "best" candidate, but if it differs from the
        // cached upstream we only switch when it's better by a margin (>=10% lower
        // latency score) or uses at least one fewer hop. Otherwise, we keep the
        // current next hop to avoid flapping.
        let cached = self.get_cached_upstream();
        let mut upstream_routes: Vec<_> = self
            .routes
            .iter()
            .flat_map(|(rsu_mac, seqs)| {
                seqs.iter()
                    .map(move |(seq, (_, mac, hops, _, _))| (seq, rsu_mac, mac, hops))
            })
            .filter(|(_, rsu_mac, _, _)| rsu_mac == &&target_mac)
            .collect();
        upstream_routes.sort_by(|(_, _, _, hops), (_, _, _, bhops)| hops.cmp(bhops));

        // Prepare references to per-seq downstream observations, grouped by RSU
        let route_options: IndexMap<_, _> = self
            .routes
            .iter()
            .flat_map(|(rsus, im)| {
                im.iter()
                    .map(move |(seq, (dur, mac, hops, _r, downstream))| {
                        (seq, (dur, mac, hops, downstream, rsus))
                    })
            })
            .collect();

        // Compute deterministic integer-based metrics for latency in microseconds across ALL hops.
        // Prefer lower latency first; break ties by fewer hops.
        let latency_candidates: HashMap<MacAddress, (u128, u128, u32, u32)> = route_options
            .iter()
            .rev()
            .filter_map(|(_seq, (_dur, _mac, _hops, downstream, rsu_mac))| {
                if *rsu_mac == &target_mac {
                    downstream.get(&target_mac)
                } else {
                    None
                }
            })
            .flat_map(|route_vec| route_vec.iter())
            .fold(
                HashMap::default(),
                |mut hm: HashMap<MacAddress, (u128, u128, u32, u32)>, route| {
                    let hop_val = route.hops;
                    if let Some(lat) = route.latency.map(|x| x.as_micros()) {
                        let entry =
                            hm.entry(route.mac)
                                .or_insert((u128::MAX, 0u128, 0u32, hop_val));
                        if entry.0 > lat as u128 {
                            entry.0 = lat as u128; // min
                        }
                        entry.1 += lat as u128; // sum
                        entry.2 += 1; // count
                        entry.3 = hop_val; // keep latest hops (they should be consistent per mac)
                    }
                    hm
                },
            );

        if !latency_candidates.is_empty() {
            // Select by (score = min + avg), then by hops
            let mut scored: Vec<_> = latency_candidates
                .iter()
                .map(|(mac, (min_us, sum_us, n, hops_val))| {
                    let avg_us = if *n > 0 {
                        *sum_us / (*n as u128)
                    } else {
                        u128::MAX
                    };
                    let score = if *min_us == u128::MAX || avg_us == u128::MAX {
                        u128::MAX
                    } else {
                        *min_us + avg_us
                    };
                    (score, *hops_val, *mac, avg_us)
                })
                .collect();
            scored.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
            let (best_score, best_hops, best_mac, best_avg) = scored[0];

            // If cached is set but isn't in latency candidates (no latency observed yet),
            // keep cached unless best has at least one fewer hop.
            if let Some(cached_mac) = cached {
                if !latency_candidates.contains_key(&cached_mac) {
                    if let Some((_, _, _, cached_hops_ref)) = upstream_routes
                        .iter()
                        .find(|(_, _, mac_ref, _)| **mac_ref == cached_mac)
                    {
                        let cached_hops = **cached_hops_ref;
                        if best_mac != cached_mac {
                            let fewer_hops = best_hops + 1 <= cached_hops;
                            if !fewer_hops {
                                return Some(Route {
                                    hops: cached_hops,
                                    mac: cached_mac,
                                    latency: None,
                                });
                            }
                        }
                    }
                }
            }

            // If we have a cached upstream that is also a candidate for this RSU,
            // apply hysteresis: stick to cached unless the new one is clearly better.
            if let Some(cached_mac) = cached {
                if let Some((cached_min, cached_sum, cached_n, cached_hops)) =
                    latency_candidates.get(&cached_mac).copied()
                {
                    let cached_avg = if cached_n > 0 {
                        cached_sum / (cached_n as u128)
                    } else {
                        u128::MAX
                    };
                    let cached_score = if cached_min == u128::MAX || cached_avg == u128::MAX {
                        u128::MAX
                    } else {
                        cached_min + cached_avg
                    };

                    // If best is the cached, just return it.
                    if best_mac == cached_mac {
                        return Some(Route {
                            hops: best_hops,
                            mac: best_mac,
                            latency: if best_avg == u128::MAX {
                                None
                            } else {
                                Some(Duration::from_micros(best_avg as u64))
                            },
                        });
                    }

                    // Switching conditions:
                    // - strictly fewer hops by at least 1
                    // - or latency score better by >=10%
                    let fewer_hops = best_hops + 1 <= cached_hops;
                    let latency_better_enough =
                        if cached_score == u128::MAX && best_score != u128::MAX {
                            true // prefer finite measurement over unknown
                        } else if cached_score == u128::MAX || best_score == u128::MAX {
                            false
                        } else {
                            // new_score <= cached_score * 0.9 (10% or more better)
                            best_score.saturating_mul(10) < cached_score.saturating_mul(9)
                        };

                    if !(fewer_hops || latency_better_enough) {
                        // Keep cached
                        return Some(Route {
                            hops: cached_hops,
                            mac: cached_mac,
                            latency: if cached_avg == u128::MAX {
                                None
                            } else {
                                Some(Duration::from_micros(cached_avg as u64))
                            },
                        });
                    }
                }
            }

            // Default: return the best candidate
            return Some(Route {
                hops: best_hops,
                mac: best_mac,
                latency: if best_avg == u128::MAX {
                    None
                } else {
                    Some(Duration::from_micros(best_avg as u64))
                },
            });
        }

        // Fallback: no latency observed yet, prefer fewer hops (original behavior)
        if let Some((_, _, best_mac_ref, best_hops_ref)) = upstream_routes.first() {
            let best_mac = **best_mac_ref;
            let best_hops = **best_hops_ref;

            // Apply hysteresis with hops-only info when we don't have latency.
            if let Some(cached_mac) = cached {
                if let Some((_, _, _, cached_hops_ref)) = upstream_routes
                    .iter()
                    .find(|(_, _, mac_ref, _)| **mac_ref == cached_mac)
                {
                    let cached_hops = **cached_hops_ref;
                    if best_mac != cached_mac {
                        let fewer_hops = best_hops + 1 <= cached_hops; // switch only if at least one fewer hop
                        if !fewer_hops {
                            return Some(Route {
                                hops: cached_hops,
                                mac: cached_mac,
                                latency: None,
                            });
                        }
                    }
                }
            }

            return Some(Route {
                hops: best_hops,
                mac: best_mac,
                latency: None,
            });
        }
        None
    }

    /// Compute the best route to `mac` and store it in the cached upstream.
    /// This is the write API callers should use when they want selection to
    /// also update the cached upstream. This separates the pure selection
    /// logic (above) from the side-effect of caching.
    pub fn select_and_cache_upstream(&self, mac: MacAddress) -> Option<Route> {
        let route = self.get_route_to(Some(mac))?;
        tracing::info!(upstream = %route.mac, source = %mac, "select_and_cache_upstream selected upstream for source");
        self.cached_upstream.store(Some(route.mac.into()));
        #[cfg(feature = "stats")]
        crate::metrics::inc_cache_select();
        Some(route)
    }
}
