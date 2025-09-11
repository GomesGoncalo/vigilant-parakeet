use node_lib::args::NodeType;
use node_lib::control::obu::Obu;
use node_lib::control::rsu::Rsu;
use node_lib::test_helpers::util::{
    await_condition_with_time_advance, mk_args, mk_device_from_fd, mk_shim_pair,
};
use std::sync::Arc;
use std::time::Duration;

/// Integration test: create an RSU and an OBU connected by a bidirectional
/// socketpair and check that the OBU learns the RSU as its upstream.
#[tokio::test]
#[ignore = "Test requires legacy RSU behavior - RSUs now require centralized server"]
async fn rsu_and_obu_topology_discovery() -> anyhow::Result<()> {
    // Initialize tracing for test output
    node_lib::init_test_tracing();
    // Use paused time for deterministic test execution
    tokio::time::pause();
    // Create shim tun pair using shared helper
    let (tun_a, tun_b) = mk_shim_pair();

    // Create a socketpair for bidirectional communication between devices
    let (node_fds, _hub_fds) = node_lib::test_helpers::util::mk_socketpairs(1)?;
    let mac_a: mac_address::MacAddress = [1u8, 2, 3, 4, 5, 6].into();
    let mac_b: mac_address::MacAddress = [10u8, 11, 12, 13, 14, 15].into();

    let dev_a = mk_device_from_fd(mac_a, node_fds[0]);
    let dev_b = mk_device_from_fd(mac_b, _hub_fds[0]);

    // Build Args for Rsu and Obu with hello periodicity so they exchange heartbeats
    let args_rsu = mk_args(NodeType::Rsu, Some(100));
    let args_obu = mk_args(NodeType::Obu, None);

    // Construct nodes (they spawn background tasks)
    let _rsu = Rsu::new(args_rsu, Arc::new(tun_a), Arc::new(dev_a))?;
    let obu = Obu::new(args_obu, Arc::new(tun_b), Arc::new(dev_b))?;

    // Instead of polling, await for the OBU to discover an upstream with timeout.
    // RSU sends heartbeats every 100ms, allow up to 5 seconds.
    let result = await_condition_with_time_advance(
        Duration::from_millis(10),
        || obu.cached_upstream_mac(),
        Duration::from_secs(5),
    )
    .await;

    // The OBU should have discovered the RSU as its upstream
    match result {
        Ok(mac) => {
            assert_eq!(mac, mac_a, "OBU discovered wrong upstream MAC");
        }
        Err(_) => {
            panic!("OBU did not discover an upstream within timeout");
        }
    }

    Ok(())
}
