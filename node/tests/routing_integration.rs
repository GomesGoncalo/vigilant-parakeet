use node_lib::test_helpers::hub::UpstreamExpectation;
use node_lib::test_helpers::util::{
    await_condition_with_time_advance, mk_device_from_fd, mk_hub_with_checks_mocked_time,
    mk_shim_pairs,
};
use obu_lib::Obu;
use rsu_lib::Rsu;
use std::sync::Arc;
use std::time::Duration;

mod common;

#[tokio::test]
async fn obu_and_rsu_choose_same_next_hop_for_same_messages() {
    node_lib::init_test_tracing();
    // Use mocked time for deterministic test execution - MUST be before node creation
    tokio::time::pause();

    // Create 2 shim TUN pairs and keep peer for OBU
    let mut pairs = mk_shim_pairs(2);
    let (tun_rsu, _tun_rsu_peer) = pairs.remove(0);
    let (tun_obu, tun_obu_peer) = pairs.remove(0);

    // Create node<->hub socketpair for two nodes
    let (node_fds_v, hub_fds_v) =
        node_lib::test_helpers::util::mk_socketpairs(2).expect("mk_socketpairs failed");
    let node_fds = [node_fds_v[0], node_fds_v[1]];

    // MACs
    let mac_rsu: mac_address::MacAddress = [1, 2, 3, 4, 5, 6].into();
    let mac_obu: mac_address::MacAddress = [10, 11, 12, 13, 14, 15].into();

    // Hub delays: make RSU->OBU direct low latency so both observe same next hop
    let delays: Vec<Vec<u64>> = vec![vec![0, 2], vec![2, 0]];

    // Upstream expectation: expect OBU to forward a frame upstream toward RSU
    let payload: &[u8] = b"payload";
    // UpstreamExpectation::new(idx, from, to, expected_payload)
    let (upstream_expectation, upstream_future) =
        UpstreamExpectation::new(1, mac_obu, mac_rsu, Some(payload.to_vec()));

    mk_hub_with_checks_mocked_time(hub_fds_v, delays, vec![Arc::new(upstream_expectation)]);

    // Wrap device fds
    let dev_rsu = mk_device_from_fd(mac_rsu, node_fds[0]);
    let dev_obu = mk_device_from_fd(mac_obu, node_fds[1]);

    // Build Args
    let args_rsu = common::mk_rsu_args(50);
    let args_obu = common::mk_obu_args();

    // Construct nodes
    let _rsu = Rsu::new(args_rsu, Arc::new(tun_rsu), Arc::new(dev_rsu)).expect("Rsu::new failed");
    let obu = Obu::new(args_obu, Arc::new(tun_obu), Arc::new(dev_obu)).expect("Obu::new failed");

    // Wait for OBU to cache upstream via RSU
    let res = await_condition_with_time_advance(
        Duration::from_millis(10),
        || {
            if let Some(mac) = obu.cached_upstream_mac() {
                if mac == mac_rsu {
                    return Some(mac);
                }
            }
            None
        },
        Duration::from_secs(10),
    )
    .await;

    match res {
        Ok(mac) => assert_eq!(mac, mac_rsu),
        Err(_) => panic!("OBU did not cache upstream within timeout"),
    }

    // Trigger an upstream send by writing on the peer end of OBU's TUN; the session task should forward it.
    tun_obu_peer
        .send_all(payload)
        .await
        .expect("tun_obu_peer.send_all failed");

    // Wait for hub to observe the upstream packet
    let _ =
        node_lib::test_helpers::util::await_with_timeout(upstream_future, Duration::from_secs(2))
            .await
            .expect("Hub did not observe expected upstream packet within timeout");
}
