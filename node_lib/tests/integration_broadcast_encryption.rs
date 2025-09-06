use node_lib::args::NodeType;
use node_lib::control::obu::Obu;
use node_lib::control::rsu::Rsu;
use node_lib::test_helpers::hub::HubCheck;
use node_lib::test_helpers::util::{
    await_condition_with_time_advance, mk_device_from_fd, mk_node_params, mk_shim_pairs,
};
use node_lib::Args;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

/// Captures broadcast traffic to verify encryption behavior
struct BroadcastChecker {
    broadcast_received_count: Arc<AtomicUsize>,
    broadcast_payloads: Arc<Mutex<Vec<Vec<u8>>>>,
    test_payload: Vec<u8>,
}

impl HubCheck for BroadcastChecker {
    fn on_packet(&self, from_idx: usize, data: &[u8]) {
        // Only look at packets from RSU (index 0) going to OBUs
        if from_idx == 0 {
            tracing::debug!("BroadcastChecker: RSU sent packet with {} bytes", data.len());
            if let Ok(msg) = node_lib::messages::message::Message::try_from(data) {
                tracing::debug!("BroadcastChecker: Parsed message: {:?}", msg.get_packet_type());
                if let node_lib::messages::packet_type::PacketType::Data(
                    node_lib::messages::data::Data::Downstream(downstream),
                ) = msg.get_packet_type()
                {
                    tracing::debug!("BroadcastChecker: Found downstream packet with {} bytes of data", downstream.data().len());
                    // Check if this downstream contains our test payload
                    if downstream
                        .data()
                        .windows(self.test_payload.len())
                        .any(|window| window == self.test_payload)
                    {
                        tracing::debug!("BroadcastChecker: Found test payload in downstream packet!");
                        self.broadcast_received_count
                            .fetch_add(1, Ordering::SeqCst);
                        self.broadcast_payloads
                            .lock()
                            .unwrap()
                            .push(downstream.data().to_vec());
                    } else {
                        tracing::debug!("BroadcastChecker: No test payload found in downstream packet");
                    }
                }
            } else {
                tracing::debug!("BroadcastChecker: Failed to parse message from RSU");
            }
        }
    }
}

