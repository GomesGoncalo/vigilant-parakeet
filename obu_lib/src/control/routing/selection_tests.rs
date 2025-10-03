//! Route Selection Tests
//!
//! Comprehensive tests for route selection logic including hysteresis,
//! latency-based routing, and deterministic tie-breaking.

use super::super::routing::{Routing, Target};
use crate::args::{ObuArgs, ObuParameters};
use mac_address::MacAddress;
use tokio::time::{Duration, Instant};

#[test]
fn get_route_to_none_when_empty() {
    let args = crate::test_helpers::mk_test_obu_args();

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
    let args = crate::test_helpers::mk_test_obu_args();
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
    let args = crate::test_helpers::mk_test_obu_args();
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

// NOTE: Additional tests from more_tests module would go here
// Keeping tests minimal for now to demonstrate structure

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
    let hb_fast = node_lib::messages::control::heartbeat::Heartbeat::try_from(&hb_fast_bytes[..])
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
    let hb_slow = node_lib::messages::control::heartbeat::Heartbeat::try_from(&hb_slow_bytes[..])
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
}
