use node_lib::test_helpers::hub::HubCheck;
use node_lib::test_helpers::util::{
    advance_until, await_condition_with_time_advance, mk_device_from_fd, mk_shim_pairs,
};
use obu_lib::Obu;
use rsu_lib::Rsu;
mod common;
use common::{mk_obu_args, mk_obu_args_encrypted, mk_rsu_args};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

/// Common payload checker that can be used to inspect packets for test payloads
struct PayloadChecker {
    payload_found: Arc<AtomicBool>,
    test_payload: Vec<u8>,
}

impl HubCheck for PayloadChecker {
    fn on_packet(&self, from_idx: usize, data: &[u8]) {
        // Check packets from OBU1 (index 1)
        if from_idx != 1 {
            return;
        }

        let Ok(msg) = node_lib::messages::message::Message::try_from(data) else {
            return;
        };

        let node_lib::messages::packet_type::PacketType::Data(data_msg) = msg.get_packet_type()
        else {
            return;
        };

        let message_data = match data_msg {
            node_lib::messages::data::Data::Upstream(upstream) => upstream.data(),
            node_lib::messages::data::Data::Downstream(downstream) => downstream.data(),
        };

        if message_data
            .windows(self.test_payload.len())
            .any(|window| window == self.test_payload)
        {
            self.payload_found.store(true, Ordering::SeqCst);
        }
    }
}

/// Test that payload encryption prevents intermediate OBUs from reading data.
/// Creates RSU -> OBU1 -> OBU2 topology with encryption enabled on OBUs.
/// RSU is now a transparent forwarding relay (no encryption/decryption).
/// Verifies that OBU1 (intermediate) cannot read the payload.
#[tokio::test]
async fn test_payload_encryption_prevents_inspection() {
    node_lib::init_test_tracing();
    tokio::time::pause();

    // Create 3 shim TUN pairs (RSU doesn't use one)
    let mut pairs = mk_shim_pairs(3);
    let (_tun_rsu_unused, _tun_rsu_peer) = pairs.remove(0);
    let (tun_obu1, _tun_obu1_peer) = pairs.remove(0);
    let (tun_obu2, tun_obu2_peer) = pairs.remove(0);

    // Create hub for 3 nodes
    let (node_fds_v, hub_fds_v) =
        node_lib::test_helpers::util::mk_socketpairs(3).expect("mk_socketpairs failed");
    let node_fds = [node_fds_v[0], node_fds_v[1], node_fds_v[2]];
    let hub_fds = [hub_fds_v[0], hub_fds_v[1], hub_fds_v[2]];

    // MAC addresses
    let mac_rsu: mac_address::MacAddress = [1, 2, 3, 4, 5, 6].into();
    let mac_obu1: mac_address::MacAddress = [10, 11, 12, 13, 14, 15].into();
    let mac_obu2: mac_address::MacAddress = [20, 21, 22, 23, 24, 25].into();

    let dev_rsu = mk_device_from_fd(mac_rsu, node_fds[0]);
    let dev_obu1 = mk_device_from_fd(mac_obu1, node_fds[1]);
    let dev_obu2 = mk_device_from_fd(mac_obu2, node_fds[2]);

    // Setup hub with delays to force OBU2 -> OBU1 -> RSU topology
    let delays: Vec<Vec<u64>> = vec![vec![0, 2, 100], vec![2, 0, 2], vec![100, 2, 0]];

    // Create a custom checker to inspect packets going through the intermediate OBU1
    let payload_inspected = Arc::new(AtomicBool::new(false));
    let payload_inspected_clone = payload_inspected.clone();
    let test_payload = b"secret data should not be readable by OBU1";
    let test_payload_vec = test_payload.to_vec();

    let inspector = Arc::new(PayloadChecker {
        payload_found: payload_inspected_clone,
        test_payload: test_payload_vec.clone(),
    });

    // Create hub with our custom inspector
    node_lib::test_helpers::util::mk_hub_with_checks_mocked_time(
        hub_fds.to_vec(),
        delays,
        vec![inspector],
    );

    // RSU uses regular args (no encryption - it's now a transparent relay)
    // OBUs use encrypted args (they handle the crypto)
    let args_rsu = mk_rsu_args(100);
    let args_obu1 = mk_obu_args_encrypted();
    let args_obu2 = mk_obu_args_encrypted();

    // Create nodes (RSU no longer takes a TUN device)
    let _rsu = Rsu::new(args_rsu, Arc::new(dev_rsu), "test_rsu".to_string()).unwrap();
    let _obu1 = Obu::new(
        args_obu1,
        Arc::new(tun_obu1),
        Arc::new(dev_obu1),
        "test_obu".to_string(),
    )
    .unwrap();
    let obu2 = Obu::new(
        args_obu2,
        Arc::new(tun_obu2),
        Arc::new(dev_obu2),
        "test_obu".to_string(),
    )
    .unwrap();

    // Wait for topology discovery
    tokio::time::advance(Duration::from_millis(500)).await;

    // Wait for OBU2 to discover upstream through OBU1
    let result = await_condition_with_time_advance(
        Duration::from_millis(10),
        || obu2.cached_upstream_mac(),
        Duration::from_secs(5),
    )
    .await;

    assert!(result.is_ok(), "OBU2 should discover upstream");

    let upstream_mac = obu2
        .cached_upstream_mac()
        .expect("OBU2 should have upstream");
    assert_eq!(
        upstream_mac, mac_obu1,
        "OBU2 should route through OBU1 (not directly to RSU)"
    );

    // Send test payload from OBU2
    let mut frame = Vec::new();
    frame.extend_from_slice(&mac_rsu.bytes()); // to
    frame.extend_from_slice(&mac_obu2.bytes()); // from
    frame.extend_from_slice(&test_payload_vec); // payload

    tun_obu2_peer
        .send_all(&frame)
        .await
        .expect("Failed to send test frame");

    // Give time for the frame to propagate through the network
    tokio::time::advance(Duration::from_millis(100)).await;

    // Verify that OBU1 did not see the plaintext payload
    assert!(
        !payload_inspected.load(Ordering::SeqCst),
        "Intermediate OBU1 should not be able to read the encrypted payload"
    );
}

