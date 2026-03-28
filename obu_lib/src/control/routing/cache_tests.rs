//! Cache Management Tests
//!
//! Tests for upstream caching and failover functionality.

use super::super::routing::Routing;
use mac_address::MacAddress;
use tokio::time::{Duration, Instant};

#[test]
fn select_and_cache_upstream_sets_cache() {
    let args = crate::test_helpers::mk_test_obu_args();

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
    let args = crate::test_helpers::mk_test_obu_args();

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
    let primary_before = routing.get_cached_upstream().expect("primary");
    let next_candidate: MacAddress = [11u8; 6].into();
    // store ordered candidates [primary, next]
    routing
        .cache
        .set_candidates(vec![primary_before, next_candidate]);

    // Simulate a send failure by directly calling failover_cached_upstream()
    let promoted = routing.failover_cached_upstream();
    assert!(promoted.is_some());
    let primary_after = routing.get_cached_upstream().expect("primary after");
    assert_ne!(primary_before, primary_after);
    assert_eq!(primary_after, promoted.unwrap());
}

// Regression test: multi-hop OBU that only receives RSU heartbeats via a relay
// (never directly) must still be able to route data packets.
// This mirrors obu5's topology (obu5→obu1→RSU, RSU→obu5 has packet loss and 15ms delay
// so obu1's 1ms relay always arrives first).
#[test]
fn multihop_obu_get_route_to_none_returns_relay() {
    use node_lib::messages::control::{heartbeat::Heartbeat, Control};
    use node_lib::messages::{message::Message, packet_type::PacketType};

    let args = crate::test_helpers::mk_test_obu_args();
    let boot = Instant::now() - Duration::from_secs(1);
    let mut routing = Routing::new(&args, &boot).expect("routing built");

    let rsu_mac: MacAddress = [1u8; 6].into();
    let relay_mac: MacAddress = [10u8; 6].into(); // e.g. obu1
    let our_mac: MacAddress = [9u8; 6].into(); // e.g. obu5

    // obu5 only ever sees RSU heartbeats via obu1 (relay), never directly.
    let hb = Heartbeat::new(Duration::from_millis(1), 1u32, rsu_mac);
    let hb_msg = Message::new(
        relay_mac,
        [255u8; 6].into(),
        PacketType::Control(Control::Heartbeat(hb)),
    );
    let _ = routing.handle_heartbeat(&hb_msg, our_mac).expect("handled");

    // get_route_to(None) must return Some(Route{mac=relay_mac}) so that
    // obu5 can actually forward data packets via relay_mac toward RSU.
    let route = routing.get_route_to(None);
    assert!(
        route.is_some(),
        "multi-hop OBU must have a route for data forwarding, got None"
    );
    assert_eq!(
        route.unwrap().mac,
        relay_mac,
        "route next-hop must be the relay OBU, not None"
    );
}
