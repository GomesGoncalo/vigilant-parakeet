//! Regression Tests
//!
//! Tests for specific bug fixes and edge cases discovered during development.

use super::super::routing::Routing;
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
    let args = crate::test_helpers::mk_test_obu_args();

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
fn heartbeat_reply_with_loop_falls_back_to_rsu_direct() {
    let args = crate::test_helpers::mk_test_obu_args();

    let boot = Instant::now();
    let mut routing = Routing::new(&args, &boot).expect("routing built");

    // Heartbeat originates from A (RSU), observed via B (pkt.from)
    let hb_source: MacAddress = [10u8; 6].into(); // A = RSU
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
    // equal to our recorded next_upstream (i.e., message.sender == next_upstream),
    // but some other node forwarded it.
    let reply_sender: MacAddress = pkt_from; // B == next_upstream (would loop)
    let hbr = HeartbeatReply::from_sender(&hb, reply_sender);
    let reply_from: MacAddress = [30u8; 6].into(); // a different relay node
    let reply_msg = Message::new(
        reply_from,
        [255u8; 6].into(),
        PacketType::Control(Control::HeartbeatReply(hbr.clone())),
    );

    // Even though next_upstream == sender, we should NOT bail: the RSU source
    // (hb_source=[10u8;6]) is different from sender and pkt_from, so we fall
    // back to forwarding directly to the RSU.
    let res = routing.handle_heartbeat_reply(&reply_msg, our_mac);
    assert!(
        res.is_ok(),
        "should forward via RSU direct instead of bailing: {res:?}"
    );
    assert!(
        res.unwrap().is_some(),
        "should produce a forwarded reply to RSU"
    );
}

#[test]
fn heartbeat_reply_bails_only_when_rsu_direct_also_loops() {
    // Bail is only reachable when message.source() (RSU MAC) is also the loop
    // partner.  Construct an artificial scenario: set the RSU source MAC equal
    // to both `sender` and `pkt_from` of the HeartbeatReply.
    let args = crate::test_helpers::mk_test_obu_args();
    let boot = Instant::now();
    let mut routing = Routing::new(&args, &boot).expect("routing built");

    // Use the same MAC for both source (RSU) and reply sender to exhaust fallbacks.
    let shared_mac: MacAddress = [20u8; 6].into(); // acts as both RSU and loop peer
    let our_mac: MacAddress = [9u8; 6].into();

    let hb = Heartbeat::new(std::time::Duration::from_millis(1), 2u32, shared_mac);
    let hb_msg = Message::new(
        shared_mac,
        [255u8; 6].into(),
        PacketType::Control(Control::Heartbeat(hb.clone())),
    );
    let _ = routing
        .handle_heartbeat(&hb_msg, our_mac)
        .expect("handled hb");

    // sender == next_upstream == pkt_from == message.source() — every fallback loops.
    let hbr = HeartbeatReply::from_sender(&hb, shared_mac);
    let reply_msg = Message::new(
        shared_mac,
        [255u8; 6].into(),
        PacketType::Control(Control::HeartbeatReply(hbr)),
    );
    let res = routing.handle_heartbeat_reply(&reply_msg, our_mac);
    assert!(res.is_err());
    assert!(format!("{}", res.unwrap_err()).contains("loop detected"));
}

