use common::device::Device;
use common::device::DeviceIo;
use common::tun::test_tun::TokioTun;
use common::tun::Tun;
use node_lib::args::{Args, NodeParameters, NodeType};
use node_lib::control::obu::Obu;
use node_lib::control::rsu::Rsu;
use std::os::unix::io::FromRawFd;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::unix::AsyncFd;

/// Integration test: create an RSU and an OBU connected by a bidirectional
/// socketpair and check that the OBU learns the RSU as its upstream.
#[tokio::test]
async fn rsu_and_obu_topology_discovery() {
    // Initialize tracing for test output
    node_lib::init_test_tracing();
    // Create shim tun pair and wrap in common::tun::Tun
    let (tun_a_shim, tun_b_shim) = TokioTun::new_pair();
    let tun_a = Tun::new_shim(tun_a_shim);
    let tun_b = Tun::new_shim(tun_b_shim);

    // Create a socketpair for bidirectional communication between devices
    let mut fds = [0; 2];
    unsafe {
        let r = libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, fds.as_mut_ptr());
        assert_eq!(r, 0, "socketpair failed");
        // make both ends non-blocking so AsyncFd readiness works and writes don't block
        let _ = libc::fcntl(fds[0], libc::F_SETFL, libc::O_NONBLOCK);
        let _ = libc::fcntl(fds[1], libc::F_SETFL, libc::O_NONBLOCK);
    }

    // Wrap each fd in an AsyncFd<DeviceIo> and create Devices with distinct MACs
    let async_a = AsyncFd::new(unsafe { DeviceIo::from_raw_fd(fds[0]) }).unwrap();
    let async_b = AsyncFd::new(unsafe { DeviceIo::from_raw_fd(fds[1]) }).unwrap();

    let mac_a: mac_address::MacAddress = [1u8, 2, 3, 4, 5, 6].into();
    let mac_b: mac_address::MacAddress = [10u8, 11, 12, 13, 14, 15].into();

    let dev_a = Device::from_asyncfd_for_bench(mac_a, async_a);
    let dev_b = Device::from_asyncfd_for_bench(mac_b, async_b);

    // Build Args for Rsu and Obu with hello periodicity so they exchange heartbeats
    let node_params_rsu = NodeParameters {
        node_type: NodeType::Rsu,
        hello_history: 10,
        hello_periodicity: Some(100),
        cached_candidates: 3,
    };
    let node_params_obu = NodeParameters {
        node_type: NodeType::Obu,
        hello_history: 10,
        hello_periodicity: None,
        cached_candidates: 3,
    };

    let args_rsu = Args {
        bind: String::from("unused"),
        tap_name: None,
        ip: None,
        mtu: 1500,
        node_params: node_params_rsu,
    };
    let args_obu = Args {
        bind: String::from("unused"),
        tap_name: None,
        ip: None,
        mtu: 1500,
        node_params: node_params_obu,
    };

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