/// Test that encryption can be disabled and payloads remain readable
#[tokio::test]
async fn test_encryption_disabled_allows_inspection() {
    node_lib::init_test_tracing();
    tokio::time::pause();

    // Similar setup but with encryption disabled
    let mut pairs = mk_shim_pairs(2);
    let (_tun_rsu_unused, _tun_rsu_peer) = pairs.remove(0);
    let (tun_obu1, tun_obu1_peer) = pairs.remove(0);

    let (node_fds_v, hub_fds_v) =
        node_lib::test_helpers::util::mk_socketpairs(2).expect("mk_socketpairs failed");
    let node_fds = [node_fds_v[0], node_fds_v[1]];
    let hub_fds = [hub_fds_v[0], hub_fds_v[1]];

    let mac_rsu: mac_address::MacAddress = [1, 2, 3, 4, 5, 6].into();
    let mac_obu1: mac_address::MacAddress = [10, 11, 12, 13, 14, 15].into();

    let dev_rsu = mk_device_from_fd(mac_rsu, node_fds[0]);
    let dev_obu1 = mk_device_from_fd(mac_obu1, node_fds[1]);

    let delays: Vec<Vec<u64>> = vec![vec![0, 2], vec![2, 0]];

    let payload_seen = Arc::new(AtomicBool::new(false));
    let payload_seen_clone = payload_seen.clone();
    let test_payload = b"readable data";
    let test_payload_vec = test_payload.to_vec();

    let checker = Arc::new(PayloadChecker {
        payload_found: payload_seen_clone,
        test_payload: test_payload_vec.clone(),
    });

    node_lib::test_helpers::util::mk_hub_with_checks_mocked_time(
        hub_fds.to_vec(),
        delays,
        vec![checker],
    );

    // Create nodes with encryption DISABLED
    let args_rsu = mk_rsu_args(100);
    let args_obu1 = mk_obu_args();

    // RSU no longer takes a TUN device
    let _rsu = Rsu::new(args_rsu, Arc::new(dev_rsu), "test_rsu".to_string()).unwrap();
    let obu1 = Obu::new(
        args_obu1,
        Arc::new(tun_obu1),
        Arc::new(dev_obu1),
        "test_obu".to_string(),
    )
    .unwrap();

    // Wait for topology discovery
    tokio::time::advance(Duration::from_millis(200)).await;

    let result = await_condition_with_time_advance(
        Duration::from_millis(10),
        || obu1.cached_upstream_mac(),
        Duration::from_secs(2),
    )
    .await;

    assert!(result.is_ok(), "OBU1 should discover upstream");

    // Give more time for the topology to fully stabilize
    tokio::time::advance(Duration::from_millis(500)).await;

    // Send frame with test payload
    let mut frame = Vec::new();
    frame.extend_from_slice(&mac_rsu.bytes());
    frame.extend_from_slice(&mac_obu1.bytes());
    frame.extend_from_slice(&test_payload_vec);

    tun_obu1_peer
        .send_all(&frame)
        .await
        .expect("Failed to send test frame");

    // Give more time for the frame to be processed and sent over the network
    advance_until(
        || payload_seen.load(Ordering::SeqCst),
        Duration::from_millis(1),
        Duration::from_millis(10),
    )
    .await;

    // When encryption is disabled, the payload should be readable
    assert!(
        payload_seen.load(Ordering::SeqCst),
        "With encryption disabled, payload should be readable in transit"
    );
}

