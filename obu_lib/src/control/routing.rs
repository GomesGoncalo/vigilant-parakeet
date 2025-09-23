use super::{node::ReplyType, route::Route};
use crate::args::ObuArgs;
use anyhow::{bail, Result};
use arc_swap::ArcSwapOption;
use indexmap::IndexMap;
use mac_address::MacAddress;
use node_lib::messages::{
    control::{heartbeat::HeartbeatReply, Control},
    message::Message,
    packet_type::PacketType,
};
use std::collections::{hash_map::Entry, HashMap, HashSet};
use std::sync::Arc;
use tokio::time::{Duration, Instant};
use tracing::Level;

#[derive(Debug)]
struct Target {
    hops: u32,
    mac: MacAddress,
    latency: Option<Duration>,
}

#[cfg(test)]
mod extra_tests {
    use super::*;
    use crate::args::{ObuArgs, ObuParameters};
    use indexmap::IndexMap;
    use std::collections::HashMap;
    use std::time::Duration;

    // Ensure failover_cached_upstream can rebuild candidate list from latency observations
    #[test]
    fn failover_rebuilds_candidates_from_latency() {
        let args = ObuArgs {
            bind: String::new(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            obu_params: ObuParameters {
                hello_history: 2,
                cached_candidates: 3,
                enable_encryption: false,
            },
        };

        let boot = Instant::now();
        let mut routing = Routing::new(&args, &boot).expect("routing built");

        // Prepare an RSU entry whose downstream map contains latencyed observations
        let src: MacAddress = [9u8; 6].into(); // cached_source we want rebuilt for
        let rsu: MacAddress = [100u8; 6].into();
        let mut seqmap = IndexMap::new();
        let mut downstream_map: HashMap<MacAddress, Vec<Target>> = HashMap::new();

        // Two candidate next-hops with measured latencies; the lower-latency one should be preferred
        downstream_map.insert(
            src,
            vec![
                Target {
                    hops: 1,
                    mac: [1u8; 6].into(),
                    latency: Some(Duration::from_millis(20)),
                },
                Target {
                    hops: 1,
                    mac: [2u8; 6].into(),
                    latency: Some(Duration::from_millis(5)),
                },
            ],
        );

        seqmap.insert(
            0u32,
            (
                Duration::from_millis(0),
                rsu,
                1u32,
                IndexMap::new(),
                downstream_map,
            ),
        );
        routing.routes.insert(rsu, seqmap);

        // Ensure cached_candidates empty so failover path rebuilds
        routing.cached_candidates.store(None);
        routing.cached_upstream.store(None);
        routing.cached_source.store(Some(src.into()));

        // Call failover; this should rebuild candidates and return a primary
        let promoted = routing.failover_cached_upstream();
        assert!(promoted.is_some());
        // After rebuild, cached_candidates should be populated
        let cands = routing.get_cached_candidates();
        assert!(cands.is_some());
        let c = cands.unwrap();
        assert!(!c.is_empty());
    }

    // Ensure select_and_cache_upstream backfills using hops-only when no latency data
    #[test]
    fn select_and_cache_upstream_backfills_by_hops() {
        let args = ObuArgs {
            bind: String::new(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            obu_params: ObuParameters {
                hello_history: 2,
                cached_candidates: 3,
                enable_encryption: false,
            },
        };

        let boot = Instant::now();
        let mut routing = Routing::new(&args, &boot).expect("routing built");

        // Create two upstream routes (no latency measurements) with different hop counts
        let rsu: MacAddress = [200u8; 6].into();
        let mut seqmap = IndexMap::new();

        // target is the RSU itself here; populate upstream entries for this RSU
        let via_a: MacAddress = [11u8; 6].into();
        let via_b: MacAddress = [12u8; 6].into();

        // No downstream entries required for hops-only selection; instead insert upstream_routes
        seqmap.insert(
            1u32,
            (
                Duration::from_millis(0),
                via_a,
                2u32, // hops
                IndexMap::new(),
                HashMap::new(),
            ),
        );
        seqmap.insert(
            2u32,
            (
                Duration::from_millis(0),
                via_b,
                1u32, // fewer hops, should be preferred
                IndexMap::new(),
                HashMap::new(),
            ),
        );
        routing.routes.insert(rsu, seqmap);

        // Now call select_and_cache_upstream and expect cached_candidates to include via_b first
        let selected = routing
            .select_and_cache_upstream(rsu)
            .expect("selected route");
        let cached = routing.get_cached_candidates().expect("cands");
        assert!(!cached.is_empty());
        // First candidate should be the one with fewer hops (via_b)
        assert_eq!(cached[0], via_b);
        // And selected primary should match
        assert_eq!(selected.mac, cached[0]);
    }

    #[test]
    fn test_set_cached_candidates_clears_and_sets() {
        let args = ObuArgs {
            bind: String::new(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            obu_params: ObuParameters {
                hello_history: 2,
                cached_candidates: 3,
                enable_encryption: false,
            },
        };

        let boot = Instant::now();
        let routing = Routing::new(&args, &boot).expect("routing built");

        // Start with empty candidates
        routing.test_set_cached_candidates(vec![]);
        assert!(routing.get_cached_candidates().is_none());
        assert!(routing.get_cached_upstream().is_none());

        // Set some candidates
        let a: MacAddress = [1u8; 6].into();
        let b: MacAddress = [2u8; 6].into();
        routing.test_set_cached_candidates(vec![a, b]);
        let c = routing.get_cached_candidates().expect("cands");
        assert_eq!(c[0], a);
        assert_eq!(routing.get_cached_upstream().expect("primary"), a);
    }

    #[test]
    fn failover_backfills_from_hops_when_no_latency() {
        let args = ObuArgs {
            bind: String::new(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            obu_params: ObuParameters {
                hello_history: 2,
                cached_candidates: 3,
                enable_encryption: false,
            },
        };

        let boot = Instant::now();
        let mut routing = Routing::new(&args, &boot).expect("routing built");

        // Build routes where only hop-count information is available
        let src: MacAddress = [9u8; 6].into();
        // Use src as the RSU key so the hops-based backfill path finds entries
        let rsu: MacAddress = src;
        let via_a: MacAddress = [11u8; 6].into();
        let via_b: MacAddress = [12u8; 6].into();
        let mut seqmap = IndexMap::new();

        seqmap.insert(
            1u32,
            (
                Duration::from_millis(0),
                via_a,
                3u32, // more hops
                IndexMap::new(),
                HashMap::new(),
            ),
        );
        seqmap.insert(
            2u32,
            (
                Duration::from_millis(0),
                via_b,
                1u32, // fewer hops, should be preferred
                IndexMap::new(),
                HashMap::new(),
            ),
        );
        routing.routes.insert(rsu, seqmap);

        // Set cached source so failover can rebuild
        routing.cached_candidates.store(None);
        routing.cached_upstream.store(None);
        routing.cached_source.store(Some(src.into()));

        let promoted = routing.failover_cached_upstream();
        // Should have promoted something
        assert!(promoted.is_some());
        let primary = routing.get_cached_upstream().expect("primary");
        // promoted should match the cached upstream
        assert_eq!(primary, promoted.unwrap());
        // And the primary should be one of the candidates we inserted
        let cands = routing.get_cached_candidates().expect("cands");
        assert!(cands.contains(&primary));
    }
}

#[cfg(test)]
mod tests {
    use super::Routing;
    use crate::args::{ObuArgs, ObuParameters};
    use node_lib::messages::{
        control::heartbeat::Heartbeat, control::heartbeat::HeartbeatReply, control::Control,
        message::Message, packet_type::PacketType,
    };
    // ReplyType is not used in these test helpers; remove unused import.
    use mac_address::MacAddress;
    use tokio::time::{Duration, Instant};

    #[test]
    fn handle_heartbeat_creates_route_and_returns_replies() {
        let args = ObuArgs {
            bind: String::default(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            obu_params: ObuParameters {
                hello_history: 2,
                cached_candidates: 3,
                enable_encryption: false,
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
        let args = ObuArgs {
            bind: String::default(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            obu_params: ObuParameters {
                hello_history: 2,
                cached_candidates: 3,
                enable_encryption: false,
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
    use crate::args::{ObuArgs, ObuParameters};
    use mac_address::MacAddress;
    use tokio::time::{Duration, Instant};

    #[test]
    fn select_and_cache_upstream_sets_cache() {
        let args = ObuArgs {
            bind: String::default(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            obu_params: ObuParameters {
                hello_history: 2,
                cached_candidates: 3,
                enable_encryption: false,
            },
        };

        let boot = Instant::now() - Duration::from_secs(1);
        let mut routing = Routing::new(&args, &boot).expect("routing built");

        // Create a heartbeat to populate routes
        let hb_source: MacAddress = [7u8; 6].into();
        let pkt_from: MacAddress = [8u8; 6].into();
        let our_mac: MacAddress = [9u8; 6].into();
        let hb = node_lib::messages::control::heartbeat::Heartbeat::new(
            std::time::Duration::from_millis(1),
            1u32,
            hb_source,
        );
        let hb_msg = node_lib::messages::message::Message::new(
            pkt_from,
            [255u8; 6].into(),
            node_lib::messages::packet_type::PacketType::Control(
                node_lib::messages::control::Control::Heartbeat(hb.clone()),
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

    #[test]
    fn failover_promotes_next_candidate() {
        let args = ObuArgs {
            bind: String::default(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            obu_params: ObuParameters {
                hello_history: 2,
                cached_candidates: 3,
                enable_encryption: false,
            },
        };

        let boot = Instant::now() - Duration::from_secs(1);
        let mut routing = Routing::new(&args, &boot).expect("routing built");

        // Create a heartbeat to populate routes and select primary
        let hb_source: MacAddress = [7u8; 6].into();
        let pkt_from: MacAddress = [8u8; 6].into();
        let our_mac: MacAddress = [9u8; 6].into();
        let hb = node_lib::messages::control::heartbeat::Heartbeat::new(
            std::time::Duration::from_millis(1),
            1u32,
            hb_source,
        );
        let hb_msg = node_lib::messages::message::Message::new(
            pkt_from,
            [255u8; 6].into(),
            node_lib::messages::packet_type::PacketType::Control(
                node_lib::messages::control::Control::Heartbeat(hb.clone()),
            ),
        );
        let _ = routing
            .handle_heartbeat(&hb_msg, our_mac)
            .expect("handled hb");

        // Now select and cache the upstream for hb_source
        let _ = routing
            .select_and_cache_upstream(hb_source)
            .expect("selected");

        // Ensure we have candidates stored; for test determinism populate two candidates
        use std::sync::Arc;
        let primary_before = routing.get_cached_upstream().expect("primary");
        let next_candidate: MacAddress = [11u8; 6].into();
        // store ordered candidates [primary, next]
        routing
            .cached_candidates
            .store(Some(Arc::new(vec![primary_before, next_candidate])));
        routing.cached_upstream.store(Some(primary_before.into()));

        // Simulate a send failure by directly calling failover_cached_upstream()
        let promoted = routing.failover_cached_upstream();
        assert!(promoted.is_some());
        let primary_after = routing.get_cached_upstream().expect("primary after");
        assert_ne!(primary_before, primary_after);
        assert_eq!(primary_after, promoted.unwrap());
    }
}

#[cfg(test)]
mod regression_tests {
    use super::Routing;
    use crate::args::{ObuArgs, ObuParameters};
    use mac_address::MacAddress;
    use node_lib::messages::control::heartbeat::{Heartbeat, HeartbeatReply};
    use node_lib::messages::{control::Control, message::Message, packet_type::PacketType};
    use tokio::time::Instant;

    // Regression test for the case where a HeartbeatReply arrives from the
    // recorded next hop (pkt.from() == next_upstream). Previously the code
    // treated that as a loop and bailed; that's incorrect. We should only
    // bail if the recorded next_upstream equals the HeartbeatReply's
    // reported sender (message.sender()). This test asserts we do not bail
    // when pkt.from() == next_upstream but message.sender() != next_upstream.
    #[test]
    fn heartbeat_reply_from_next_hop_does_not_bail() {
        let args = ObuArgs {
            bind: String::default(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            obu_params: ObuParameters {
                hello_history: 2,
                cached_candidates: 3,
                enable_encryption: false,
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
        let args = ObuArgs {
            bind: String::default(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            obu_params: ObuParameters {
                hello_history: 2,
                cached_candidates: 3,
                enable_encryption: false,
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
        assert!(format!("{err}").contains("loop detected"));
    }
}

#[cfg(test)]
mod more_tests {
    use super::Routing;
    use super::Target;
    use crate::args::{ObuArgs, ObuParameters};

    use mac_address::MacAddress;
    use tokio::time::{Duration, Instant};

    #[test]
    fn get_route_to_none_when_empty() {
        let args = ObuArgs {
            bind: String::default(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            obu_params: ObuParameters {
                hello_history: 2,
                cached_candidates: 3,
                enable_encryption: false,
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
    fn tie_break_prefers_lower_mac_when_scores_equal() {
        // Build args and routing
        let args = ObuArgs {
            bind: String::default(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            obu_params: ObuParameters {
                hello_history: 2,
                cached_candidates: 3,
                enable_encryption: false,
            },
        };
        let boot = Instant::now() - Duration::from_secs(1);
        let mut routing = Routing::new(&args, &boot).expect("routing built");

        // We'll manually populate routes to create two candidates with equal score
        // Candidate A: MAC [1,0,0,0,0,1], Candidate B: MAC [2,0,0,0,0,2]
        let target: MacAddress = [9u8; 6].into();
        let candidate_a: MacAddress = [1u8, 0, 0, 0, 0, 1].into();
        let candidate_b: MacAddress = [2u8, 0, 0, 0, 0, 2].into();

        // Populate `routes` with downstream observations for a single RSU/seq
        let rsu_mac: MacAddress = [100u8; 6].into();
        let seq = 0u32;
        let mut seqmap = indexmap::IndexMap::new();
        let mut downstream_map: std::collections::HashMap<MacAddress, Vec<Target>> =
            std::collections::HashMap::new();

        // Both candidates have same hops and same latency values so score equal
        downstream_map.insert(
            target,
            vec![
                Target {
                    hops: 2,
                    mac: candidate_a,
                    latency: Some(Duration::from_millis(10)),
                },
                Target {
                    hops: 2,
                    mac: candidate_b,
                    latency: Some(Duration::from_millis(10)),
                },
            ],
        );

        seqmap.insert(
            seq,
            (
                Duration::from_millis(0),
                rsu_mac,
                1u32,
                indexmap::IndexMap::new(),
                downstream_map,
            ),
        );
        routing.routes.insert(rsu_mac, seqmap);

        // Now ask for route to target; since scores tie, the lower MAC should win
        let route = routing.get_route_to(Some(target)).expect("route present");
        assert!(route.mac.bytes() < candidate_b.bytes());
    }

    #[test]
    fn none_latency_handling_prefers_min_and_none_ignored_in_avg() {
        let args = ObuArgs {
            bind: String::default(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            obu_params: ObuParameters {
                hello_history: 2,
                cached_candidates: 3,
                enable_encryption: false,
            },
        };
        let boot = Instant::now() - Duration::from_secs(1);
        let mut routing = Routing::new(&args, &boot).expect("routing built");

        let target: MacAddress = [9u8; 6].into();
        let candidate_with_none: MacAddress = [5u8; 6].into();
        let candidate_with_val: MacAddress = [6u8; 6].into();

        let rsu_mac: MacAddress = [101u8; 6].into();
        let seq = 0u32;
        let mut seqmap = indexmap::IndexMap::new();
        let mut downstream_map: std::collections::HashMap<MacAddress, Vec<Target>> =
            std::collections::HashMap::new();

        // Candidate A has None latency (unmeasured), Candidate B has concrete latencies
        downstream_map.insert(
            target,
            vec![
                Target {
                    hops: 1,
                    mac: candidate_with_none,
                    latency: None,
                },
                Target {
                    hops: 1,
                    mac: candidate_with_val,
                    latency: Some(Duration::from_millis(50)),
                },
            ],
        );

        seqmap.insert(
            seq,
            (
                Duration::from_millis(0),
                rsu_mac,
                1u32,
                indexmap::IndexMap::new(),
                downstream_map,
            ),
        );
        routing.routes.insert(rsu_mac, seqmap);

        // Candidate with measured latency should be preferred since None is treated as MAX
        let route = routing.get_route_to(Some(target)).expect("route present");
        assert_eq!(route.mac, candidate_with_val);
    }

    #[test]
    fn duplicate_heartbeat_returns_none() {
        let args = ObuArgs {
            bind: String::default(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            obu_params: ObuParameters {
                hello_history: 4,
                cached_candidates: 3,
                enable_encryption: false,
            },
        };

        let boot = Instant::now();
        let mut routing = Routing::new(&args, &boot).expect("routing built");

        let hb_source: MacAddress = [5u8; 6].into();
        let pkt_from: MacAddress = [6u8; 6].into();
        let our_mac: MacAddress = [7u8; 6].into();
        let hb = node_lib::messages::control::heartbeat::Heartbeat::new(
            std::time::Duration::from_millis(1),
            123u32,
            hb_source,
        );
        let hb_msg = node_lib::messages::message::Message::new(
            pkt_from,
            [255u8; 6].into(),
            node_lib::messages::packet_type::PacketType::Control(
                node_lib::messages::control::Control::Heartbeat(hb.clone()),
            ),
        );

        let first = routing.handle_heartbeat(&hb_msg, our_mac).expect("hb1");
        assert!(first.is_some());
        let second = routing.handle_heartbeat(&hb_msg, our_mac).expect("hb2");
        assert!(second.is_none(), "duplicate id should be ignored");
    }

    #[test]
    fn hello_history_eviction_keeps_latest() {
        let args = ObuArgs {
            bind: String::default(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            obu_params: ObuParameters {
                hello_history: 1,
                cached_candidates: 3,
                enable_encryption: false,
            },
        };

        let boot = Instant::now();
        let mut routing = Routing::new(&args, &boot).expect("routing built");

        let hb_source: MacAddress = [1u8, 1, 1, 1, 1, 1].into();
        let pkt_from: MacAddress = [2u8, 2, 2, 2, 2, 2].into();
        let our_mac: MacAddress = [9u8; 6].into();

        let hb1 = node_lib::messages::control::heartbeat::Heartbeat::new(
            std::time::Duration::from_millis(1),
            1u32,
            hb_source,
        );
        let msg1 = node_lib::messages::message::Message::new(
            pkt_from,
            [255u8; 6].into(),
            node_lib::messages::packet_type::PacketType::Control(
                node_lib::messages::control::Control::Heartbeat(hb1.clone()),
            ),
        );
        let _ = routing.handle_heartbeat(&msg1, our_mac).unwrap();

        let hb2 = node_lib::messages::control::heartbeat::Heartbeat::new(
            std::time::Duration::from_millis(2),
            2u32,
            hb_source,
        );
        let msg2 = node_lib::messages::message::Message::new(
            pkt_from,
            [255u8; 6].into(),
            node_lib::messages::packet_type::PacketType::Control(
                node_lib::messages::control::Control::Heartbeat(hb2.clone()),
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
        let args = ObuArgs {
            bind: String::default(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            obu_params: ObuParameters {
                hello_history: 4,
                cached_candidates: 3,
                enable_encryption: false,
            },
        };

        let boot = Instant::now();
        let mut routing = Routing::new(&args, &boot).expect("routing built");

        let hb_source: MacAddress = [9u8, 8, 7, 6, 5, 4].into();
        let pkt_from: MacAddress = [1u8, 2, 3, 4, 5, 6].into();
        let our_mac: MacAddress = [0u8; 6].into();

        // Insert id 10 first
        let hb10 = node_lib::messages::control::heartbeat::Heartbeat::new(
            std::time::Duration::from_millis(10),
            10u32,
            hb_source,
        );
        let msg10 = node_lib::messages::message::Message::new(
            pkt_from,
            [255u8; 6].into(),
            node_lib::messages::packet_type::PacketType::Control(
                node_lib::messages::control::Control::Heartbeat(hb10.clone()),
            ),
        );
        let _ = routing.handle_heartbeat(&msg10, our_mac).unwrap();

        // Now insert smaller id 5, which should clear existing entries
        let hb5 = node_lib::messages::control::heartbeat::Heartbeat::new(
            std::time::Duration::from_millis(5),
            5u32,
            hb_source,
        );
        let msg5 = node_lib::messages::message::Message::new(
            pkt_from,
            [255u8; 6].into(),
            node_lib::messages::packet_type::PacketType::Control(
                node_lib::messages::control::Control::Heartbeat(hb5.clone()),
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
        let args = ObuArgs {
            bind: String::default(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            obu_params: ObuParameters {
                hello_history: 2,
                cached_candidates: 3,
                enable_encryption: false,
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
        let args = ObuArgs {
            bind: String::default(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            obu_params: ObuParameters {
                hello_history: 2,
                cached_candidates: 3,
                enable_encryption: false,
            },
        };

        let boot = Instant::now();
        let mut routing = Routing::new(&args, &boot).expect("routing built");
        let hb_source: MacAddress = [7u8; 6].into();
        let pkt_from: MacAddress = [8u8; 6].into();
        let our_mac: MacAddress = [9u8; 6].into();

        let hb = node_lib::messages::control::heartbeat::Heartbeat::new(
            std::time::Duration::from_millis(1),
            1u32,
            hb_source,
        );
        let hb_msg = node_lib::messages::message::Message::new(
            pkt_from,
            [255u8; 6].into(),
            node_lib::messages::packet_type::PacketType::Control(
                node_lib::messages::control::Control::Heartbeat(hb.clone()),
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
        let args = ObuArgs {
            bind: String::default(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            obu_params: ObuParameters {
                hello_history: 2,
                cached_candidates: 3,
                enable_encryption: false,
            },
        };

        let boot = Instant::now();
        let mut routing = Routing::new(&args, &boot).expect("routing built");

        let rsu: MacAddress = [1u8; 6].into();
        let via_b: MacAddress = [2u8; 6].into();
        let via_c: MacAddress = [3u8; 6].into();
        let our_mac: MacAddress = [9u8; 6].into();

        // Heartbeat from RSU via B with 2 hops
        let hb1 = node_lib::messages::control::heartbeat::Heartbeat::new(
            std::time::Duration::from_millis(1),
            1u32,
            rsu,
        );
        let msg1 = node_lib::messages::message::Message::new(
            via_b,
            [255u8; 6].into(),
            node_lib::messages::packet_type::PacketType::Control(
                node_lib::messages::control::Control::Heartbeat(hb1.clone()),
            ),
        );
        // Insert, then cache selection chooses B
        let _ = routing.handle_heartbeat(&msg1, our_mac).unwrap();
        let sel1 = routing.select_and_cache_upstream(rsu).expect("selected");
        assert_eq!(sel1.mac, via_b);

        // Another Heartbeat from RSU via C with same hops (2)
        let hb2 = node_lib::messages::control::heartbeat::Heartbeat::new(
            std::time::Duration::from_millis(2),
            2u32,
            rsu,
        );
        let msg2 = node_lib::messages::message::Message::new(
            via_c,
            [255u8; 6].into(),
            node_lib::messages::packet_type::PacketType::Control(
                node_lib::messages::control::Control::Heartbeat(hb2.clone()),
            ),
        );
        let _ = routing.handle_heartbeat(&msg2, our_mac).unwrap();

        // get_route_to(Some) should prefer keeping cached (B) since hops are equal
        let route = routing.get_route_to(Some(rsu)).expect("route");
        assert_eq!(route.mac, via_b, "should keep cached when hops equal");
    }

    #[test]
    fn hysteresis_switches_when_one_fewer_hop() {
        let args = ObuArgs {
            bind: String::default(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            obu_params: ObuParameters {
                hello_history: 2,
                cached_candidates: 3,
                enable_encryption: false,
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
        let hb1 = node_lib::messages::control::heartbeat::Heartbeat::try_from(&hb1_bytes[..])
            .expect("hb1 bytes to heartbeat");
        let msg1 = node_lib::messages::message::Message::new(
            via_b,
            [255u8; 6].into(),
            node_lib::messages::packet_type::PacketType::Control(
                node_lib::messages::control::Control::Heartbeat(hb1.clone()),
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
        let hb2 = node_lib::messages::control::heartbeat::Heartbeat::try_from(&hb2_bytes[..])
            .expect("hb2 bytes to heartbeat");
        let msg2 = node_lib::messages::message::Message::new(
            via_c,
            [255u8; 6].into(),
            node_lib::messages::packet_type::PacketType::Control(
                node_lib::messages::control::Control::Heartbeat(hb2.clone()),
            ),
        );
        let _ = routing.handle_heartbeat(&msg2, our_mac).unwrap();

        // Now get_route_to(Some) should switch to C due to one fewer hop
        let route = routing.get_route_to(Some(rsu)).expect("route");
        assert_eq!(route.mac, via_c, "should switch when one fewer hop");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn hysteresis_latency_improvement_below_10_percent_keeps_cached() {
        let args = ObuArgs {
            bind: String::default(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            obu_params: ObuParameters {
                hello_history: 2,
                cached_candidates: 3,
                enable_encryption: false,
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
        let hb1 = node_lib::messages::control::heartbeat::Heartbeat::try_from(&hb1_bytes[..])
            .expect("hb1");
        let msg1 = node_lib::messages::message::Message::new(
            via_b,
            [255u8; 6].into(),
            node_lib::messages::packet_type::PacketType::Control(
                node_lib::messages::control::Control::Heartbeat(hb1.clone()),
            ),
        );
        let _ = routing.handle_heartbeat(&msg1, our_mac).unwrap();
        routing.select_and_cache_upstream(rsu).expect("cached B");

        // Advance 25ms between HB and HBR for B
        tokio::time::advance(Duration::from_millis(25)).await;
        let hbr1 = node_lib::messages::control::heartbeat::HeartbeatReply::from_sender(&hb1, rsu);
        let reply1 = node_lib::messages::message::Message::new(
            via_b,
            [255u8; 6].into(),
            node_lib::messages::packet_type::PacketType::Control(
                node_lib::messages::control::Control::HeartbeatReply(hbr1.clone()),
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
        let hb2 = node_lib::messages::control::heartbeat::Heartbeat::try_from(&hb2_bytes[..])
            .expect("hb2");
        let msg2 = node_lib::messages::message::Message::new(
            via_c,
            [255u8; 6].into(),
            node_lib::messages::packet_type::PacketType::Control(
                node_lib::messages::control::Control::Heartbeat(hb2.clone()),
            ),
        );
        let _ = routing.handle_heartbeat(&msg2, our_mac).unwrap();
        // Advance 23ms for C (less than 10% better than 25ms)
        tokio::time::advance(Duration::from_millis(23)).await;
        let hbr2 = node_lib::messages::control::heartbeat::HeartbeatReply::from_sender(&hb2, rsu);
        let reply2 = node_lib::messages::message::Message::new(
            via_c,
            [255u8; 6].into(),
            node_lib::messages::packet_type::PacketType::Control(
                node_lib::messages::control::Control::HeartbeatReply(hbr2.clone()),
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
    async fn hysteresis_latency_improvement_above_10_percent_switches() {
        let args = ObuArgs {
            bind: String::default(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            obu_params: ObuParameters {
                hello_history: 2,
                cached_candidates: 3,
                enable_encryption: false,
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
        let hb1 = node_lib::messages::control::heartbeat::Heartbeat::try_from(&hb1_bytes[..])
            .expect("hb1");
        let msg1 = node_lib::messages::message::Message::new(
            via_b,
            [255u8; 6].into(),
            node_lib::messages::packet_type::PacketType::Control(
                node_lib::messages::control::Control::Heartbeat(hb1.clone()),
            ),
        );
        let _ = routing.handle_heartbeat(&msg1, our_mac).unwrap();
        routing.select_and_cache_upstream(rsu).expect("cached B");

        // Advance 40ms for B
        tokio::time::advance(Duration::from_millis(40)).await;
        let hbr1 = node_lib::messages::control::heartbeat::HeartbeatReply::from_sender(&hb1, rsu);
        let reply1 = node_lib::messages::message::Message::new(
            via_b,
            [255u8; 6].into(),
            node_lib::messages::packet_type::PacketType::Control(
                node_lib::messages::control::Control::HeartbeatReply(hbr1.clone()),
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
        let hb2 = node_lib::messages::control::heartbeat::Heartbeat::try_from(&hb2_bytes[..])
            .expect("hb2");
        let msg2 = node_lib::messages::message::Message::new(
            via_c,
            [255u8; 6].into(),
            node_lib::messages::packet_type::PacketType::Control(
                node_lib::messages::control::Control::Heartbeat(hb2.clone()),
            ),
        );
        let _ = routing.handle_heartbeat(&msg2, our_mac).unwrap();

        // Advance 20ms for C (>= 10% better than 40ms)
        tokio::time::advance(Duration::from_millis(20)).await;
        let hbr2 = node_lib::messages::control::heartbeat::HeartbeatReply::from_sender(&hb2, rsu);
        let reply2 = node_lib::messages::message::Message::new(
            via_c,
            [255u8; 6].into(),
            node_lib::messages::packet_type::PacketType::Control(
                node_lib::messages::control::Control::HeartbeatReply(hbr2.clone()),
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

    #[tokio::test(flavor = "current_thread")]
    async fn hysteresis_prefers_measured_when_cached_unmeasured() {
        let args = ObuArgs {
            bind: String::default(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            obu_params: ObuParameters {
                hello_history: 2,
                cached_candidates: 3,
                enable_encryption: false,
            },
        };
        // Use paused time for deterministic latency measurement
        tokio::time::pause();
        let boot = Instant::now() - Duration::from_secs(1);
        let mut routing = Routing::new(&args, &boot).expect("routing built");

        let rsu: MacAddress = [50u8; 6].into();
        let via_b: MacAddress = [60u8; 6].into();
        let via_c: MacAddress = [70u8; 6].into();
        let our_mac: MacAddress = [111u8; 6].into();

        // Insert a heartbeat observed via B (id 1) and make it the cached primary
        let mut hb1_bytes = Vec::new();
        hb1_bytes.extend_from_slice(&0u128.to_be_bytes());
        hb1_bytes.extend_from_slice(&1u32.to_be_bytes());
        hb1_bytes.extend_from_slice(&0u32.to_be_bytes());
        hb1_bytes.extend_from_slice(&rsu.bytes());
        let hb1 = node_lib::messages::control::heartbeat::Heartbeat::try_from(&hb1_bytes[..])
            .expect("hb1");
        let msg1 = node_lib::messages::message::Message::new(
            via_b,
            [255u8; 6].into(),
            node_lib::messages::packet_type::PacketType::Control(
                node_lib::messages::control::Control::Heartbeat(hb1.clone()),
            ),
        );
        let _ = routing.handle_heartbeat(&msg1, our_mac).unwrap();
        // Force cached primary to via_b but do NOT record any latency for it
        routing.test_set_cached_candidates(vec![via_b]);

        // Now observe the RSU via C and record a latency for C
        let mut hb2_bytes = Vec::new();
        hb2_bytes.extend_from_slice(&0u128.to_be_bytes());
        hb2_bytes.extend_from_slice(&2u32.to_be_bytes());
        hb2_bytes.extend_from_slice(&0u32.to_be_bytes());
        hb2_bytes.extend_from_slice(&rsu.bytes());
        let hb2 = node_lib::messages::control::heartbeat::Heartbeat::try_from(&hb2_bytes[..])
            .expect("hb2");
        let msg2 = node_lib::messages::message::Message::new(
            via_c,
            [255u8; 6].into(),
            node_lib::messages::packet_type::PacketType::Control(
                node_lib::messages::control::Control::Heartbeat(hb2.clone()),
            ),
        );
        let _ = routing.handle_heartbeat(&msg2, our_mac).unwrap();

        // Advance time so the reply records a measurable latency for via_c
        tokio::time::advance(Duration::from_millis(30)).await;
        let hbr2 = node_lib::messages::control::heartbeat::HeartbeatReply::from_sender(&hb2, rsu);
        let reply2 = node_lib::messages::message::Message::new(
            via_c,
            [255u8; 6].into(),
            node_lib::messages::packet_type::PacketType::Control(
                node_lib::messages::control::Control::HeartbeatReply(hbr2.clone()),
            ),
        );
        // This will record latency for via_c and also trigger select_and_cache_upstream
        let _ = routing
            .handle_heartbeat_reply(&reply2, our_mac)
            .unwrap_or(None);

        // Now selection should prefer the measured candidate via_c even though
        // the cached primary (via_b) had no latency observations.
        let route = routing.get_route_to(Some(rsu)).expect("route");
        assert_eq!(
            route.mac, via_c,
            "should prefer measured candidate when cached unmeasured"
        );

        // And the cached upstream should reflect the switch after selection
        let cached = routing.get_route_to(None).expect("cached route");
        assert_eq!(
            cached.mac, via_c,
            "cached should reflect the measured switch"
        );
    }

    /// Comprehensive test for latency measurement with mocked time covering OBU functionality.
    /// This test verifies that OBU can measure latency and use it for routing decisions.
    #[tokio::test(flavor = "current_thread")]
    async fn test_latency_measurement_with_mocked_time() {
        // Use paused time for deterministic latency measurement
        tokio::time::pause();
        let boot = Instant::now();

        // Test OBU latency measurement and routing
        let obu_args = ObuArgs {
            bind: String::default(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            obu_params: ObuParameters {
                hello_history: 3,
                cached_candidates: 3,
                enable_encryption: false,
            },
        };
        let mut obu_routing = Routing::new(&obu_args, &boot).expect("OBU routing built");

        let rsu: MacAddress = [1u8; 6].into();
        let via_fast: MacAddress = [10u8; 6].into();
        let via_slow: MacAddress = [20u8; 6].into();
        let our_mac: MacAddress = [100u8; 6].into();

        // Test scenario: OBU receives heartbeats from RSU via two different paths
        // and should prefer the one with lower latency when hop counts are equal

        // Heartbeat via fast path (will have 10ms latency)
        let mut hb_fast_bytes = Vec::new();
        hb_fast_bytes.extend_from_slice(&0u128.to_be_bytes());
        hb_fast_bytes.extend_from_slice(&1u32.to_be_bytes()); // sequence id
        hb_fast_bytes.extend_from_slice(&1u32.to_be_bytes()); // 1 hop
        hb_fast_bytes.extend_from_slice(&rsu.bytes());
        let hb_fast =
            node_lib::messages::control::heartbeat::Heartbeat::try_from(&hb_fast_bytes[..])
                .expect("hb_fast");
        let msg_fast = node_lib::messages::message::Message::new(
            via_fast,
            [255u8; 6].into(),
            node_lib::messages::packet_type::PacketType::Control(
                node_lib::messages::control::Control::Heartbeat(hb_fast.clone()),
            ),
        );
        let _ = obu_routing.handle_heartbeat(&msg_fast, our_mac).unwrap();

        // Advance 10ms and reply
        tokio::time::advance(Duration::from_millis(10)).await;
        let hbr_fast =
            node_lib::messages::control::heartbeat::HeartbeatReply::from_sender(&hb_fast, rsu);
        let reply_fast = node_lib::messages::message::Message::new(
            via_fast,
            [255u8; 6].into(),
            node_lib::messages::packet_type::PacketType::Control(
                node_lib::messages::control::Control::HeartbeatReply(hbr_fast.clone()),
            ),
        );
        let _ = obu_routing
            .handle_heartbeat_reply(&reply_fast, our_mac)
            .unwrap_or(None);

        // Heartbeat via slow path (will have 30ms latency)
        let mut hb_slow_bytes = Vec::new();
        hb_slow_bytes.extend_from_slice(&0u128.to_be_bytes());
        hb_slow_bytes.extend_from_slice(&2u32.to_be_bytes()); // different sequence id
        hb_slow_bytes.extend_from_slice(&1u32.to_be_bytes()); // same hop count
        hb_slow_bytes.extend_from_slice(&rsu.bytes());
        let hb_slow =
            node_lib::messages::control::heartbeat::Heartbeat::try_from(&hb_slow_bytes[..])
                .expect("hb_slow");
        let msg_slow = node_lib::messages::message::Message::new(
            via_slow,
            [255u8; 6].into(),
            node_lib::messages::packet_type::PacketType::Control(
                node_lib::messages::control::Control::Heartbeat(hb_slow.clone()),
            ),
        );
        let _ = obu_routing.handle_heartbeat(&msg_slow, our_mac).unwrap();

        // Advance 30ms and reply
        tokio::time::advance(Duration::from_millis(30)).await;
        let hbr_slow =
            node_lib::messages::control::heartbeat::HeartbeatReply::from_sender(&hb_slow, rsu);
        let reply_slow = node_lib::messages::message::Message::new(
            via_slow,
            [255u8; 6].into(),
            node_lib::messages::packet_type::PacketType::Control(
                node_lib::messages::control::Control::HeartbeatReply(hbr_slow.clone()),
            ),
        );
        let _ = obu_routing
            .handle_heartbeat_reply(&reply_slow, our_mac)
            .unwrap_or(None);

        // OBU should prefer the fast path due to latency (since hop counts are equal)
        let route = obu_routing.get_route_to(Some(rsu)).expect("OBU route");
        assert_eq!(
            route.mac, via_fast,
            "OBU should prefer fast path based on latency measurement"
        );
        assert!(
            route.latency.is_some(),
            "OBU route should have latency measurement"
        );
        assert!(
            route.latency.unwrap() < Duration::from_millis(15),
            "OBU should measure fast path latency correctly (~10ms)"
        );

        // Test that hysteresis works: slightly better latency shouldn't switch
        // Create a new candidate that's only 5% better (9.5ms vs 10ms)
        let via_slightly_better: MacAddress = [30u8; 6].into();
        let mut hb_slightly_bytes = Vec::new();
        hb_slightly_bytes.extend_from_slice(&0u128.to_be_bytes());
        hb_slightly_bytes.extend_from_slice(&3u32.to_be_bytes());
        hb_slightly_bytes.extend_from_slice(&1u32.to_be_bytes()); // same hop count
        hb_slightly_bytes.extend_from_slice(&rsu.bytes());
        let hb_slightly =
            node_lib::messages::control::heartbeat::Heartbeat::try_from(&hb_slightly_bytes[..])
                .expect("hb_slightly");
        let msg_slightly = node_lib::messages::message::Message::new(
            via_slightly_better,
            [255u8; 6].into(),
            node_lib::messages::packet_type::PacketType::Control(
                node_lib::messages::control::Control::Heartbeat(hb_slightly.clone()),
            ),
        );
        let _ = obu_routing
            .handle_heartbeat(&msg_slightly, our_mac)
            .unwrap();

        // Cache the current best route first
        obu_routing.select_and_cache_upstream(rsu).expect("cached");

        // Advance only 9.5ms for slightly better latency
        tokio::time::advance(Duration::from_millis(9) + Duration::from_micros(500)).await;
        let hbr_slightly =
            node_lib::messages::control::heartbeat::HeartbeatReply::from_sender(&hb_slightly, rsu);
        let reply_slightly = node_lib::messages::message::Message::new(
            via_slightly_better,
            [255u8; 6].into(),
            node_lib::messages::packet_type::PacketType::Control(
                node_lib::messages::control::Control::HeartbeatReply(hbr_slightly.clone()),
            ),
        );
        let _ = obu_routing
            .handle_heartbeat_reply(&reply_slightly, our_mac)
            .unwrap_or(None);

        // Should keep cached route due to hysteresis (improvement < 10%)
        let hysteresis_route = obu_routing
            .get_route_to(Some(rsu))
            .expect("hysteresis route");
        assert_eq!(
            hysteresis_route.mac, via_fast,
            "Should keep cached route when improvement < 10% (hysteresis)"
        );

        // Test significant improvement: create a candidate with >10% better latency (8ms vs 10ms = 20% better)
        let via_much_better: MacAddress = [40u8; 6].into();
        let mut hb_much_bytes = Vec::new();
        hb_much_bytes.extend_from_slice(&0u128.to_be_bytes());
        hb_much_bytes.extend_from_slice(&4u32.to_be_bytes());
        hb_much_bytes.extend_from_slice(&1u32.to_be_bytes()); // same hop count
        hb_much_bytes.extend_from_slice(&rsu.bytes());
        let hb_much =
            node_lib::messages::control::heartbeat::Heartbeat::try_from(&hb_much_bytes[..])
                .expect("hb_much");
        let msg_much = node_lib::messages::message::Message::new(
            via_much_better,
            [255u8; 6].into(),
            node_lib::messages::packet_type::PacketType::Control(
                node_lib::messages::control::Control::Heartbeat(hb_much.clone()),
            ),
        );
        let _ = obu_routing.handle_heartbeat(&msg_much, our_mac).unwrap();

        // Advance 8ms for significantly better latency (>10% improvement)
        tokio::time::advance(Duration::from_millis(8)).await;
        let hbr_much =
            node_lib::messages::control::heartbeat::HeartbeatReply::from_sender(&hb_much, rsu);
        let reply_much = node_lib::messages::message::Message::new(
            via_much_better,
            [255u8; 6].into(),
            node_lib::messages::packet_type::PacketType::Control(
                node_lib::messages::control::Control::HeartbeatReply(hbr_much.clone()),
            ),
        );
        let _ = obu_routing
            .handle_heartbeat_reply(&reply_much, our_mac)
            .unwrap_or(None);

        // Should switch to much better route (>10% improvement)
        let switched_route = obu_routing.get_route_to(Some(rsu)).expect("switched route");
        assert_eq!(
            switched_route.mac, via_much_better,
            "Should switch to route with >10% latency improvement"
        );

        // Verify mocked time worked as expected (total: 10 + 30 + 9.5 + 8 = 57.5ms)
        let total_advance = Duration::from_millis(57) + Duration::from_micros(500);
        assert!(
            tokio::time::Instant::now().duration_since(tokio::time::Instant::now() - total_advance)
                >= total_advance,
            "Mocked time should have advanced correctly"
        );
    }
}

#[derive(Debug)]
#[allow(clippy::type_complexity)]
pub struct Routing {
    args: ObuArgs,
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
    // Remember the last source MAC for which we selected/cached an upstream (e.g., RSU MAC).
    cached_source: ArcSwapOption<MacAddress>,
    // Keep an ordered list of N-best candidate upstreams for fast failover.
    // This is optional and kept in sync with `cached_upstream` when selection
    // is performed.
    cached_candidates: ArcSwapOption<Vec<MacAddress>>,
    // Track distinct neighbors that forwarded heartbeats for a given source (e.g., RSU)
    source_neighbors: HashMap<MacAddress, HashSet<MacAddress>>,
}

impl Routing {
    // ...existing code...

    pub fn new(args: &ObuArgs, boot: &Instant) -> Result<Self> {
        if args.obu_params.hello_history == 0 {
            bail!("we need to be able to store at least 1 hello");
        }
        Ok(Self {
            args: args.clone(),
            boot: *boot,
            routes: HashMap::default(),
            cached_upstream: ArcSwapOption::from(None),
            cached_source: ArcSwapOption::from(None),
            cached_candidates: ArcSwapOption::from(None),
            source_neighbors: HashMap::default(),
        })
    }

    /// Return the cached upstream MAC if present.
    pub fn get_cached_upstream(&self) -> Option<MacAddress> {
        // Primary cached upstream (first candidate) -- kept for backwards compat.
        self.cached_upstream.load().as_ref().map(|m| **m)
    }

    /// Clear the cached upstream (useful when topology changes) and increment metric.
    pub fn clear_cached_upstream(&self) {
        tracing::trace!("clearing cached_upstream");
        self.cached_upstream.store(None);
        self.cached_candidates.store(None);
        #[cfg(feature = "stats")]
        crate::metrics::inc_cache_clear();
    }

    /// Return the ordered cached candidates (primary first) when present.
    pub fn get_cached_candidates(&self) -> Option<Vec<MacAddress>> {
        self.cached_candidates
            .load()
            .as_ref()
            .map(|arcv| (**arcv).clone())
    }

    /// Rotate to the next cached candidate (promote the next candidate to primary).
    /// Returns the newly promoted primary if any.
    pub fn failover_cached_upstream(&self) -> Option<MacAddress> {
        let mut cand_opt = self
            .cached_candidates
            .load()
            .as_ref()
            .map(|arcv| (**arcv).clone());

        let mut cands = cand_opt.take().unwrap_or_default();
        if cands.len() <= 1 {
            // Try to rebuild an N-best list based on the last cached source (e.g., RSU)
            if let Some(src) = self.cached_source.load().as_ref().map(|m| **m) {
                let n_best = usize::try_from(self.args.obu_params.cached_candidates)
                    .unwrap_or(3)
                    .max(1);
                // Compute latency-based candidates first
                let mut latency_candidates: HashMap<MacAddress, (u128, u128, u32, u32)> =
                    HashMap::default();
                for (_rsu, seqs) in self.routes.iter() {
                    for (_seq, (_dur, _mac, _hops, _r, downstream)) in seqs.iter() {
                        if let Some(vec) = downstream.get(&src) {
                            for route in vec.iter() {
                                if let Some(lat) = route.latency.map(|x| x.as_micros()) {
                                    let entry = latency_candidates.entry(route.mac).or_insert((
                                        u128::MAX,
                                        0u128,
                                        0u32,
                                        route.hops,
                                    ));
                                    if entry.0 > lat {
                                        entry.0 = lat;
                                    }
                                    entry.1 += lat;
                                    entry.2 += 1;
                                    entry.3 = route.hops;
                                }
                            }
                        }
                    }
                }
                if !latency_candidates.is_empty() {
                    // Use the shared helper to score and sort candidates deterministically.
                    let scored_full =
                        crate::control::routing_utils::score_and_sort_latency_candidates(
                            latency_candidates,
                        );
                    cands = scored_full
                        .into_iter()
                        .map(|(_score, _hops, mac, _avg)| mac)
                        .take(n_best)
                        .collect();
                }
                // Backfill with hop-based ordering if needed
                if cands.len() < n_best {
                    let mut upstream_routes: Vec<_> = self
                        .routes
                        .iter()
                        .flat_map(|(rsu_mac, seqs)| {
                            seqs.iter()
                                .map(move |(seq, (_, mac, hops, _, _))| (seq, rsu_mac, mac, hops))
                        })
                        .filter(|(_, rsu_mac, _, _)| rsu_mac == &&src)
                        .collect();
                    upstream_routes.sort_by(|(_, _, _, hops), (_, _, _, bhops)| hops.cmp(bhops));
                    let mut seen: std::collections::HashSet<MacAddress> =
                        cands.iter().copied().collect();
                    for (_seq, _rsu, mac_ref, _hops) in upstream_routes.into_iter() {
                        if !seen.contains(mac_ref) {
                            seen.insert(*mac_ref);
                            cands.push(*mac_ref);
                            if cands.len() >= n_best {
                                break;
                            }
                        }
                    }
                }
                // Store rebuilt candidates
                if !cands.is_empty() {
                    self.cached_candidates.store(Some(Arc::new(cands.clone())));
                }
            }
        }

        if cands.len() <= 1 {
            // Nothing to rotate to
            return cands.first().copied();
        }
        // Rotate to next
        let old = cands.remove(0);
        cands.push(old);
        self.cached_candidates.store(Some(Arc::new(cands.clone())));
        self.cached_upstream.store(Some(cands[0].into()));
        Some(cands[0])
    }

    /// Test helper: directly set cached candidates and primary for tests.
    pub fn test_set_cached_candidates(&self, cands: Vec<MacAddress>) {
        use std::sync::Arc;
        if cands.is_empty() {
            self.cached_candidates.store(None);
            self.cached_upstream.store(None);
        } else {
            self.cached_candidates.store(Some(Arc::new(cands.clone())));
            self.cached_upstream.store(Some(cands[0].into()));
        }
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
                self.args.obu_params.hello_history,
            )?));

        if entry.first().is_some_and(|(x, _)| x > &message.id()) {
            entry.clear();
        }

        if entry.len() == entry.capacity() && entry.capacity() > 0 {
            entry.swap_remove_index(0);
        }

        let seen_seq = entry.get(&message.id()).is_some();
        let duration = Instant::now().duration_since(self.boot);
        if !seen_seq {
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
        }

        let entry_from = self
            .routes
            .entry(pkt.from()?)
            .or_insert(IndexMap::with_capacity(usize::try_from(
                self.args.obu_params.hello_history,
            )?));

        if entry_from.first().is_some_and(|(x, _)| x > &message.id()) {
            entry_from.clear();
        }

        if entry_from.len() == entry_from.capacity() && entry_from.capacity() > 0 {
            entry_from.swap_remove_index(0);
        }

        // Always ensure we have an adjacency entry for the neighbor that forwarded
        // this heartbeat sequence (pkt.from). Insert if absent for this seq id.
        if !entry_from.contains_key(&message.id()) {
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
        }

        // Track that `pkt.from()` forwarded a heartbeat for `message.source()`
        self.source_neighbors
            .entry(message.source())
            .or_default()
            .insert(pkt.from()?);

        // If we've already seen this heartbeat id for the given source, we've now ensured
        // the adjacency entry for pkt.from(), but we should not forward or reply again.
        // However, refresh selection/cached candidates to incorporate the newly observed
        // neighbor in the N-best list (hysteresis preserves current primary).
        if seen_seq {
            let _ = self.select_and_cache_upstream(message.source());
            return Ok(None);
        }

        match (old_route, self.get_route_to(Some(message.source()))) {
            (None, Some(new_route)) => {
                tracing::event!(
                    Level::DEBUG,
                    from = %mac,
                    to = %message.source(),
                    through = %new_route,
                    "route created on heartbeat",
                );
                let sel = self.select_and_cache_upstream(message.source());
                tracing::trace!(selection = ?sel.as_ref().map(|r| r.mac), "heartbeat: select_and_cache_upstream");
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
        // If the target_mac is not an RSU we've recorded heartbeats for, attempt to
        // compute a route toward this node using downstream observations across all
        // heartbeat sequences. This allows forwarding downstream frames toward other
        // OBUs (e.g., two-hop paths) using observed neighbors and latencies.
        if !self.routes.contains_key(&target_mac) {
            // Collect candidate next hops that lead to target_mac along with hop-count and latency.
            let mut candidates: Vec<(u32, MacAddress, u128)> = Vec::new();
            for (_rsu, seqs) in self.routes.iter() {
                for (_seq, (_dur, _next_upstream, _hops, _r, downstream)) in seqs.iter() {
                    if let Some(vec) = downstream.get(&target_mac) {
                        for t in vec.iter() {
                            let us = t.latency.map(|d| d.as_micros()).unwrap_or(u128::MAX);
                            candidates.push((t.hops, t.mac, us));
                        }
                    }
                }
            }
            if candidates.is_empty() {
                return None;
            }
            let min_hops = candidates.iter().map(|(h, _, _)| *h).min().unwrap();
            use crate::control::routing_utils::{pick_best_next_hop, NextHopStats};

            let mut per_next: std::collections::HashMap<MacAddress, NextHopStats> =
                std::collections::HashMap::new();
            for (_h, mac, us) in candidates.into_iter().filter(|(h, _, _)| *h == min_hops) {
                let e = per_next.entry(mac).or_insert(NextHopStats {
                    min_us: u128::MAX,
                    sum_us: 0,
                    count: 0,
                });
                if us < e.min_us {
                    e.min_us = us;
                }
                if us != u128::MAX {
                    e.sum_us += us;
                    e.count += 1;
                }
            }

            let (mac, avg) = pick_best_next_hop(per_next)?;
            return Some(Route {
                hops: min_hops,
                mac,
                latency: if avg == u128::MAX {
                    None
                } else {
                    Some(Duration::from_micros(avg as u64))
                },
            });
        }
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

        // Compute deterministic integer-based metrics for latency in microseconds across ALL hops.
        // Prefer lower latency first; break ties by fewer hops.
        // Build latency_candidates deterministically by scanning all recorded sequences
        // (same approach as `select_and_cache_upstream`) to avoid timing/order issues.
        let mut latency_candidates: HashMap<MacAddress, (u128, u128, u32, u32)> =
            HashMap::default();
        for (_rsu, seqs) in self.routes.iter() {
            for (_seq, (_dur, _mac, _hops, _r, downstream)) in seqs.iter() {
                if let Some(vec) = downstream.get(&target_mac) {
                    for route in vec.iter() {
                        if let Some(lat) = route.latency.map(|x| x.as_micros()) {
                            let entry = latency_candidates.entry(route.mac).or_insert((
                                u128::MAX,
                                0u128,
                                0u32,
                                route.hops,
                            ));
                            if entry.0 > lat {
                                entry.0 = lat;
                            }
                            entry.1 += lat;
                            entry.2 += 1;
                            entry.3 = route.hops;
                        }
                    }
                }
            }
        }

        if !latency_candidates.is_empty() {
            // Use helper to pick the best candidate; clone the map so we can still
            // inspect it below for cached membership/hops.
            let (best_mac, best_avg) =
                crate::control::routing_utils::pick_best_from_latency_candidates(
                    latency_candidates.clone(),
                )
                .expect("latency_candidates non-empty");
            let (best_min, _best_sum, _best_n, best_hops) =
                latency_candidates.get(&best_mac).copied().unwrap();
            let best_score = if best_min == u128::MAX || best_avg == u128::MAX {
                u128::MAX
            } else {
                best_min + best_avg
            };

            // If cached is set but isn't in latency candidates (no latency observed yet),
            // prefer a measured candidate when available. The previous behavior kept
            // cached unless the best had at least one fewer hop; that prevented
            // switching when the new candidate had strictly better latency but the
            // cached one had no latency measurements. Here we switch to the best
            // measured candidate (when one exists). If there are no measured
            // candidates, fall back to the hops-only hysteresis.
            if let Some(cached_mac) = cached {
                if !latency_candidates.contains_key(&cached_mac) {
                    // If we have a finite scored best (i.e., measured candidate),
                    // prefer it (allow switching). Otherwise fall back to hops-only
                    // decision as before.
                    if best_score != u128::MAX {
                        // best candidate is measured; let the default return of
                        // best happen (do nothing here).
                    } else if let Some((_, _, _, cached_hops_ref)) = upstream_routes
                        .iter()
                        .find(|(_, _, mac_ref, _)| **mac_ref == cached_mac)
                    {
                        let cached_hops = **cached_hops_ref;
                        if best_mac != cached_mac {
                            let fewer_hops = best_hops < cached_hops;
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
                    tracing::debug!(
                        "cached candidate: mac={:?} min={} sum={} n={} hops={} avg={} score={}",
                        cached_mac,
                        cached_min,
                        cached_sum,
                        cached_n,
                        cached_hops,
                        cached_avg,
                        cached_score
                    );
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
                    let fewer_hops = best_hops < cached_hops;
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
                        let fewer_hops = best_hops < cached_hops; // switch only if at least one fewer hop
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

    pub fn select_and_cache_upstream(&self, mac: MacAddress) -> Option<Route> {
        let route = self.get_route_to(Some(mac))?;
        tracing::trace!(upstream = %route.mac, source = %mac, "select_and_cache_upstream selected upstream for source");
        // Store primary cached upstream as before
        self.cached_upstream.store(Some(route.mac.into()));
        // Remember the source (e.g., RSU) we selected for
        self.cached_source.store(Some(mac.into()));
        // Also attempt to populate an ordered list of N-best candidates for fast failover.
        // Use the configured value from `Args` (convert to usize), defaulting to 3 if invalid.
        let n_best = usize::try_from(self.args.obu_params.cached_candidates)
            .unwrap_or(3)
            .max(1);
        if let Some(candidates) = {
            // Re-run a variant of selection to fetch multiple candidates.
            // Call get_route_to for this mac to trigger same computation; then
            // fall back to computing from latency_candidates in the routing
            // internal structures. Because `get_route_to` is pure, we can
            // compute candidates deterministically by copying the logic here.
            // For simplicity, compute from latency and hops collected across
            // observed routes.
            // Recreate the latency_candidates map used by get_route_to.
            let mut latency_candidates: HashMap<MacAddress, (u128, u128, u32, u32)> =
                HashMap::default();
            for (_rsu, seqs) in self.routes.iter() {
                for (_seq, (_dur, _mac, _hops, _r, downstream)) in seqs.iter() {
                    if let Some(vec) = downstream.get(&mac) {
                        for route in vec.iter() {
                            if let Some(lat) = route.latency.map(|x| x.as_micros()) {
                                let entry = latency_candidates.entry(route.mac).or_insert((
                                    u128::MAX,
                                    0u128,
                                    0u32,
                                    route.hops,
                                ));
                                if entry.0 > lat {
                                    entry.0 = lat;
                                }
                                entry.1 += lat;
                                entry.2 += 1;
                                entry.3 = route.hops;
                            }
                        }
                    }
                }
            }
            if !latency_candidates.is_empty() {
                let scored_full = crate::control::routing_utils::score_and_sort_latency_candidates(
                    latency_candidates,
                );
                let mut out: Vec<MacAddress> = scored_full
                    .into_iter()
                    .map(|(_score, _hops, mac, _avg)| mac)
                    .take(n_best)
                    .collect();
                // If we still have capacity, backfill with hop-based candidates not already present
                if out.len() < n_best {
                    let mut upstream_routes: Vec<_> = self
                        .routes
                        .iter()
                        .flat_map(|(rsu_mac, seqs)| {
                            seqs.iter()
                                .map(move |(seq, (_, mac, hops, _, _))| (seq, rsu_mac, mac, hops))
                        })
                        .filter(|(_, rsu_mac, _, _)| rsu_mac == &&mac)
                        .collect();
                    upstream_routes.sort_by(|(_, _, _, hops), (_, _, _, bhops)| hops.cmp(bhops));
                    let mut seen: std::collections::HashSet<MacAddress> =
                        out.iter().copied().collect();
                    for (_seq, _rsu, mac_ref, _hops) in upstream_routes.into_iter() {
                        if !seen.contains(mac_ref) {
                            seen.insert(*mac_ref);
                            out.push(*mac_ref);
                            if out.len() >= n_best {
                                break;
                            }
                        }
                    }
                }
                // As a final fallback, add any recorded neighbors that forwarded heartbeats
                // for this source (not yet included), then, if capacity remains, include the
                // source itself.
                if out.len() < n_best {
                    if let Some(neigh) = self.source_neighbors.get(&mac) {
                        for cand in neigh.iter() {
                            if !out.contains(cand) {
                                out.push(*cand);
                                if out.len() >= n_best {
                                    break;
                                }
                            }
                        }
                    }
                }
                if out.len() < n_best && !out.contains(&mac) {
                    out.push(mac);
                }
                Some(out)
            } else {
                // Fallback: choose by fewest hops across upstream_routes
                let mut upstream_routes: Vec<_> = self
                    .routes
                    .iter()
                    .flat_map(|(rsu_mac, seqs)| {
                        seqs.iter()
                            .map(move |(seq, (_, mac, hops, _, _))| (seq, rsu_mac, mac, hops))
                    })
                    .filter(|(_, rsu_mac, _, _)| rsu_mac == &&mac)
                    .collect();
                upstream_routes.sort_by(|(_, _, _, hops), (_, _, _, bhops)| hops.cmp(bhops));
                let mut seen = std::collections::HashSet::new();
                let mut out = Vec::new();
                for (_seq, _rsu, mac_ref, _hops) in upstream_routes.into_iter() {
                    if !seen.contains(mac_ref) {
                        seen.insert(*mac_ref);
                        out.push(*mac_ref);
                        if out.len() >= n_best {
                            break;
                        }
                    }
                }
                if out.len() < n_best {
                    if let Some(neigh) = self.source_neighbors.get(&mac) {
                        for cand in neigh.iter() {
                            if !out.contains(cand) {
                                out.push(*cand);
                                if out.len() >= n_best {
                                    break;
                                }
                            }
                        }
                    }
                }
                if out.len() < n_best && !out.contains(&mac) {
                    out.push(mac);
                }
                if out.is_empty() {
                    None
                } else {
                    Some(out)
                }
            }
        } {
            // store candidates but do NOT override the already-stored primary
            // cached upstream; keep `route.mac` as the primary to preserve
            // hysteresis semantics handled by `get_route_to`.
            self.cached_candidates
                .store(Some(Arc::new(candidates.clone())));
        }
        #[cfg(feature = "stats")]
        crate::metrics::inc_cache_select();
        Some(route)
    }
}
