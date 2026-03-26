use node_lib::test_helpers::hub::UpstreamExpectation;
use obu_lib::Obu;
use rsu_lib::Rsu;

use node_lib::test_helpers::util::{
    await_condition_with_time_advance, await_with_timeout, mk_device_from_fd,
    mk_hub_with_checks_mocked_time, mk_shim_pairs,
};
mod common;
use common::{mk_obu_args, mk_rsu_args};
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn rsu_and_two_obus_choose_two_hop_when_direct_has_higher_latency() {
    node_lib::init_test_tracing();
    // Use mocked time for deterministic test execution - MUST be before node creation
    tokio::time::pause();

    // Create 3 shim TUN pairs and keep the peer for OBU2
    let mut pairs = mk_shim_pairs(3);
    let (_tun_rsu, _peer0) = pairs.remove(0);
    let (tun_obu1, _peer1) = pairs.remove(0);
    let (tun_obu2, tun_obu2_peer) = pairs.remove(0);

    // Create 3 node<->hub links as socketpairs: (node_fd[i], hub_fd[i])
    let (node_fds_v, hub_fds_v) =
        node_lib::test_helpers::util::mk_socketpairs(3).expect("mk_socketpairs failed");
    let node_fds = [node_fds_v[0], node_fds_v[1], node_fds_v[2]];
    let hub_fds = [hub_fds_v[0], hub_fds_v[1], hub_fds_v[2]];

    // Wrap node ends as Devices
    let mac_rsu: mac_address::MacAddress = [1, 2, 3, 4, 5, 6].into();
    let mac_obu1: mac_address::MacAddress = [10, 11, 12, 13, 14, 15].into();
    let mac_obu2: mac_address::MacAddress = [20, 21, 22, 23, 24, 25].into();

    // Spawn the hub with delay matrix: index 0=RSU, 1=OBU1, 2=OBU2
    // Make direct path RSU->OBU2 high latency (50ms), RSU<->OBU1 and OBU1<->OBU2 low (2ms)
    let delays: Vec<Vec<u64>> = vec![vec![0, 2, 50], vec![2, 0, 2], vec![50, 2, 0]];
    // Payload we'll inject later; verify via hub expectation as well
    let payload: &[u8] = b"test payload";

    // Create a future-based expectation instead of atomic flag
    let (upstream_expectation, upstream_future) =
        UpstreamExpectation::new(2, mac_obu2, mac_obu1, Some(payload.to_vec()));

    mk_hub_with_checks_mocked_time(
        hub_fds.to_vec(),
        delays,
        vec![Arc::new(upstream_expectation)],
    );

    let dev_rsu = mk_device_from_fd(mac_rsu, node_fds[0]);
    let dev_obu1 = mk_device_from_fd(mac_obu1, node_fds[1]);
    let dev_obu2 = mk_device_from_fd(mac_obu2, node_fds[2]);

    // Build Args
    let args_rsu = mk_rsu_args(50);
    let args_obu1 = mk_obu_args();
    let args_obu2 = mk_obu_args();

    // Construct nodes (RSU no longer takes a TUN device)
    let _rsu =
        Rsu::new(args_rsu, Arc::new(dev_rsu), "test_rsu".to_string()).expect("Rsu::new failed");
    let _obu1 = Obu::new(
        args_obu1,
        Arc::new(tun_obu1),
        Arc::new(dev_obu1),
        "test_obu1".to_string(),
    )
    .expect("Obu::new failed");
    let tun_obu2_arc = Arc::new(tun_obu2);
    let obu2 = Obu::new(
        args_obu2,
        tun_obu2_arc,
        Arc::new(dev_obu2),
        "test_obu2".to_string(),
    )
    .expect("Obu::new failed");

    // Wait for OBU2 to cache upstream route using await/timeout pattern
    // RSU sends heartbeats every 50ms, allow up to 10 seconds
    let result = await_condition_with_time_advance(
        Duration::from_millis(10),
        || {
            if let Some(mac) = obu2.cached_upstream_mac() {
                if mac == mac_obu1 {
                    return Some(mac);
                }
            }
            None
        },
        Duration::from_secs(10),
    )
    .await;

    let cached = match result {
        Ok(mac) => Some(mac),
        Err(_) => panic!("OBU2 did not cache upstream within timeout"),
    };

    assert!(cached.is_some(), "OBU2 did not cache an upstream");
    assert_eq!(
        cached,
        Some(mac_obu1),
        "OBU2 should prefer two-hop path via OBU1"
    );

    // Trigger an upstream send by writing on the peer end of OBU2's TUN; the session task should forward it.
    tun_obu2_peer
        .send_all(payload)
        .await
        .expect("tun_obu2_peer.send_all failed");

    // Wait for the hub to observe the expected upstream packet using future-based expectation
    let _ = await_with_timeout(upstream_future, Duration::from_secs(2))
        .await
        .expect("Hub did not observe expected upstream packet within timeout");
}

