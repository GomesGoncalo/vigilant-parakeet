use node_lib::args::NodeType;
use node_lib::control::obu::Obu;
use node_lib::control::rsu::Rsu;
use node_lib::test_helpers::hub::UpstreamMatchCheck;
use node_lib::test_helpers::util::mk_shim_pairs;
use node_lib::test_helpers::util::{mk_args, mk_device_from_fd, poll_until};
use std::sync::{atomic::AtomicBool, Arc};
use std::time::Duration;

/// Integration test: build RSU, OBU1, OBU2 connected by a hub. OBU2 should
/// prefer OBU1 as upstream (two-hop) given the delay matrix. Then close OBU1's
/// hub endpoint to simulate a send failure and verify OBU2 promotes to another
/// candidate.
#[tokio::test]
async fn obu_promotes_on_primary_send_failure_via_hub_closure() {
    node_lib::init_test_tracing();

    // Create shim TUN pairs and keep the peer for OBU2
    let mut pairs = mk_shim_pairs(3);
    let (tun_rsu, _peer0) = pairs.remove(0);
    let (tun_obu1, _peer1) = pairs.remove(0);
    let (tun_obu2, tun_obu2_peer) = pairs.remove(0);

    // Create 3 node<->hub links as socketpairs: (node_fd[i], hub_fd[i])
    let (node_fds_v, hub_fds_v) = node_lib::test_helpers::util::mk_socketpairs(3);
    let node_fds = [node_fds_v[0], node_fds_v[1], node_fds_v[2]];
    let hub_fds = [hub_fds_v[0], hub_fds_v[1], hub_fds_v[2]];

    // Node MACs: index 0=RSU, 1=OBU1, 2=OBU2
    let mac_rsu: mac_address::MacAddress = [1, 2, 3, 4, 5, 6].into();
    let mac_obu1: mac_address::MacAddress = [10, 11, 12, 13, 14, 15].into();
    let mac_obu2: mac_address::MacAddress = [20, 21, 22, 23, 24, 25].into();

    // Hub delays: prefer RSU<->OBU1 and OBU1<->OBU2 (2ms) over direct RSU<->OBU2 (50ms).
    let delays: Vec<Vec<u64>> = vec![vec![0, 2, 50], vec![2, 0, 2], vec![50, 2, 0]];
    let saw_upstream = Arc::new(AtomicBool::new(false));

    node_lib::test_helpers::util::mk_hub_with_checks(
        hub_fds.to_vec(),
        delays,
        vec![Arc::new(UpstreamMatchCheck {
            idx: 2,
            from: mac_obu2,
            to: mac_obu1,
            expected_payload: None,
            flag: saw_upstream.clone(),
        }) as Arc<dyn node_lib::test_helpers::hub::HubCheck>],
    );

    let dev_rsu = mk_device_from_fd(mac_rsu, node_fds[0]);
    let dev_obu1 = mk_device_from_fd(mac_obu1, node_fds[1]);
    let dev_obu2 = mk_device_from_fd(mac_obu2, node_fds[2]);

    // Build Args using the shared helper.
    let args_rsu = mk_args(NodeType::Rsu, Some(50));
    let args_obu1 = mk_args(NodeType::Obu, None);
    let args_obu2 = mk_args(NodeType::Obu, None);

    // Construct nodes
    let _rsu = Rsu::new(args_rsu, Arc::new(tun_rsu), Arc::new(dev_rsu)).expect("rsu new");
    let _obu1 = Obu::new(args_obu1, Arc::new(tun_obu1), Arc::new(dev_obu1)).expect("obu1 new");
    let obu2 = Obu::new(args_obu2, Arc::new(tun_obu2), Arc::new(dev_obu2)).expect("obu2 new");

    // Wait for OBU2 to cache upstream route; expect it to eventually prefer OBU1
    // (two-hop path). Poll until the desired selection is observed.
    let cached = poll_until(|| obu2.cached_upstream_mac(), 200, 100).await;
    assert_eq!(cached, Some(mac_obu1), "OBU2 should prefer OBU1 initially");

    // Ensure we have at least two candidates cached at OBU2 before cutting the link
    let have_two = poll_until(
        || {
            let len = obu2.cached_upstream_candidates_len();
            if len >= 2 {
                Some(len)
            } else {
                None
            }
        },
        160,
        50,
    )
    .await;
    if have_two.is_none() {
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
    let _ = tun_obu2_peer.send_all(b"trigger after close").await;
    for _ in 0..5 {
        let _ = tun_obu2_peer.send_all(b"trigger after close").await;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Wait for OBU2 to promote to the next candidate (not OBU1)
    let promoted = poll_until(
        || {
            let p = obu2.cached_upstream_mac();
            if p.is_some() && p.unwrap() != mac_obu1 {
                p
            } else {
                None
            }
        },
        80,
        50,
    )
    .await;

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
