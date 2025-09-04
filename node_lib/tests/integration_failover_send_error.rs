use common::device::{Device, DeviceIo};
use common::tun::test_tun::TokioTun;
use common::tun::Tun;
use node_lib::args::{Args, NodeParameters, NodeType};
use node_lib::control::obu::Obu;
use node_lib::control::rsu::Rsu;
use node_lib::test_helpers::hub::{Hub, UpstreamMatchCheck};
use std::os::unix::io::FromRawFd;
use std::sync::{atomic::AtomicBool, Arc};
use std::time::Duration;
use tokio::io::unix::AsyncFd;

/// Integration test: build RSU, OBU1, OBU2 connected by a hub. OBU2 should
/// prefer OBU1 as upstream (two-hop) given the delay matrix. Then close OBU1's
/// hub endpoint to simulate a send failure and verify OBU2 promotes to another
/// candidate.
#[tokio::test]
async fn obu_promotes_on_primary_send_failure_via_hub_closure() {
    node_lib::init_test_tracing();

    // Create 3 TUNs (one per node). Keep OBU2's peer so we can inject upstream traffic reliably.
    let (tun_rsu_a, _) = TokioTun::new_pair();
    let (tun_obu1_a, _) = TokioTun::new_pair();
    let (tun_obu2_a, tun_obu2_b) = TokioTun::new_pair();
    let tun_rsu = Tun::new_shim(tun_rsu_a);
    let tun_obu1 = Tun::new_shim(tun_obu1_a);
    let tun_obu2 = Tun::new_shim(tun_obu2_a);

    // Create 3 node<->hub links as socketpairs: (node_fd[i], hub_fd[i])
    let mut node_fds = [0; 3];
    let mut hub_fds = [0; 3];
    for i in 0..3 {
        let mut fds = [0; 2];
        unsafe {
            let r = libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, fds.as_mut_ptr());
            assert_eq!(r, 0, "socketpair failed");
            let _ = libc::fcntl(fds[0], libc::F_SETFL, libc::O_NONBLOCK);
            let _ = libc::fcntl(fds[1], libc::F_SETFL, libc::O_NONBLOCK);
        }
        node_fds[i] = fds[0];
        hub_fds[i] = fds[1];
    }

    // Node MACs: index 0=RSU, 1=OBU1, 2=OBU2
    let mac_rsu: mac_address::MacAddress = [1, 2, 3, 4, 5, 6].into();
    let mac_obu1: mac_address::MacAddress = [10, 11, 12, 13, 14, 15].into();
    let mac_obu2: mac_address::MacAddress = [20, 21, 22, 23, 24, 25].into();

    // Hub delays: prefer RSU<->OBU1 and OBU1<->OBU2 (2ms) over direct RSU<->OBU2 (50ms).
    let delays = [[0, 2, 50], [2, 0, 2], [50, 2, 0]];
    let saw_upstream = Arc::new(AtomicBool::new(false));

    Hub::new(hub_fds.to_vec(), delays)
        .add_check(Arc::new(UpstreamMatchCheck {
            idx: 2,
            from: mac_obu2,
            to: mac_obu1,
            expected_payload: None,
            flag: saw_upstream.clone(),
        }))
        .spawn();

    let dev_rsu = Device::from_asyncfd_for_bench(
        mac_rsu,
        AsyncFd::new(unsafe { DeviceIo::from_raw_fd(node_fds[0]) }).unwrap(),
    );
    let dev_obu1 = Device::from_asyncfd_for_bench(
        mac_obu1,
        AsyncFd::new(unsafe { DeviceIo::from_raw_fd(node_fds[1]) }).unwrap(),
    );
    let dev_obu2 = Device::from_asyncfd_for_bench(
        mac_obu2,
        AsyncFd::new(unsafe { DeviceIo::from_raw_fd(node_fds[2]) }).unwrap(),
    );

    // Build Args
    // Use shorter hello_history/periodicity to converge faster in test
    let args_rsu = Args {
        bind: String::from("unused"),
        tap_name: None,
        ip: None,
        mtu: 1500,
        node_params: NodeParameters {
            node_type: NodeType::Rsu,
            hello_history: 2,
            hello_periodicity: Some(20),
            cached_candidates: 3,
        },
    };
    let args_obu1 = Args {
        bind: String::from("unused"),
        tap_name: None,
        ip: None,
        mtu: 1500,
        node_params: NodeParameters {
            node_type: NodeType::Obu,
            hello_history: 2,
            hello_periodicity: None,
            cached_candidates: 3,
        },
    };
    let args_obu2 = Args {
        bind: String::from("unused"),
        tap_name: None,
        ip: None,
        mtu: 1500,
        node_params: NodeParameters {
            node_type: NodeType::Obu,
            hello_history: 2,
            hello_periodicity: None,
            cached_candidates: 3,
        },
    };

    // Construct nodes
    let _rsu = Rsu::new(args_rsu, Arc::new(tun_rsu), Arc::new(dev_rsu)).expect("rsu new");
    let _obu1 = Obu::new(args_obu1, Arc::new(tun_obu1), Arc::new(dev_obu1)).expect("obu1 new");
    let obu2 = Obu::new(args_obu2, Arc::new(tun_obu2), Arc::new(dev_obu2)).expect("obu2 new");

    // Wait for OBU2 to cache upstream route; expect it to eventually prefer OBU1 (two-hop path).
    // Some routes may be briefly observed; poll until we see the desired selection.
    let mut cached = None;
    for _ in 0..200 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        cached = obu2.cached_upstream_mac();
        if cached == Some(mac_obu1) {
            break;
        }
    }

    assert_eq!(cached, Some(mac_obu1), "OBU2 should prefer OBU1 initially");

    // Ensure we have at least two candidates cached at OBU2 before cutting the link
    for _ in 0..160 {
        if obu2.cached_upstream_candidates_len() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    if obu2.cached_upstream_candidates_len() < 2 {
        let current = obu2.cached_upstream_mac();
        panic!(
            "expected at least two candidates cached, got {} (primary={:?})",
            obu2.cached_upstream_candidates_len(),
            current
        );
    }

    // Now simulate a send failure at OBU2 by shutting down reads on OBU2's hub endpoint (index 2).
    // This keeps the fd open but makes the peer's writes fail with EPIPE, triggering failover logic
    // without tearing down the entire hub.
    unsafe {
        libc::shutdown(hub_fds[2], libc::SHUT_RD);
    }

    // Repeatedly trigger upstream sends to force send errors and eventual failover
    let peer = Tun::new_shim(tun_obu2_b);
    for _ in 0..5 {
        let _ = peer.send_all(b"trigger after close").await;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Wait for OBU2 to promote to the next candidate (not OBU1)
    let mut promoted = None;
    for _ in 0..80 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        promoted = obu2.cached_upstream_mac();
        if promoted.is_some() && promoted.unwrap() != mac_obu1 {
            break;
        }
    }

    assert!(
        promoted.is_some(),
        "OBU2 did not promote after send failure"
    );
    assert_ne!(
        promoted.unwrap(),
        mac_obu1,
        "primary should have changed after failure"
    );
}
