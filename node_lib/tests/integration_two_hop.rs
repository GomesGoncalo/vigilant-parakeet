use node_lib::args::NodeType;
use node_lib::control::obu::Obu;
use node_lib::control::rsu::Rsu;
use node_lib::test_helpers::hub::{DownstreamFromIdxCheck, UpstreamMatchCheck};
use node_lib::test_helpers::util::{mk_args, mk_device_from_fd, mk_shim_pairs, mk_hub_with_checks};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

#[tokio::test]
async fn rsu_and_two_obus_choose_two_hop_when_direct_has_higher_latency() {
    node_lib::init_test_tracing();
    // Use mocked time for deterministic test execution - MUST be before node creation
    // tokio::time::pause();

    // Create 3 shim TUN pairs and keep the peer for OBU2
    let mut pairs = mk_shim_pairs(3);
    let (tun_rsu, _peer0) = pairs.remove(0);
    let (tun_obu1, _peer1) = pairs.remove(0);
    let (tun_obu2, tun_obu2_peer) = pairs.remove(0);

    // Create 3 node<->hub links as socketpairs: (node_fd[i], hub_fd[i])
    let (node_fds_v, hub_fds_v) = node_lib::test_helpers::util::mk_socketpairs(3).expect("mk_socketpairs failed");
    let node_fds = [node_fds_v[0], node_fds_v[1], node_fds_v[2]];
    let hub_fds = [hub_fds_v[0], hub_fds_v[1], hub_fds_v[2]];

    // Wrap node ends as Devices
    let mac_rsu: mac_address::MacAddress = [1, 2, 3, 4, 5, 6].into();
    let mac_obu1: mac_address::MacAddress = [10, 11, 12, 13, 14, 15].into();
    let mac_obu2: mac_address::MacAddress = [20, 21, 22, 23, 24, 25].into();

    // Spawn the hub with delay matrix: index 0=RSU, 1=OBU1, 2=OBU2
    // Make direct path RSU->OBU2 high latency (50ms), RSU<->OBU1 and OBU1<->OBU2 low (2ms)
    let delays: Vec<Vec<u64>> = vec![vec![0, 2, 50], vec![2, 0, 2], vec![50, 2, 0]];
    let saw_forward_to_obu1 = Arc::new(AtomicBool::new(false));
    // Payload we'll inject later; verify via hub check as well
    let payload: &[u8] = b"test payload";

    mk_hub_with_checks(
        hub_fds.to_vec(),
        delays,
        vec![Arc::new(UpstreamMatchCheck {
            idx: 2,
            from: mac_obu2,
            to: mac_obu1,
            expected_payload: Some(payload.to_vec()),
            flag: saw_forward_to_obu1.clone(),
        }) as Arc<dyn node_lib::test_helpers::hub::HubCheck>],
    );

    let dev_rsu = mk_device_from_fd(mac_rsu, node_fds[0]);
    let dev_obu1 = mk_device_from_fd(mac_obu1, node_fds[1]);
    let dev_obu2 = mk_device_from_fd(mac_obu2, node_fds[2]);

    // Build Args
    let args_rsu = mk_args(NodeType::Rsu, Some(50));
    let args_obu1 = mk_args(NodeType::Obu, None);
    let args_obu2 = mk_args(NodeType::Obu, None);

    // Construct nodes
    let _rsu = Rsu::new(args_rsu, Arc::new(tun_rsu), Arc::new(dev_rsu)).expect("Rsu::new failed");
    let _obu1 = Obu::new(args_obu1, Arc::new(tun_obu1), Arc::new(dev_obu1)).expect("Obu::new failed");
    let tun_obu2_arc = Arc::new(tun_obu2);
    let obu2 = Obu::new(args_obu2, tun_obu2_arc, Arc::new(dev_obu2)).expect("Obu::new failed");

    // Wait for OBU2 to cache upstream route using real time
    // RSU sends heartbeats every 50ms, so check every 100ms
    let mut cached = None;
    for i in 0..100 { // up to 10s worth 
        tokio::time::sleep(Duration::from_millis(100)).await;
        cached = obu2.cached_upstream_mac();
        tracing::debug!(poll = i, cached_upstream = ?cached, "polling for upstream selection");
        if cached == Some(mac_obu1) {
            break;
        }
    }
    assert!(cached.is_some(), "OBU2 did not cache an upstream");
    assert_eq!(
        cached,
        Some(mac_obu1),
        "OBU2 should prefer two-hop path via OBU1"
    );

    // Trigger an upstream send by writing on the peer end of OBU2's TUN; the session task should forward it.
    tun_obu2_peer.send_all(payload).await.expect("tun_obu2_peer.send_all failed");

    // Wait up to equivalent time for the hub to observe the upstream packet
    for _i in 0..40 { // equivalent to ~2s
        tokio::time::advance(Duration::from_millis(50)).await;
        if saw_forward_to_obu1.load(Ordering::SeqCst) {
            break;
        }
    }
    // This assertion is soft; the primary assertion is the cached upstream.
    assert!(saw_forward_to_obu1.load(Ordering::SeqCst));
}