// Regression test for the mutual-loop scenario described in the obu5/RSU visibility bug:
//
// When two OBUs are in range of each other and the RSU, timing can cause each to first
// receive a heartbeat seq forwarded BY the other, recording each other as next_upstream.
// Without the fix, both nodes bail ("loop detected") on each other's HeartbeatReply,
// preventing either reply from reaching the RSU.
//
// The fix: when next_upstream == sender (loop detected for this seq), fall back to the
// globally-cached upstream as an alternative forwarding path, provided it differs from
// both the sender and pkt_from (otherwise bail as before).
#[test]
fn mutual_loop_resolved_via_cached_upstream() {
    use std::time::Duration;

    let args = crate::test_helpers::mk_test_obu_args();
    let boot = Instant::now();
    let mut routing = Routing::new(&args, &boot).expect("routing built");

    let rsu_mac: MacAddress = [1u8; 6].into();
    let peer_mac: MacAddress = [2u8; 6].into();
    let our_mac: MacAddress = [9u8; 6].into();

    // Step 1: First, establish a correct route to RSU via rsu_mac itself (seq=1),
    // so the cached upstream is set to rsu_mac.
    let hb_seq1 = Heartbeat::new(Duration::from_millis(1), 1u32, rsu_mac);
    let msg_seq1 = Message::new(
        rsu_mac,
        [255u8; 6].into(),
        PacketType::Control(Control::Heartbeat(hb_seq1.clone())),
    );
    let _ = routing
        .handle_heartbeat(&msg_seq1, our_mac)
        .expect("handled seq1");
    let _ = routing.select_and_cache_upstream(rsu_mac);
    assert_eq!(
        routing.get_cached_upstream(),
        Some(rsu_mac),
        "cached upstream should be rsu_mac after seq1"
    );

    // Step 2: For seq=2, receive heartbeat from peer_mac first → next_upstream = peer_mac.
    let hb_seq2 = Heartbeat::new(Duration::from_millis(1), 2u32, rsu_mac);
    let msg_seq2_via_peer = Message::new(
        peer_mac,
        [255u8; 6].into(),
        PacketType::Control(Control::Heartbeat(hb_seq2.clone())),
    );
    let _ = routing
        .handle_heartbeat(&msg_seq2_via_peer, our_mac)
        .expect("handled seq2 via peer");

    // Step 3: HeartbeatReply for seq=2 with sender=peer_mac (== next_upstream for seq=2).
    // Old behavior: bail with "loop detected".
    // New behavior: cached upstream is rsu_mac (≠ peer_mac, ≠ pkt_from=peer_mac)
    //               → forward_cached to rsu_mac, return Ok(Some(...)).
    let hbr = HeartbeatReply::from_sender(&hb_seq2, peer_mac);
    let reply_msg = Message::new(
        peer_mac,
        [255u8; 6].into(),
        PacketType::Control(Control::HeartbeatReply(hbr)),
    );
    let res = routing.handle_heartbeat_reply(&reply_msg, our_mac);
    assert!(
        res.is_ok(),
        "should not bail when cached upstream provides a viable alternative: {res:?}"
    );
    let out = res.unwrap();
    assert!(
        out.is_some(),
        "should produce a forwarded reply via the cached upstream"
    );
}

// Regression test for the worst-case mutual-loop scenario: every recorded upstream
// (including the cached one) is the looping peer. The fallback should still succeed by
// forwarding the reply directly to the RSU source.
#[test]
fn mutual_loop_resolved_via_rsu_direct_when_all_entries_loop() {
    use std::time::Duration;

    let args = crate::test_helpers::mk_test_obu_args();
    let boot = Instant::now();
    let mut routing = Routing::new(&args, &boot).expect("routing built");

    let rsu_mac: MacAddress = [1u8; 6].into();
    let peer_mac: MacAddress = [2u8; 6].into();
    let our_mac: MacAddress = [9u8; 6].into();

    // Step 1: For seq=1, also receive heartbeat from peer_mac first so that
    // ALL recorded upstreams for rsu_mac point at peer_mac.
    let hb_seq1 = Heartbeat::new(Duration::from_millis(1), 1u32, rsu_mac);
    let msg_seq1_via_peer = Message::new(
        peer_mac,
        [255u8; 6].into(),
        PacketType::Control(Control::Heartbeat(hb_seq1.clone())),
    );
    let _ = routing
        .handle_heartbeat(&msg_seq1_via_peer, our_mac)
        .expect("handled seq1 via peer");
    // cached upstream is now peer_mac (only upstream seen so far)
    let _ = routing.select_and_cache_upstream(rsu_mac);
    assert_eq!(
        routing.get_cached_upstream(),
        Some(peer_mac),
        "cached upstream is peer_mac when peer is the only seen upstream"
    );

    // Step 2: For seq=2, also receive from peer_mac first.
    let hb_seq2 = Heartbeat::new(Duration::from_millis(1), 2u32, rsu_mac);
    let msg_seq2_via_peer = Message::new(
        peer_mac,
        [255u8; 6].into(),
        PacketType::Control(Control::Heartbeat(hb_seq2.clone())),
    );
    let _ = routing
        .handle_heartbeat(&msg_seq2_via_peer, our_mac)
        .expect("handled seq2 via peer");

    // Step 3: HeartbeatReply for seq=2 where sender=peer_mac (loop), cached=peer_mac (loops
    // too), all seq entries have next_upstream=peer_mac. Only RSU direct remains.
    let hbr = HeartbeatReply::from_sender(&hb_seq2, peer_mac);
    let reply_msg = Message::new(
        peer_mac,
        [255u8; 6].into(),
        PacketType::Control(Control::HeartbeatReply(hbr)),
    );
    let res = routing.handle_heartbeat_reply(&reply_msg, our_mac);
    assert!(
        res.is_ok(),
        "should not bail when RSU direct path is available: {res:?}"
    );
    let out = res.unwrap();
    assert!(
        out.is_some(),
        "should produce a forwarded reply via RSU direct"
    );
}

