//! Regression Tests
//!
//! Tests for specific bug fixes and edge cases discovered during development.

use super::super::routing::Routing;
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
    assert!(format!("{}", err).contains("loop detected"));
}
