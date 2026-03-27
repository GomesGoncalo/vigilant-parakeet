use node_lib::test_helpers::hub::UpstreamMatchCheck;
use node_lib::test_helpers::util::{
    await_condition_with_time_advance, mk_device_from_fd, repeat_async_send_mocked,
};
use node_lib::test_helpers::util::{mk_hub_with_checks_mocked_time, mk_shim_pairs};
use obu_lib::Obu;
use rsu_lib::Rsu;
mod common;
use common::{mk_obu_args, mk_rsu_args};
use std::sync::{atomic::AtomicBool, Arc};
use std::time::Duration;

/// Integration test: build RSU, OBU1, OBU2 connected by a hub. OBU2 should
/// prefer OBU1 as upstream (two-hop) given the delay matrix. Then close OBU1's
/// hub endpoint to simulate a send failure and verify OBU2 promotes to another
/// candidate.
///
/// Uses mocked time (`tokio::time::pause`) for deterministic behaviour under
/// coverage tools (tarpaulin) that distort real-time tokio timers.
///
/// Under tarpaulin's ptrace instrumentation, the failover depends on
/// timerfd-based heartbeats (real kernel timers) whose interaction with
/// mocked time is non-deterministic on CI runners. The test is validated
/// by the normal `cargo test` CI step (build-test) and skipped during
/// tarpaulin coverage runs.
#[tokio::test]
async fn obu_promotes_on_primary_send_failure_via_hub_closure() -> anyhow::Result<()> {
    if std::env::var("TARPAULIN").is_ok() {
        eprintln!("Skipping failover test under tarpaulin (timerfd/ptrace incompatibility)");
        return Ok(());
    }
    node_lib::init_test_tracing();
    tokio::time::pause();

    // Create shim TUN pairs and keep the peer for OBU2
    let mut pairs = mk_shim_pairs(3);
    let (_tun_rsu, _peer0) = pairs.remove(0);
    let (tun_obu1, _peer1) = pairs.remove(0);
    let (tun_obu2, tun_obu2_peer) = pairs.remove(0);

    // Create 3 node<->hub links as socketpairs: (node_fd[i], hub_fd[i])
    let (node_fds_v, hub_fds_v) = node_lib::test_helpers::util::mk_socketpairs(3)?;
    let node_fds = [node_fds_v[0], node_fds_v[1], node_fds_v[2]];
    let hub_fds = [hub_fds_v[0], hub_fds_v[1], hub_fds_v[2]];

    // Node MACs: index 0=RSU, 1=OBU1, 2=OBU2
    let mac_rsu: mac_address::MacAddress = [1, 2, 3, 4, 5, 6].into();
    let mac_obu1: mac_address::MacAddress = [10, 11, 12, 13, 14, 15].into();
    let mac_obu2: mac_address::MacAddress = [20, 21, 22, 23, 24, 25].into();

    // Hub delays: prefer RSU<->OBU1 and OBU1<->OBU2 (2ms) over direct RSU<->OBU2 (50ms).
    let delays: Vec<Vec<u64>> = vec![vec![0, 2, 50], vec![2, 0, 2], vec![50, 2, 0]];
    let saw_upstream = Arc::new(AtomicBool::new(false));

    mk_hub_with_checks_mocked_time(
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
    let args_rsu = mk_rsu_args(50);
    let args_obu1 = mk_obu_args();
    let args_obu2 = mk_obu_args();

    // Construct nodes (RSU no longer takes a TUN device)
    let _rsu = Rsu::new(args_rsu, Arc::new(dev_rsu), "test_rsu".to_string())?;
    let _obu1 = Obu::new(
        args_obu1,
        Arc::new(tun_obu1),
        Arc::new(dev_obu1),
        "test_obu1".to_string(),
    )?;
    let obu2 = Obu::new(
        args_obu2,
        Arc::new(tun_obu2),
        Arc::new(dev_obu2),
        "test_obu2".to_string(),
    )?;

    // Wait for OBU2 to cache upstream route; expect it to eventually prefer OBU1
    // (two-hop path). Advance mocked time in 10ms steps.
    let cached = await_condition_with_time_advance(
        Duration::from_millis(10),
        || obu2.cached_upstream_mac(),
        Duration::from_secs(10),
    )
    .await;
    assert!(cached.is_ok(), "OBU2 should discover upstream");
    assert_eq!(
        cached.unwrap(),
        mac_obu1,
        "OBU2 should prefer OBU1 initially"
    );

    // Ensure we have at least two candidates cached at OBU2 before cutting the link.
    let have_two = await_condition_with_time_advance(
        Duration::from_millis(10),
        || {
            let len = obu2.cached_upstream_candidates_len();
            if len >= 2 {
                Some(len)
            } else {
                None
            }
        },
        Duration::from_secs(10),
    )
    .await;
    assert!(
        have_two.is_ok(),
        "expected at least two candidates cached, got {} (primary={:?})",
        obu2.cached_upstream_candidates_len(),
        obu2.cached_upstream_mac()
    );

    // Now simulate a send failure at OBU2 by shutting down reads on OBU2's hub endpoint (index 2).
    // This keeps the fd open but makes the peer's writes fail with EPIPE, triggering failover logic
    // without tearing down the entire hub.
    node_lib::test_helpers::util::shutdown_read(hub_fds[2]);

    // Repeatedly trigger upstream sends to force send errors and eventual failover.
    repeat_async_send_mocked(|| tun_obu2_peer.send_all(b"trigger after close"), 12, 20).await;

    // Wait for OBU2 to promote to the next candidate (not OBU1).
    let promoted = await_condition_with_time_advance(
        Duration::from_millis(10),
        || {
            let p = obu2.cached_upstream_mac();
            if p.is_some() && p.unwrap() != mac_obu1 {
                p
            } else {
                None
            }
        },
        Duration::from_secs(10),
    )
    .await;

    assert!(promoted.is_ok(), "OBU2 did not promote after send failure");
    assert_ne!(
        promoted.unwrap(),
        mac_obu1,
        "primary should have changed after failure"
    );

    Ok(())
}