/// Test that broadcast traffic from OBUs is properly encrypted when sent to each recipient.
/// 
/// Topology: OBU1 -> RSU <- OBU2, OBU3
/// 
/// The test verifies:
/// 1. OBU1 sends broadcast traffic to RSU (encrypted)
/// 2. RSU correctly distributes broadcast to OBU2 and OBU3 (each individually encrypted)
/// 3. OBU2 and OBU3 can properly decrypt and receive the broadcast payload
#[tokio::test]
async fn test_broadcast_traffic_encryption_distribution() {
    node_lib::init_test_tracing();
    tokio::time::pause();

    // Create 4 shim TUN pairs for RSU, OBU1, OBU2, OBU3
    let mut pairs = mk_shim_pairs(4);
    let (tun_rsu, tun_rsu_peer) = pairs.remove(0);
    let (tun_obu1, tun_obu1_peer) = pairs.remove(0);
    let (tun_obu2, tun_obu2_peer) = pairs.remove(0);
    let (tun_obu3, tun_obu3_peer) = pairs.remove(0);

    // Create hub for 4 nodes
    let (node_fds_v, hub_fds_v) =
        node_lib::test_helpers::util::mk_socketpairs(4).expect("mk_socketpairs failed");
    let node_fds = [node_fds_v[0], node_fds_v[1], node_fds_v[2], node_fds_v[3]];
    let hub_fds = [hub_fds_v[0], hub_fds_v[1], hub_fds_v[2], hub_fds_v[3]];

    // MAC addresses
    let mac_rsu: mac_address::MacAddress = [1, 2, 3, 4, 5, 6].into();
    let mac_obu1: mac_address::MacAddress = [10, 11, 12, 13, 14, 15].into();
    let mac_obu2: mac_address::MacAddress = [20, 21, 22, 23, 24, 25].into();
    let mac_obu3: mac_address::MacAddress = [30, 31, 32, 33, 34, 35].into();

    let dev_rsu = mk_device_from_fd(mac_rsu, node_fds[0]);
    let dev_obu1 = mk_device_from_fd(mac_obu1, node_fds[1]);
    let dev_obu2 = mk_device_from_fd(mac_obu2, node_fds[2]);
    let dev_obu3 = mk_device_from_fd(mac_obu3, node_fds[3]);

    // Setup hub with low latency between all nodes
    let delays: Vec<Vec<u64>> = vec![
        vec![0, 2, 2, 2], // RSU
        vec![2, 0, 4, 4], // OBU1
        vec![2, 4, 0, 4], // OBU2 
        vec![2, 4, 4, 0], // OBU3
    ];

    // Create a broadcast traffic checker
    let broadcast_count = Arc::new(AtomicUsize::new(0));
    let broadcast_payloads = Arc::new(Mutex::new(Vec::new()));
    let test_payload = b"BROADCAST_DATA_FOR_ALL_NODES";

    let checker = Arc::new(BroadcastChecker {
        broadcast_received_count: broadcast_count.clone(),
        broadcast_payloads: broadcast_payloads.clone(),
        test_payload: test_payload.to_vec(),
    });

    // Create hub with our broadcast checker
    node_lib::test_helpers::util::mk_hub_with_checks_mocked_time(
        hub_fds.to_vec(),
        delays,
        vec![checker],
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

    let mut args_obu3 = Args {
        bind: String::from("unused"),
        tap_name: None,
        ip: None,
        mtu: 1500,
        node_params: mk_node_params(NodeType::Obu, None),
    };
    args_obu3.node_params.enable_encryption = true;

    // Create nodes
    let _rsu = Rsu::new(args_rsu, Arc::new(tun_rsu), Arc::new(dev_rsu)).unwrap();
    let obu1 = Obu::new(args_obu1, Arc::new(tun_obu1), Arc::new(dev_obu1)).unwrap();
    let obu2 = Obu::new(args_obu2, Arc::new(tun_obu2), Arc::new(dev_obu2)).unwrap();
    let obu3 = Obu::new(args_obu3, Arc::new(tun_obu3), Arc::new(dev_obu3)).unwrap();

    // Wait for topology discovery and heartbeat reply exchanges
    tokio::time::advance(Duration::from_millis(500)).await;

    // Wait for all OBUs to discover upstream to RSU
    for (name, obu) in [("OBU1", &obu1), ("OBU2", &obu2), ("OBU3", &obu3)] {
        let result = await_condition_with_time_advance(
            Duration::from_millis(10),
            || {
                obu.cached_upstream_mac()
                    .filter(|&mac| mac == mac_rsu)
                    .map(|_| ())
            },
            Duration::from_secs(10),
        )
        .await;
        assert!(result.is_ok(), "{} should discover upstream to RSU", name);
    }

    // Give additional time for heartbeat replies to be exchanged
    // so RSU can discover the OBUs for broadcast distribution
    tokio::time::advance(Duration::from_millis(1000)).await;

    // Create a broadcast frame (destination MAC = broadcast address)
    let broadcast_mac = [255u8; 6]; // Broadcast MAC address
    let mut broadcast_frame = Vec::new();
    broadcast_frame.extend_from_slice(&broadcast_mac); // destination MAC (broadcast)
    broadcast_frame.extend_from_slice(&mac_obu1.bytes()); // source MAC
    broadcast_frame.extend_from_slice(test_payload); // payload

    // Send broadcast frame from OBU1
    tun_obu1_peer
        .send_all(&broadcast_frame)
        .await
        .expect("Failed to send broadcast frame");

    // Give time for the broadcast to propagate
    tokio::time::advance(Duration::from_millis(1000)).await;

    // Verify that RSU distributed the broadcast to the other OBUs
    // RSU should send downstream packets to OBU2 and OBU3 (not back to OBU1 as it's the sender)
    assert_eq!(
        broadcast_count.load(Ordering::SeqCst),
        2,
        "RSU should send broadcast to exactly 2 other OBUs (OBU2 and OBU3)"
    );

    // Verify that OBU2 and OBU3 received the broadcast correctly
    let mut obu2_received_data = vec![0u8; 1500];
    let obu2_result = node_lib::test_helpers::util::poll_tun_recv_with_timeout_mocked(
        &tun_obu2_peer,
        &mut obu2_received_data,
        50, // timeout per attempt
        10, // max attempts
    )
    .await;

    let mut obu3_received_data = vec![0u8; 1500];
    let obu3_result = node_lib::test_helpers::util::poll_tun_recv_with_timeout_mocked(
        &tun_obu3_peer,
        &mut obu3_received_data,
        50, // timeout per attempt
        10, // max attempts
    )
    .await;

    assert!(
        obu2_result.is_some(),
        "OBU2 should have received the broadcast packet"
    );
    assert!(
        obu3_result.is_some(),
        "OBU3 should have received the broadcast packet"
    );

    let obu2_size = obu2_result.unwrap();
    let obu3_size = obu3_result.unwrap();

    // Verify the received data contains the original broadcast frame
    assert_eq!(
        &obu2_received_data[..obu2_size],
        &broadcast_frame,
        "OBU2 should receive the original broadcast frame"
    );
    assert_eq!(
        &obu3_received_data[..obu3_size],
        &broadcast_frame,
        "OBU3 should receive the original broadcast frame"
    );

    // Verify the payload is present and readable at both OBUs
    assert!(
        obu2_received_data[..obu2_size]
            .windows(test_payload.len())
            .any(|window| window == test_payload),
        "OBU2 should receive the original broadcast payload in plaintext"
    );
    assert!(
        obu3_received_data[..obu3_size]
            .windows(test_payload.len())
            .any(|window| window == test_payload),
        "OBU3 should receive the original broadcast payload in plaintext"
    );
}

/// Test that RSU broadcast traffic is properly encrypted for each individual OBU recipient.
/// 
/// This test verifies that when RSU sends broadcast traffic, it encrypts it individually
/// for each OBU rather than sending the same encrypted payload to all.
#[tokio::test]
async fn test_rsu_broadcast_traffic_individual_encryption() {
    node_lib::init_test_tracing();
    tokio::time::pause();

    // Create 3 shim TUN pairs for RSU, OBU1, OBU2
    let mut pairs = mk_shim_pairs(3);
    let (tun_rsu, tun_rsu_peer) = pairs.remove(0);
    let (tun_obu1, tun_obu1_peer) = pairs.remove(0);
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
    let delays: Vec<Vec<u64>> = vec![vec![0, 2, 2], vec![2, 0, 4], vec![2, 4, 0]];

    // Create encryption checker to verify individual encryption
    let downstream_packets = Arc::new(Mutex::new(Vec::new()));
    let downstream_packets_clone = downstream_packets.clone();
    let test_payload = b"RSU_BROADCAST_TO_ALL_NODES";

    let encryption_checker = Arc::new(
        move |from_idx: usize, data: &[u8]| {
            // Capture downstream packets from RSU (index 0)
            if from_idx == 0 {
                if let Ok(msg) = node_lib::messages::message::Message::try_from(data) {
                    if let node_lib::messages::packet_type::PacketType::Data(
                        node_lib::messages::data::Data::Downstream(downstream),
                    ) = msg.get_packet_type()
                    {
                        downstream_packets_clone
                            .lock()
                            .unwrap()
                            .push(downstream.data().to_vec());
                    }
                }
            }
        }
    );

    // Use a simple hub checker that captures all downstream packets
    struct EncryptionChecker {
        captured_packets: Arc<Mutex<Vec<Vec<u8>>>>,
    }

    impl HubCheck for EncryptionChecker {
        fn on_packet(&self, from_idx: usize, data: &[u8]) {
            if from_idx == 0 {
                if let Ok(msg) = node_lib::messages::message::Message::try_from(data) {
                    if let node_lib::messages::packet_type::PacketType::Data(
                        node_lib::messages::data::Data::Downstream(downstream),
                    ) = msg.get_packet_type()
                    {
                        self.captured_packets
                            .lock()
                            .unwrap()
                            .push(downstream.data().to_vec());
                    }
                }
            }
        }
    }

    let checker = Arc::new(EncryptionChecker {
        captured_packets: downstream_packets.clone(),
    });

    node_lib::test_helpers::util::mk_hub_with_checks_mocked_time(
        hub_fds.to_vec(),
        delays,
        vec![checker],
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
    let obu1 = Obu::new(args_obu1, Arc::new(tun_obu1), Arc::new(dev_obu1)).unwrap();
    let obu2 = Obu::new(args_obu2, Arc::new(tun_obu2), Arc::new(dev_obu2)).unwrap();

    // Wait for topology discovery
    tokio::time::advance(Duration::from_millis(500)).await;

    // Wait for OBUs to discover RSU
    for (name, obu) in [("OBU1", &obu1), ("OBU2", &obu2)] {
        let result = await_condition_with_time_advance(
            Duration::from_millis(10),
            || {
                obu.cached_upstream_mac()
                    .filter(|&mac| mac == mac_rsu)
                    .map(|_| ())
            },
            Duration::from_secs(5),
        )
        .await;
        assert!(result.is_ok(), "{} should discover upstream to RSU", name);
    }

    // Create a broadcast frame to send from RSU's TUN interface
    let broadcast_mac = [255u8; 6]; // Broadcast MAC address
    let mut rsu_broadcast_frame = Vec::new();
    rsu_broadcast_frame.extend_from_slice(&broadcast_mac); // destination MAC (broadcast)
    rsu_broadcast_frame.extend_from_slice(&mac_rsu.bytes()); // source MAC
    rsu_broadcast_frame.extend_from_slice(test_payload); // payload

    // Send broadcast frame from RSU's TUN interface
    tun_rsu_peer
        .send_all(&rsu_broadcast_frame)
        .await
        .expect("Failed to send RSU broadcast frame");

    // Give time for the broadcast to propagate
    tokio::time::advance(Duration::from_millis(300)).await;

    // Verify RSU sent individual encrypted packets to each OBU
    let captured = downstream_packets.lock().unwrap();
    assert_eq!(
        captured.len(),
        2,
        "RSU should send individual downstream packets to each OBU"
    );

    // With proper encryption, each downstream packet should be different
    // (due to different nonces in AES-GCM encryption)
    assert_ne!(
        captured[0], captured[1],
        "Each encrypted downstream packet should be unique due to different nonces"
    );

    // Verify that both OBUs received and can decrypt the broadcast
    let mut obu1_received_data = vec![0u8; 1500];
    let obu1_result = node_lib::test_helpers::util::poll_tun_recv_with_timeout_mocked(
        &tun_obu1_peer,
        &mut obu1_received_data,
        50, // timeout per attempt
        10, // max attempts
    )
    .await;

    let mut obu2_received_data = vec![0u8; 1500];
    let obu2_result = node_lib::test_helpers::util::poll_tun_recv_with_timeout_mocked(
        &tun_obu2_peer,
        &mut obu2_received_data,
        50, // timeout per attempt
        10, // max attempts
    )
    .await;

    assert!(
        obu1_result.is_some(),
        "OBU1 should have received the RSU broadcast"
    );
    assert!(
        obu2_result.is_some(),
        "OBU2 should have received the RSU broadcast"
    );

    let obu1_size = obu1_result.unwrap();
    let obu2_size = obu2_result.unwrap();

    // Verify both OBUs received the correct decrypted data
    assert_eq!(
        &obu1_received_data[..obu1_size],
        &rsu_broadcast_frame,
        "OBU1 should receive the original RSU broadcast frame"
    );
    assert_eq!(
        &obu2_received_data[..obu2_size],
        &rsu_broadcast_frame,
        "OBU2 should receive the original RSU broadcast frame"
    );
}