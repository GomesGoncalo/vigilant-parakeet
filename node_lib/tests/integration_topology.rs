use node_lib::args::NodeType;
use node_lib::control::obu::Obu;
use node_lib::control::rsu::Rsu;
use node_lib::test_helpers::util::{mk_args, mk_device_from_fd, mk_shim_pair};
use std::sync::Arc;
use std::time::Duration;

/// Integration test: create an RSU and an OBU connected by a bidirectional
/// socketpair and check that the OBU learns the RSU as its upstream.
#[tokio::test]
async fn rsu_and_obu_topology_discovery() {
    // Initialize tracing for test output
    node_lib::init_test_tracing();
    // Create shim tun pair using shared helper
    let (tun_a, tun_b) = mk_shim_pair();

    // Create a socketpair for bidirectional communication between devices
    let mut fds = [0; 2];
    unsafe {
        let r = libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, fds.as_mut_ptr());
        assert_eq!(r, 0, "socketpair failed");
        // make both ends non-blocking so AsyncFd readiness works and writes don't block
        let _ = libc::fcntl(fds[0], libc::F_SETFL, libc::O_NONBLOCK);
        let _ = libc::fcntl(fds[1], libc::F_SETFL, libc::O_NONBLOCK);
    }

    let mac_a: mac_address::MacAddress = [1u8, 2, 3, 4, 5, 6].into();
    let mac_b: mac_address::MacAddress = [10u8, 11, 12, 13, 14, 15].into();

    let dev_a = mk_device_from_fd(mac_a, fds[0]);
    let dev_b = mk_device_from_fd(mac_b, fds[1]);

    // Build Args for Rsu and Obu with hello periodicity so they exchange heartbeats
    let args_rsu = mk_args(NodeType::Rsu, Some(100));
    let args_obu = mk_args(NodeType::Obu, None);

    // Construct nodes (they spawn background tasks)
    let _rsu = Rsu::new(args_rsu, Arc::new(tun_a), Arc::new(dev_a)).expect("rsu new");
    let obu = Obu::new(args_obu, Arc::new(tun_b), Arc::new(dev_b)).expect("obu new");

    // Wait (poll) up to 5s for the OBU to discover an upstream route.
    // Polling reduces flakiness and prints progress for debugging.
    let mut cached = None;
    for i in 0..50 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        cached = obu.cached_upstream_mac();
        tracing::debug!(poll = i, cached_upstream = ?cached, "test poll progress");
        if cached.is_some() {
            break;
        }
    }

    // The OBU should have discovered the RSU as its upstream
    assert!(cached.is_some(), "OBU did not discover an upstream");
    assert_eq!(cached.unwrap(), mac_a);
}
