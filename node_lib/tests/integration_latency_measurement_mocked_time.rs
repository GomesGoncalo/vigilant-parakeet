use node_lib::test_helpers::util::{
    await_condition_with_time_advance, mk_device_from_fd, mk_hub_with_checks_mocked_time,
    mk_shim_pairs,
};
use obu_lib::Obu;
use rsu_lib::Rsu;
mod common;
use common::{mk_obu_args, mk_rsu_args};
use std::time::Duration;

/// Test that demonstrates latency measurement with mocked time in a realistic scenario.
///
/// This test verifies that the latency measurement infrastructure works correctly with mocked time.
/// It focuses on the RSU side latency measurement which is the primary use case for the heartbeat
/// reply protocol.
///
/// The test validates:
/// 1. Multi-message packet parsing works with mocked time
/// 2. RSU can measure latency from heartbeat replies  
/// 3. The latency measurement protocol is compatible with mocked time infrastructure
///
/// Network topology: RSU + OBU with known network delays
#[tokio::test]
async fn test_latency_measurement_with_mocked_time() {
    node_lib::init_test_tracing();

    // Use mocked time - this should now work with the multi-message parsing fix
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

    // Set up hub with known delays to test latency measurement infrastructure
    let delays: Vec<Vec<u64>> = vec![
        vec![0, 20], // RSU → OBU: 20ms
        vec![20, 0], // OBU → RSU: 20ms
    ];

    mk_hub_with_checks_mocked_time(hub_fds.to_vec(), delays, vec![]);

    let dev_rsu = mk_device_from_fd(mac_rsu, node_fds[0]);
    let dev_obu = mk_device_from_fd(mac_obu, node_fds[1]);

    // Build Args - RSU sends heartbeats every 50ms
    let args_rsu = mk_rsu_args(50);
    let args_obu = mk_obu_args();

    // Construct nodes
    let rsu = Rsu::new(args_rsu, Arc::new(tun_rsu), Arc::new(dev_rsu), "test_rsu".to_string()).expect("Rsu::new failed");
    let obu = Obu::new(args_obu, Arc::new(tun_obu), Arc::new(dev_obu), "test_obu".to_string()).expect("Obu::new failed");

    // Wait for the RSU to measure latency from heartbeat replies
    // This tests the core latency measurement infrastructure with mocked time
    let result = await_condition_with_time_advance(
        Duration::from_millis(30), // Time step for advancing mocked time
        || {
            // Check if OBU has established basic connectivity (route discovery works)
            if let Some(obu_route) = obu.cached_upstream_route() {
                tracing::debug!(
                    obu_upstream = ?obu_route.mac,
                    obu_hops = obu_route.hops,
                    "OBU found upstream route"
                );

                // Primary validation: Check if RSU has measured latency to the OBU
                // This validates that the heartbeat reply protocol and multi-message
                // parsing work correctly with mocked time
                if let Some(rsu_route) = rsu.get_route_to(mac_obu) {
                    tracing::debug!(
                        rsu_to_obu_latency = ?rsu_route.latency,
                        rsu_to_obu_hops = rsu_route.hops,
                        "RSU route to OBU"
                    );

                    if rsu_route.latency.is_some() {
                        let latency = rsu_route.latency.unwrap();
                        tracing::info!(
                            measured_latency = ?latency,
                            "SUCCESS: RSU measured latency with mocked time"
                        );
                        return Some((obu_route, rsu_route));
                    }
                }
            }
            None
        },
        Duration::from_secs(10), // Allow time for heartbeat cycles and latency measurement
    )
    .await;

    // This assertion validates that latency measurement works with mocked time
    let measurement_successful = result.is_ok();
    assert!(
        measurement_successful,
        "RSU should measure latency from heartbeat replies with mocked time"
    );

    // Additional validation: verify the measured latency is reasonable
    if let Ok((obu_route, rsu_route)) = result {
        if let Some(latency) = rsu_route.latency {
            tracing::info!(
                final_rsu_latency = ?latency,
                expected_delay_ms = 20,
                obu_route_hops = obu_route.hops,
                "Latency measurement validation"
            );

            // The latency should be positive and include the network delays
            assert!(
                latency.as_millis() > 0,
                "Measured latency should be positive"
            );

            // With 20ms each way + protocol overhead, expect reasonable latency
            assert!(
                latency.as_millis() < 1000,
                "Measured latency should be reasonable (<1s)"
            );
        }
    }
}

use std::sync::Arc;
