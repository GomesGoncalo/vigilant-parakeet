//! Focused reproducer for routing loop warnings observed in the simulator logs.
//!
//! This test constructs heartbeat and heartbeat-reply messages with specific
//! MAC addresses and sequence id 0 to reproduce the two distinct loop-detection
//! cases seen in runtime logs.

use super::super::routing::Routing;
use mac_address::MacAddress;
use node_lib::messages::control::heartbeat::{Heartbeat, HeartbeatReply};
use node_lib::messages::{control::Control, message::Message, packet_type::PacketType};

use crate::test_helpers::mk_test_obu_args;
use tokio::time::Instant;

/// Reproducer: a Heartbeat is first observed via `fa_2a` (next_upstream) and a
/// HeartbeatReply reporting sender == next_upstream arrives -> should bail.
#[test]
fn reproduce_loop_case_one() {
    let args = mk_test_obu_args();
    let boot = Instant::now();
    let mut routing = Routing::new(&args, &boot).expect("routing built");

    // MACs taken from the observed logs
    let source: MacAddress = [0x2E, 0xD9, 0x12, 0x10, 0x9F, 0x47].into();
    let fa_2a: MacAddress = [0xFA, 0x2A, 0x13, 0x98, 0x32, 0xD1].into();
    let our_mac: MacAddress = [0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA].into();

    // Insert heartbeat seq=0 observed via fa_2a (this sets next_upstream = fa_2a)
    let hb = Heartbeat::new(std::time::Duration::from_millis(1), 0u32, source);
    let hb_msg = Message::new(fa_2a, [255u8; 6].into(), PacketType::Control(Control::Heartbeat(hb.clone())));

    let _ = routing.handle_heartbeat(&hb_msg, our_mac).expect("handled hb");

    // Now craft a HeartbeatReply where message.sender == next_upstream (fa_2a)
    // and pkt.from == fa_2a as in the log (should be treated as genuine loop)
    let hbr = HeartbeatReply::from_sender(&hb, fa_2a);
    let reply_msg = Message::new(fa_2a, [255u8; 6].into(), PacketType::Control(Control::HeartbeatReply(hbr.clone())));

    // Expect an error (loop detected)
    let res = routing.handle_heartbeat_reply(&reply_msg, our_mac);
    assert!(res.is_err(), "expected loop detection (case one)");
}

/// Reproducer: create a heartbeat that records next_upstream == 86:96:4D:03:16:DC,
/// then deliver a HeartbeatReply whose sender == that next_upstream but which is
/// received from a different node (A2:B9:44:12:56:2B) -> should bail.
#[test]
fn reproduce_loop_case_two() {
    let args = mk_test_obu_args();
    let boot = Instant::now();
    let mut routing = Routing::new(&args, &boot).expect("routing built");

    let source: MacAddress = [0x2E, 0xD9, 0x12, 0x10, 0x9F, 0x47].into();
    let next_up: MacAddress = [0x86, 0x96, 0x4D, 0x03, 0x16, 0xDC].into();
    let forwarder: MacAddress = [0xA2, 0xB9, 0x44, 0x12, 0x56, 0x2B].into();
    let our_mac: MacAddress = [0xBB, 0xBB, 0xBB, 0xBB, 0xBB, 0xBB].into();

    // Insert heartbeat observed via next_up (sets next_upstream = next_up)
    let hb = Heartbeat::new(std::time::Duration::from_millis(1), 0u32, source);
    let hb_msg = Message::new(next_up, [255u8; 6].into(), PacketType::Control(Control::Heartbeat(hb.clone())));
    let _ = routing.handle_heartbeat(&hb_msg, our_mac).expect("handled hb");

    // Now craft a HeartbeatReply where message.sender == next_up but pkt.from == forwarder
    let hbr = HeartbeatReply::from_sender(&hb, next_up);
    let reply_msg = Message::new(forwarder, [255u8; 6].into(), PacketType::Control(Control::HeartbeatReply(hbr.clone())));

    let res = routing.handle_heartbeat_reply(&reply_msg, our_mac);
    assert!(res.is_err(), "expected loop detection (case two)");
}