/// End-to-end: OBU2 "pings" RSU two hops away. We inject a request frame into
/// OBU2's TUN (dest=RSU MAC, src=OBU2 MAC, payload=bytes) and expect it to reach
/// RSU's TUN. Then we inject a reply from RSU's TUN (dest=OBU2 MAC, src=RSU MAC)
/// and expect OBU2's TUN to receive the reply payload. This verifies both
/// directions succeed across the two-hop route selection.
#[tokio::test]
async fn two_hop_ping_roundtrip_obu2_to_rsu() {
    node_lib::init_test_tracing();

    // Use mocked time for deterministic test execution - MUST be before node creation
    // tokio::time::pause();

    // Create shim TUN pairs and keep peers for RSU and OBU2
    let mut pairs = mk_shim_pairs(3);
    let (tun_rsu, tun_rsu_peer) = pairs.remove(0);
    let (tun_obu1, _tun_obu1_peer) = pairs.remove(0);
    let (tun_obu2, tun_obu2_peer) = pairs.remove(0);

    // Create 3 node<->hub links as socketpairs: (node_fd[i], hub_fd[i])
    let (node_fds_v, hub_fds_v) = node_lib::test_helpers::util::mk_socketpairs(3).expect("mk_socketpairs failed");
    let node_fds = [node_fds_v[0], node_fds_v[1], node_fds_v[2]];
    let hub_fds = [hub_fds_v[0], hub_fds_v[1], hub_fds_v[2]];

    // Node MACs: index 0=RSU, 1=OBU1, 2=OBU2
    let mac_rsu: mac_address::MacAddress = [1, 2, 3, 4, 5, 6].into();
    let mac_obu1: mac_address::MacAddress = [10, 11, 12, 13, 14, 15].into();
    let mac_obu2: mac_address::MacAddress = [20, 21, 22, 23, 24, 25].into();

    // Hub delays: prefer RSU<->OBU1 and OBU1<->OBU2 (2ms) over direct RSU<->OBU2 (50ms).
    let delays: Vec<Vec<u64>> = vec![vec![0, 2, 50], vec![2, 0, 2], vec![50, 2, 0]];
    let saw_downstream_from_rsu = Arc::new(AtomicBool::new(false));

    mk_hub_with_checks(
        hub_fds.to_vec(),
        delays,
        vec![Arc::new(DownstreamFromIdxCheck {
            idx: 0,
            flag: saw_downstream_from_rsu.clone(),
        }) as Arc<dyn node_lib::test_helpers::hub::HubCheck>],
    );

    // Wrap node ends as Devices using shared helper
    let dev_rsu = mk_device_from_fd(mac_rsu, node_fds[0]);
    let dev_obu1 = mk_device_from_fd(mac_obu1, node_fds[1]);
    let dev_obu2 = mk_device_from_fd(mac_obu2, node_fds[2]);

    // Build Args using shared helper
    let args_rsu = mk_args(NodeType::Rsu, Some(50));
    let args_obu = mk_args(NodeType::Obu, None);

    // Construct nodes
    let _rsu = Rsu::new(args_rsu, Arc::new(tun_rsu), Arc::new(dev_rsu)).expect("Rsu::new failed");
    let _obu1 = Obu::new(args_obu.clone(), Arc::new(tun_obu1), Arc::new(dev_obu1)).expect("Obu::new failed");
    let obu2 = Obu::new(args_obu, Arc::new(tun_obu2), Arc::new(dev_obu2)).expect("Obu::new failed");

    // Wait for OBU2 to cache upstream via OBU1 (two-hop path preferred)
    let mut cached = None;
    for i in 0..200 { // equivalent to 20s
        tokio::time::sleep(Duration::from_millis(100)).await;
        cached = obu2.cached_upstream_mac();
        tracing::debug!(poll = i, cached_upstream = ?cached, "polling for upstream selection");
        if cached == Some(mac_obu1) {
            break;
        }
    }
    assert!(cached.is_some(), "OBU2 did not cache an upstream");
    assert_eq!(
        cached.unwrap(),
        mac_obu1,
        "OBU2 should pick OBU1 as upstream"
    );

    // Prime RSU's client cache with a mapping for RSU's own MAC -> RSU node MAC
    // by sending any frame from RSU's TUN (process_tap_traffic stores `from` -> device.mac).
    let mut prime = Vec::new();
    prime.extend_from_slice(&[255u8; 6]); // dest broadcast
    prime.extend_from_slice(&mac_rsu.bytes()); // from = RSU
    prime.extend_from_slice(b"prime");
    tun_rsu_peer.send_all(&prime).await.expect("tun_rsu_peer.send_all failed");
    // Give a moment for RSU to process and store mapping
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Compose a "ping" request frame from OBU2 destined to RSU
    let payload_req = b"ping-req";
    let mut req = Vec::new();
    req.extend_from_slice(&mac_rsu.bytes()); // to
    req.extend_from_slice(&mac_obu2.bytes()); // from
    req.extend_from_slice(payload_req); // body

    // Send request into OBU2's TUN (session will forward upstream over two hops)
    tun_obu2_peer.send_all(&req).await.expect("tun_obu2_peer.send_all failed");

    // Expect RSU's TUN to receive the full upstream request frame (to+from+payload)
    let got_req_at_rsu =
        node_lib::test_helpers::util::poll_tun_recv_expected(&tun_rsu_peer, &req, 100, 100).await;
    assert!(got_req_at_rsu, "RSU did not receive ping request on TUN");

    // Give RSU additional time to ensure it has a route to OBU2
    tokio::time::sleep(Duration::from_millis(1500)).await;

    // Now craft and send a reply from RSU back to OBU2 via RSU's TUN
    let payload_rep = b"ping-rep";
    let mut rep = Vec::new();
    rep.extend_from_slice(&mac_obu2.bytes()); // to
    rep.extend_from_slice(&mac_rsu.bytes()); // from
    rep.extend_from_slice(payload_rep);
    tun_rsu_peer.send_all(&rep).await.expect("tun_rsu_peer.send_all failed");

    // Wait for hub to observe a Downstream packet from RSU
    for _ in 0..50 {
        if saw_downstream_from_rsu.load(Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    println!(
        "hub saw downstream from RSU: {}",
        saw_downstream_from_rsu.load(Ordering::SeqCst)
    );

    // Wait for the hub to observe a Downstream frame emitted from RSU before expecting OBU2's TUN
    for _ in 0..50 {
        if saw_downstream_from_rsu.load(Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Expect OBU2's TUN to receive the full downstream reply frame (to+from+payload)
    let got_rep_at_obu2 =
        node_lib::test_helpers::util::poll_tun_recv_expected(&tun_obu2_peer, &rep, 100, 150).await;
    assert!(got_rep_at_obu2, "OBU2 did not receive ping reply on TUN");
}
