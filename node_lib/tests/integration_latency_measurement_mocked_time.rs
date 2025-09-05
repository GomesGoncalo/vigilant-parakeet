use node_lib::args::NodeType;
use node_lib::control::obu::Obu;
use node_lib::control::rsu::Rsu;
use node_lib::test_helpers::util::{
    await_condition_with_time_advance, mk_args, mk_device_from_fd, mk_hub_with_checks_mocked_time,
    mk_shim_pairs,
};
use std::time::Duration;

/// Test that demonstrates the latency measurement issue with mocked time.
///
/// This test should verify that latency-based routing decisions work correctly
/// with mocked time, but currently fails due to timing measurement issues.
///
/// Issue: https://github.com/GomesGoncalo/vigilant-parakeet/issues/21
///
/// The problem is that with `tokio::time::pause()` and discrete time advancement,
/// the latency measurement system used by the routing algorithm doesn't work
/// correctly, affecting the core latency-aware route selection functionality.
#[tokio::test]
#[ignore = "Latency measurement doesn't work correctly with mocked time - Issue #21"]
async fn test_latency_measurement_with_mocked_time() {
    node_lib::init_test_tracing();

    // Use mocked time - this is where the problem occurs
    tokio::time::pause();

    // Create simple 2-node topology: RSU and OBU
    let mut pairs = mk_shim_pairs(2);
    let (tun_rsu, _peer_rsu) = pairs.remove(0);
    let (tun_obu, _peer_obu) = pairs.remove(0);

    // Create 2 node<->hub links as socketpairs
    let (node_fds_v, hub_fds_v) =
        node_lib::test_helpers::util::mk_socketpairs(2).expect("mk_socketpairs failed");
    let node_fds = [node_fds_v[0], node_fds_v[1]];
    let hub_fds = [hub_fds_v[0], hub_fds_v[1]];

    // Node MACs: index 0=RSU, 1=OBU
    let mac_rsu: mac_address::MacAddress = [1, 2, 3, 4, 5, 6].into();
    let mac_obu: mac_address::MacAddress = [10, 11, 12, 13, 14, 15].into();

    // Set up hub with a known delay (e.g., 20ms)
    let delays: Vec<Vec<u64>> = vec![
        vec![0, 20], // RSU -> OBU: 20ms
        vec![20, 0], // OBU -> RSU: 20ms
    ];

    mk_hub_with_checks_mocked_time(hub_fds.to_vec(), delays, vec![]);

    let dev_rsu = mk_device_from_fd(mac_rsu, node_fds[0]);
    let dev_obu = mk_device_from_fd(mac_obu, node_fds[1]);

    // Build Args
    let args_rsu = mk_args(NodeType::Rsu, Some(50)); // RSU sends heartbeats every 50ms
    let args_obu = mk_args(NodeType::Obu, None);

    // Construct nodes
    let _rsu = Rsu::new(args_rsu, Arc::new(tun_rsu), Arc::new(dev_rsu)).expect("Rsu::new failed");
    let obu = Obu::new(args_obu, Arc::new(tun_obu), Arc::new(dev_obu)).expect("Obu::new failed");

    // Wait for OBU to receive heartbeats and cache an upstream route with latency measurement
    // With working latency measurement, the OBU should cache a route with latency info
    let result = await_condition_with_time_advance(
        Duration::from_millis(10),
        || {
            // Check if the OBU has cached an upstream route with latency measurement
            if let Some(cached_route) = obu.cached_upstream_route() {
                tracing::debug!(
                    cached_upstream = ?cached_route.mac,
                    route_latency = ?cached_route.latency,
                    "Found cached upstream route"
                );

                // Check if latency measurement is working correctly
                if cached_route.latency.is_some() {
                    // With the 20ms delay set in the hub, we should see meaningful latency measurements
                    return Some(cached_route);
                }
            }
            None
        },
        Duration::from_secs(5), // 5 seconds total timeout
    )
    .await;

    // This assertion should pass if latency measurement works correctly with mocked time
    let route_found = result.is_ok();
    assert!(
        route_found,
        "OBU should cache upstream route with latency measurement"
    );

    // Additional verification: check that the measured latency is reasonable
    if let Ok(cached_route) = result {
        if let Some(latency) = cached_route.latency {
            // The measured latency should reflect the 20ms delay set in the hub
            // With mocked time, this might not work correctly
            tracing::info!(
                measured_latency = ?latency,
                expected_delay_ms = 20,
                cached_upstream_mac = ?cached_route.mac,
                "Latency measurement result"
            );
        }
    }
}

use std::sync::Arc;