/// Regression: cached upstream must be refreshed on every new non-duplicate
/// heartbeat, not only on first route discovery.
///
/// When the primary direct path disappears and only relay heartbeats arrive as
/// new (non-duplicate) sequences, the old `old_route.is_none()` guard meant
/// `select_and_cache_upstream` was never called — leaving the cached upstream
/// pointing at the unreachable direct path indefinitely (orphan).
///
/// With hello_history=2 and the fix in place:
/// - Phase 1: 2 direct heartbeats (pkt.from=rsu1) fill the window → cached=rsu1
/// - Phase 2: 2 relay heartbeats (pkt.from=obu1, new seq IDs).
///   After both arrive, rsu1 has fully rotated out of the history window.
///   select_and_cache_upstream fires on each new heartbeat (new fix), and on
///   the second relay heartbeat upstream_hops={obu1} only — so obu1 is chosen.
#[test]
fn cached_upstream_refreshed_when_direct_path_lost() {
    use tokio::time::Duration;

    // hello_history=2 so two relay heartbeats are enough to rotate out the
    // direct-path entries.
    let args = crate::test_helpers::mk_test_obu_args();
    let boot = Instant::now();
    let mut routing = Routing::new(&args, &boot).expect("routing built");

    let rsu1: MacAddress = [0x11u8; 6].into();
    let obu1: MacAddress = [0x22u8; 6].into();
    let our_mac: MacAddress = [0x99u8; 6].into();

    // Phase 1: two direct heartbeats (pkt.from = rsu1).
    for seq in 1u32..=2 {
        let hb = Heartbeat::new(Duration::from_millis(u64::from(seq) * 1000), seq, rsu1);
        let msg = Message::new(
            rsu1,
            [0xFF; 6].into(),
            PacketType::Control(Control::Heartbeat(hb)),
        );
        routing.handle_heartbeat(&msg, our_mac).expect("hb ok");
    }
    assert_eq!(
        routing.get_cached_upstream(),
        Some(rsu1),
        "after direct-only heartbeats, upstream should be rsu1"
    );

    // Phase 2: two relay heartbeats (pkt.from = obu1, new seq IDs).
    // These are new sequences so seen_seq=false.  With the fix, each one
    // triggers select_and_cache_upstream; after the second one rsu1 has
    // rotated out of the history window entirely → obu1 is the only option.
    for seq in 3u32..=4 {
        let hb = Heartbeat::new(Duration::from_millis(u64::from(seq) * 1000), seq, rsu1);
        let msg = Message::new(
            obu1,
            [0xFF; 6].into(),
            PacketType::Control(Control::Heartbeat(hb)),
        );
        routing
            .handle_heartbeat(&msg, our_mac)
            .expect("relay hb ok");
    }

    assert_eq!(
        routing.get_cached_upstream(),
        Some(obu1),
        "after relay-only heartbeats, upstream must switch to obu1 (direct path lost)"
    );
}
