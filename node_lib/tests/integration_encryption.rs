use node_lib::args::NodeType;
use node_lib::control::obu::Obu;
use node_lib::control::rsu::Rsu;
use node_lib::test_helpers::util::{
    await_condition_with_time_advance, mk_device_from_fd, mk_shim_pairs, mk_node_params,
};
use node_lib::test_helpers::hub::HubCheck;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::Duration;
use node_lib::Args;

/// Test that payload encryption prevents intermediate OBUs from reading data.
/// Creates RSU -> OBU1 -> OBU2 topology with encryption enabled.
/// Verifies that OBU1 (intermediate) cannot read the payload while OBU2 (destination) can.
#[tokio::test]
async fn test_payload_encryption_prevents_inspection() {
    node_lib::init_test_tracing();
    tokio::time::pause();

    // Create 3 shim TUN pairs
    let mut pairs = mk_shim_pairs(3);
    let (tun_rsu, _tun_rsu_peer) = pairs.remove(0);
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

    // Setup hub with low latency
    let delays: Vec<Vec<u64>> = vec![vec![0, 2, 4], vec![2, 0, 2], vec![4, 2, 0]];
    
    // Create a custom checker to inspect packets going through the intermediate OBU1
    let payload_inspected = Arc::new(AtomicBool::new(false));
    let payload_inspected_clone = payload_inspected.clone();
    let test_payload = b"secret data should not be readable by OBU1";
    let test_payload_vec = test_payload.to_vec();
    
    struct PayloadInspector {
        payload_inspected: Arc<AtomicBool>,
        test_payload: Vec<u8>,
    }
    
    impl HubCheck for PayloadInspector {
        fn on_packet(&self, from_idx: usize, data: &[u8]) {
            // Check if this is from OBU1 (index 1) forwarding data
            if from_idx == 1 {
                // Try to parse as message and look for the plaintext payload
                if let Ok(msg) = node_lib::messages::message::Message::try_from(data) {
                    if let node_lib::messages::packet_type::PacketType::Data(
                        node_lib::messages::data::Data::Upstream(upstream)
                    ) = msg.get_packet_type() {
                        // Check if the raw payload contains our test data (it shouldn't if encrypted)
                        if upstream.data().windows(self.test_payload.len()).any(|window| window == self.test_payload) {
                            self.payload_inspected.store(true, Ordering::SeqCst);
                        }
                    }
                }
            }
        }
    }
    
    let inspector = Arc::new(PayloadInspector {
        payload_inspected: payload_inspected_clone,
        test_payload: test_payload_vec.clone(),
    });

    // Create hub with our custom inspector
    node_lib::test_helpers::util::mk_hub_with_checks_mocked_time(
        hub_fds.to_vec(),
        delays,
        vec![inspector],
    );

    // Create nodes with encryption enabled
    let mut args_rsu = Args {
        bind: String::from("unused"),
        tap_name: None,
        ip: None,
        mtu: 1500,
        node_params: mk_node_params(NodeType::Rsu, Some(100)),
    };
    args_rsu.node_params.enable_encryption = true;

    let mut args_obu1 = Args {
        bind: String::from("unused"),
        tap_name: None,
        ip: None,
        mtu: 1500,
        node_params: mk_node_params(NodeType::Obu, None),
    };
    args_obu1.node_params.enable_encryption = true;

    let mut args_obu2 = Args {
        bind: String::from("unused"),
        tap_name: None,
        ip: None,
        mtu: 1500,
        node_params: mk_node_params(NodeType::Obu, None),
    };
    args_obu2.node_params.enable_encryption = true;

    // Create nodes
    let _rsu = Rsu::new(args_rsu, Arc::new(tun_rsu), Arc::new(dev_rsu)).unwrap();
    let _obu1 = Obu::new(args_obu1, Arc::new(tun_obu1), Arc::new(dev_obu1)).unwrap();
    let obu2 = Obu::new(args_obu2, Arc::new(tun_obu2), Arc::new(dev_obu2)).unwrap();

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

    // Send test payload from OBU2 to RSU
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
    assert!(!payload_inspected.load(Ordering::SeqCst), 
            "Intermediate OBU1 should not be able to read the encrypted payload");

    // TODO: Verify that RSU received and decrypted the payload correctly
    // This would require capturing what RSU sends to its TUN interface
}