/// Test that verifies encrypted packets cannot be inspected by intermediate nodes.
/// RSU is now a transparent relay - it forwards encrypted data without decryption.
#[tokio::test]
async fn test_ping_encryption_prevents_inspection_rsu_transparent() {
    node_lib::init_test_tracing();
    tokio::time::pause();

    // Create 3 shim TUN pairs (RSU doesn't use one)
    let mut pairs = mk_shim_pairs(3);
    let (_tun_rsu_unused, _tun_rsu_peer) = pairs.remove(0);
    let (tun_obu1, _tun_obu1_peer) = pairs.remove(0);
    let (tun_obu2, tun_obu2_peer) = pairs.remove(0);

    // Create hub for 3 nodes
    let (node_fds_v, hub_fds_v) =
        node_lib::test_helpers::util::mk_socketpairs(3).expect("mk_socketpairs failed");
    let node_fds = [node_fds_v[0], node_fds_v[1], node_fds_v[2]];
    let hub_fds = [hub_fds_v[0], hub_fds_v[1], hub_fds_v[2]];

    let mac_rsu: mac_address::MacAddress = [1, 2, 3, 4, 5, 6].into();
    let mac_obu1: mac_address::MacAddress = [10, 11, 12, 13, 14, 15].into();
    let mac_obu2: mac_address::MacAddress = [20, 21, 22, 23, 24, 25].into();

    let dev_rsu = mk_device_from_fd(mac_rsu, node_fds[0]);
    let dev_obu1 = mk_device_from_fd(mac_obu1, node_fds[1]);
    let dev_obu2 = mk_device_from_fd(mac_obu2, node_fds[2]);

    // Setup hub with delays: OBU2 -> RSU -> OBU1
    let delays: Vec<Vec<u64>> = vec![vec![0, 2, 4], vec![2, 0, 50], vec![4, 50, 0]];

    let ping_content_found = Arc::new(AtomicBool::new(false));
    let ping_content_found_clone = ping_content_found.clone();
    let ping_payload = b"This is a ping payload that should be encrypted";
    let ping_payload_vec = ping_payload.to_vec();

    let inspector = Arc::new(PayloadChecker {
        payload_found: ping_content_found_clone,
        test_payload: ping_payload_vec.clone(),
    });

    node_lib::test_helpers::util::mk_hub_with_checks_mocked_time(
        hub_fds.to_vec(),
        delays,
        vec![inspector],
    );

    // RSU is transparent relay (no encryption config)
    // OBUs handle encryption
    let args_rsu = mk_rsu_args(100);
    let args_obu1 = mk_obu_args_encrypted();
    let args_obu2 = mk_obu_args_encrypted();

    // Create nodes (RSU no longer takes a TUN device)
    let _rsu = Rsu::new(args_rsu, Arc::new(dev_rsu), "test_rsu".to_string()).unwrap();
    let _obu1 = Obu::new(
        args_obu1,
        Arc::new(tun_obu1),
        Arc::new(dev_obu1),
        "test_obu".to_string(),
    )
    .unwrap();
    let obu2 = Obu::new(
        args_obu2,
        Arc::new(tun_obu2),
        Arc::new(dev_obu2),
        "test_obu".to_string(),
    )
    .unwrap();

    // Wait for topology discovery
    tokio::time::advance(Duration::from_millis(500)).await;

    let result = await_condition_with_time_advance(
        Duration::from_millis(10),
        || {
            if let Some(upstream_mac) = obu2.cached_upstream_mac() {
                if upstream_mac == mac_rsu {
                    return Some(upstream_mac);
                }
            }
            None
        },
        Duration::from_secs(5),
    )
    .await;

    assert!(result.is_ok(), "OBU2 should discover upstream through RSU");

    // Send ping payload from OBU2
    let mut frame = Vec::new();
    frame.extend_from_slice(&mac_obu1.bytes()); // to OBU1
    frame.extend_from_slice(&mac_obu2.bytes()); // from OBU2
    frame.extend_from_slice(&ping_payload_vec);

    tun_obu2_peer
        .send_all(&frame)
        .await
        .expect("Failed to send ping frame");

    // Give time for the frame to propagate
    tokio::time::advance(Duration::from_millis(200)).await;

    // Verify that plaintext payload was NOT visible on the wire
    assert!(
        !ping_content_found.load(Ordering::SeqCst),
        "Encrypted ping payload should not be readable in transit"
    );
}