/// Integration test: verify OBU2 establishes a two-hop route via OBU1 to the RSU,
/// and can send upstream data that reaches the RSU's VANET device.
/// Note: RSU no longer has a TAP device; data is forwarded to the server via cloud.
/// This test verifies routing/heartbeat discovery still works correctly.
#[tokio::test]
async fn two_hop_route_discovery_and_upstream() {
    node_lib::init_test_tracing();

    // Use mocked time for deterministic test execution - MUST be before node creation
    tokio::time::pause();

    // Create shim TUN pairs (RSU doesn't need one, but OBUs do)
    let mut pairs = mk_shim_pairs(3);
    let (_tun_rsu_unused, _tun_rsu_peer) = pairs.remove(0);
    let (tun_obu1, _tun_obu1_peer) = pairs.remove(0);
    let (tun_obu2, tun_obu2_peer) = pairs.remove(0);

    // Create 3 node<->hub links as socketpairs: (node_fd[i], hub_fd[i])
    let (node_fds_v, hub_fds_v) =
        node_lib::test_helpers::util::mk_socketpairs(3).expect("mk_socketpairs failed");
    let node_fds = [node_fds_v[0], node_fds_v[1], node_fds_v[2]];
    let hub_fds = [hub_fds_v[0], hub_fds_v[1], hub_fds_v[2]];

    // Node MACs: index 0=RSU, 1=OBU1, 2=OBU2
    let mac_rsu: mac_address::MacAddress = [1, 2, 3, 4, 5, 6].into();
    let mac_obu1: mac_address::MacAddress = [10, 11, 12, 13, 14, 15].into();
    let mac_obu2: mac_address::MacAddress = [20, 21, 22, 23, 24, 25].into();

    // Hub delays: prefer RSU<->OBU1 and OBU1<->OBU2 (2ms) over direct RSU<->OBU2 (50ms).
    let delays: Vec<Vec<u64>> = vec![vec![0, 2, 50], vec![2, 0, 2], vec![50, 2, 0]];

    // Create a future-based expectation for upstream from OBU2
    let (upstream_expectation, upstream_future) =
        UpstreamExpectation::new(2, mac_obu2, mac_obu1, None);

    mk_hub_with_checks_mocked_time(
        hub_fds.to_vec(),
        delays,
        vec![Arc::new(upstream_expectation)],
    );

    // Wrap node ends as Devices using shared helper
    let dev_rsu = mk_device_from_fd(mac_rsu, node_fds[0]);
    let dev_obu1 = mk_device_from_fd(mac_obu1, node_fds[1]);
    let dev_obu2 = mk_device_from_fd(mac_obu2, node_fds[2]);

    // Build Args using shared helper
    let args_rsu = mk_rsu_args(50);
    let args_obu = mk_obu_args();

    // Construct nodes (RSU no longer takes a TUN device)
    let _rsu =
        Rsu::new(args_rsu, Arc::new(dev_rsu), "test_rsu2".to_string()).expect("Rsu::new failed");
    let _obu1 = Obu::new(
        args_obu.clone(),
        Arc::new(tun_obu1),
        Arc::new(dev_obu1),
        "test_obu1_2".to_string(),
    )
    .expect("Obu::new failed");
    let obu2 = Obu::new(
        args_obu,
        Arc::new(tun_obu2),
        Arc::new(dev_obu2),
        "test_obu2_2".to_string(),
    )
    .expect("Obu::new failed");

    // Wait for OBU2 to cache upstream via OBU1 (two-hop path preferred)
    let result = await_condition_with_time_advance(
        Duration::from_millis(10),
        || {
            if let Some(mac) = obu2.cached_upstream_mac() {
                if mac == mac_obu1 {
                    return Some(mac);
                }
            }
            None
        },
        Duration::from_secs(20),
    )
    .await;

    let cached = match result {
        Ok(mac) => Some(mac),
        Err(_) => panic!("OBU2 did not cache upstream within timeout"),
    };

    assert!(cached.is_some(), "OBU2 did not cache an upstream");
    assert_eq!(
        cached.unwrap(),
        mac_obu1,
        "OBU2 should pick OBU1 as upstream"
    );

    // Send upstream data from OBU2 via its TUN
    let payload = b"test payload";
    tun_obu2_peer
        .send_all(payload)
        .await
        .expect("tun_obu2_peer.send_all failed");

    // Wait for the hub to observe the expected upstream packet
    let _ = await_with_timeout(upstream_future, Duration::from_secs(2))
        .await
        .expect("Hub did not observe expected upstream packet within timeout");
}