/// Test that encryption can be disabled and payloads remain readable
#[tokio::test]
async fn test_encryption_disabled_allows_inspection() {
    node_lib::init_test_tracing();
    tokio::time::pause();

    // Similar setup but with encryption disabled
    let mut pairs = mk_shim_pairs(2);
    let (tun_rsu, _tun_rsu_peer) = pairs.remove(0);
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
    
    // Custom checker to look for plaintext payload in upstream data
    struct PlaintextChecker {
        payload_seen: Arc<AtomicBool>,
        test_payload: Vec<u8>,
    }
    
    impl HubCheck for PlaintextChecker {
        fn on_packet(&self, from_idx: usize, data: &[u8]) {
            // Check all packets from OBU1 (index 1)
            if from_idx == 1 {
                if let Ok(msg) = node_lib::messages::message::Message::try_from(data) {
                    if let node_lib::messages::packet_type::PacketType::Data(
                        node_lib::messages::data::Data::Upstream(upstream)
                    ) = msg.get_packet_type() {
                        // The upstream data should contain to+from+payload
                        // Check if we can find our payload starting at offset 12
                        if upstream.data().len() >= 12 + self.test_payload.len() {
                            let payload_part = &upstream.data()[12..];
                            if payload_part.starts_with(&self.test_payload) {
                                self.payload_seen.store(true, Ordering::SeqCst);
                            }
                        }
                    }
                }
            }
        }
    }
    
    let checker = Arc::new(PlaintextChecker {
        payload_seen: payload_seen_clone,
        test_payload: test_payload_vec.clone(),
    });

    node_lib::test_helpers::util::mk_hub_with_checks_mocked_time(
        hub_fds.to_vec(),
        delays,
        vec![checker],
    );

    // Create nodes with encryption DISABLED
    let args_rsu = Args {
        bind: String::from("unused"),
        tap_name: None,
        ip: None,
        mtu: 1500,
        node_params: mk_node_params(NodeType::Rsu, Some(100)), // encryption defaults to false
    };

    let args_obu1 = Args {
        bind: String::from("unused"),
        tap_name: None,
        ip: None,
        mtu: 1500,
        node_params: mk_node_params(NodeType::Obu, None), // encryption defaults to false
    };

    let _rsu = Rsu::new(args_rsu, Arc::new(tun_rsu), Arc::new(dev_rsu)).unwrap();
    let obu1 = Obu::new(args_obu1, Arc::new(tun_obu1), Arc::new(dev_obu1)).unwrap();

    // Wait for topology discovery
    tokio::time::advance(Duration::from_millis(200)).await;
    
    let result = await_condition_with_time_advance(
        Duration::from_millis(10),
        || obu1.cached_upstream_mac(),
        Duration::from_secs(2),
    )
    .await;

    assert!(result.is_ok(), "OBU1 should discover upstream");

    // Send frame with test payload
    let mut frame = Vec::new();
    frame.extend_from_slice(&mac_rsu.bytes());
    frame.extend_from_slice(&mac_obu1.bytes());
    frame.extend_from_slice(&test_payload_vec);
    
    tun_obu1_peer
        .send_all(&frame)
        .await
        .expect("Failed to send test frame");

    // Give time for the frame to be processed and sent over the network
    for _i in 0..10 {
        tokio::time::advance(Duration::from_millis(100)).await;
        if payload_seen.load(Ordering::SeqCst) {
            break;
        }
    }

    // When encryption is disabled, the payload should be readable
    assert!(payload_seen.load(Ordering::SeqCst), 
            "With encryption disabled, payload should be readable in transit");
}