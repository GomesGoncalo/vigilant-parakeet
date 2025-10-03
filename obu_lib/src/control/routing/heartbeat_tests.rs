//! Heartbeat Processing Tests
//!
//! Tests for heartbeat and heartbeat reply message handling.

use super::super::routing::Routing;
use mac_address::MacAddress;
use node_lib::messages::{
    control::heartbeat::Heartbeat, control::heartbeat::HeartbeatReply, control::Control,
    message::Message, packet_type::PacketType,
};
use tokio::time::{Duration, Instant};

#[test]
fn handle_heartbeat_creates_route_and_returns_replies() {
    let args = crate::test_helpers::mk_test_obu_args();

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
    let args = crate::test_helpers::mk_test_obu_args();

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
