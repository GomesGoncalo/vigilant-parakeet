use node_lib::args::NodeType;
use node_lib::control::obu::Obu;
use node_lib::control::rsu::Rsu;
use node_lib::server::Server;
use node_lib::test_helpers::hub::HubCheck;
use node_lib::test_helpers::util::{
    await_condition_with_time_advance, mk_device_from_fd, mk_node_params, mk_shim_pairs,
};
use node_lib::Args;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

// Type alias to simplify complex type
type CapturedPackets = Arc<Mutex<Vec<(usize, Vec<u8>)>>>;

/// Captures broadcast traffic for testing
struct BroadcastTrafficChecker {
    rsu_downstream_count: Arc<AtomicUsize>,
    captured_packets: CapturedPackets, // (from_idx, packet_data)
}

impl HubCheck for BroadcastTrafficChecker {
    fn on_packet(&self, from_idx: usize, data: &[u8]) {
        // Capture all Data::Downstream packets from RSU (index 0)
        if from_idx == 0 {
            // With SOCK_DGRAM, each packet is received separately with proper frame boundaries
            match node_lib::messages::message::Message::try_from(data) {
                Ok(msg) => {
                    if let node_lib::messages::packet_type::PacketType::Data(
                        node_lib::messages::data::Data::Downstream(_),
                    ) = msg.get_packet_type()
                    {
                        self.rsu_downstream_count.fetch_add(1, Ordering::SeqCst);
                        self.captured_packets
                            .lock()
                            .unwrap()
                            .push((from_idx, data.to_vec()));
                    }
                }
                Err(_) => {
                    // Ignore unparseable data
                }
            }
        }
    }
}

/// Test broadcast traffic from OBU goes to RSU and spreads to other nodes
/// This addresses the first part of the user's request  
#[tokio::test]
async fn test_obu_broadcast_spreads_to_other_nodes() {
    node_lib::init_test_tracing();
    tokio::time::pause();

    // Start the server first
    let server_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080);
    let _server = Server::new(server_addr).await.expect("Failed to start server");

    // Create 3 shim TUN pairs for RSU, OBU1 (sender), OBU2 (receiver)
    let mut pairs = mk_shim_pairs(3);
    let (tun_rsu, _tun_rsu_peer) = pairs.remove(0);
    let (tun_obu1, tun_obu1_peer) = pairs.remove(0);
    let (tun_obu2, _tun_obu2_peer) = pairs.remove(0);

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

    // Setup hub with low latency between all nodes
    let delays: Vec<Vec<u64>> = vec![vec![0, 2, 2], vec![2, 0, 4], vec![2, 4, 0]];

    // Create traffic checker to monitor RSU downstream packets
    let downstream_count = Arc::new(AtomicUsize::new(0));
    let captured_packets = Arc::new(Mutex::new(Vec::new()));
    let checker = Arc::new(BroadcastTrafficChecker {
        rsu_downstream_count: downstream_count.clone(),
        captured_packets: captured_packets.clone(),
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
        node_params: mk_node_params(NodeType::Rsu, Some(50)),
    };
    args_rsu.node_params.enable_encryption = true;
    args_rsu.node_params.server_address = Some("127.0.0.1:8080".parse().unwrap());

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

    // Wait for both OBUs to discover RSU as upstream
    tokio::time::advance(Duration::from_millis(200)).await;

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
        assert!(result.is_ok(), "{} should discover RSU as upstream", name);
    }

    // Wait longer for RSU to receive heartbeat replies and build routing table
    tokio::time::advance(Duration::from_millis(500)).await;

    // Verify RSU has routing entries for both OBUs before broadcasting
    let result = await_condition_with_time_advance(
        Duration::from_millis(10),
        || {
            let next_hop_count = _rsu.next_hop_count();
            // RSU should have routing entries for both OBUs
            if next_hop_count >= 2 {
                Some(())
            } else {
                None
            }
        },
        Duration::from_secs(5),
    )
    .await;
    assert!(
        result.is_ok(),
        "RSU should have routing entries for OBUs before processing broadcast"
    );

    // Send broadcast frame from OBU1
    let broadcast_mac = [255u8; 6]; // Broadcast destination
    let test_payload = b"BROADCAST_TEST"; // Test payload
    let mut broadcast_frame = Vec::new();
    broadcast_frame.extend_from_slice(&broadcast_mac); // destination MAC
    broadcast_frame.extend_from_slice(&mac_obu1.bytes()); // source MAC
    broadcast_frame.extend_from_slice(test_payload); // payload

    tun_obu1_peer
        .send_all(&broadcast_frame)
        .await
        .expect("Failed to send broadcast frame from OBU1");

    // Use a more robust waiting mechanism that properly yields control
    let mut attempts = 0;
    loop {
        tokio::time::advance(Duration::from_millis(100)).await;
        tokio::task::yield_now().await;

        let count = downstream_count.load(Ordering::SeqCst);
        if count > 0 {
            break;
        }

        attempts += 1;
        if attempts > 20 {
            // 2 seconds total
            break;
        }
    }

    // Verify RSU generated downstream packets for broadcast distribution
    let count = downstream_count.load(Ordering::SeqCst);

    // Should have generated exactly 1 downstream packet (for OBU2, since OBU1 is the sender)
    assert_eq!(
        count, 1,
        "RSU should generate exactly 1 downstream packet for OBU2 when OBU1 sends broadcast"
    );

    // Verify that captured packets contain properly encrypted broadcast data
    let captured = captured_packets.lock().unwrap();
    assert_eq!(
        captured.len(),
        1,
        "Should have captured exactly 1 downstream packet"
    );

    let (from_idx, packet_data) = &captured[0];
    assert_eq!(*from_idx, 0, "Packet should come from RSU (index 0)");

    // Parse the downstream message to verify it contains encrypted data
    let msg = node_lib::messages::message::Message::try_from(packet_data.as_slice())
        .expect("Should be able to parse downstream message");

    if let node_lib::messages::packet_type::PacketType::Data(
        node_lib::messages::data::Data::Downstream(downstream),
    ) = msg.get_packet_type()
    {
        // The downstream data should be encrypted (different from original)
        assert_ne!(
            downstream.data(),
            &broadcast_frame,
            "Downstream data should be encrypted (different from original broadcast frame)"
        );

        // Verify the data is at least the expected size (original + encryption overhead)
        assert!(
            downstream.data().len() >= broadcast_frame.len(),
            "Encrypted data should be at least as large as original frame"
        );
    } else {
        panic!("Expected downstream data packet");
    }
}

/// Test RSU broadcast traffic is sent individually and encrypted to each node
/// This addresses the second part of the user's request
#[tokio::test]
async fn test_rsu_broadcast_individual_encryption() {
    node_lib::init_test_tracing();
    tokio::time::pause();

    // Start the server first
    let server_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080);
    let _server = Server::new(server_addr).await.expect("Failed to start server");

    // Create 3 shim TUN pairs for RSU, OBU1, OBU2
    let mut pairs = mk_shim_pairs(3);
    let (tun_rsu, tun_rsu_peer) = pairs.remove(0);
    let (tun_obu1, _tun_obu1_peer) = pairs.remove(0);
    let (tun_obu2, _tun_obu2_peer) = pairs.remove(0);

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

    // Create traffic checker to capture downstream packets
    let downstream_count = Arc::new(AtomicUsize::new(0));
    let captured_packets = Arc::new(Mutex::new(Vec::new()));
    let checker = Arc::new(BroadcastTrafficChecker {
        rsu_downstream_count: downstream_count.clone(),
        captured_packets: captured_packets.clone(),
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
        node_params: mk_node_params(NodeType::Rsu, Some(50)),
    };
    args_rsu.node_params.enable_encryption = true;
    args_rsu.node_params.server_address = Some("127.0.0.1:8080".parse().unwrap());

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

    // Wait for OBUs to discover RSU
    tokio::time::advance(Duration::from_millis(200)).await;

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
        assert!(result.is_ok(), "{} should discover RSU as upstream", name);
    }

    // Wait for RSU to build routing table by receiving heartbeat replies
    tokio::time::advance(Duration::from_millis(500)).await;

    // Verify RSU has routing entries for both OBUs before sending broadcast
    let result = await_condition_with_time_advance(
        Duration::from_millis(10),
        || {
            let next_hop_count = _rsu.next_hop_count();
            // RSU should have routing entries for both OBUs
            if next_hop_count >= 2 {
                Some(())
            } else {
                None
            }
        },
        Duration::from_secs(5),
    )
    .await;
    assert!(
        result.is_ok(),
        "RSU should have routing entries for OBUs before broadcasting"
    );

    // Send broadcast frame from RSU's TUN interface
    let broadcast_mac = [255u8; 6];
    let test_payload = b"RSU_BROADCAST_TO_ALL";
    let mut rsu_broadcast_frame = Vec::new();
    rsu_broadcast_frame.extend_from_slice(&broadcast_mac); // destination MAC
    rsu_broadcast_frame.extend_from_slice(&mac_rsu.bytes()); // source MAC
    rsu_broadcast_frame.extend_from_slice(test_payload); // payload

    tun_rsu_peer
        .send_all(&rsu_broadcast_frame)
        .await
        .expect("Failed to send broadcast from RSU");

    // Use a more robust waiting mechanism that properly yields control
    let mut attempts = 0;
    loop {
        tokio::time::advance(Duration::from_millis(100)).await;
        tokio::task::yield_now().await;

        let count = downstream_count.load(Ordering::SeqCst);
        if count >= 2 {
            break;
        }

        attempts += 1;
        if attempts > 20 {
            // 2 seconds total
            break;
        }
    }

    // Verify RSU generated individual downstream packets for each OBU
    let count = downstream_count.load(Ordering::SeqCst);

    // Should have generated exactly 2 downstream packets (one for each OBU)
    assert_eq!(
        count, 2,
        "RSU should generate exactly 2 downstream packets (one for each OBU) for RSU broadcast"
    );

    // Verify that captured packets contain individually encrypted broadcast data
    let captured = captured_packets.lock().unwrap();
    assert_eq!(
        captured.len(),
        2,
        "Should have captured exactly 2 downstream packets"
    );

    // All packets should come from RSU
    for (from_idx, packet_data) in &*captured {
        assert_eq!(*from_idx, 0, "All packets should come from RSU (index 0)");

        // Parse each downstream message to verify it contains encrypted data
        let msg = node_lib::messages::message::Message::try_from(packet_data.as_slice())
            .expect("Should be able to parse downstream message");

        if let node_lib::messages::packet_type::PacketType::Data(
            node_lib::messages::data::Data::Downstream(downstream),
        ) = msg.get_packet_type()
        {
            // The downstream data should be encrypted (different from original)
            assert_ne!(
                downstream.data(),
                &rsu_broadcast_frame,
                "Downstream data should be encrypted (different from original broadcast frame)"
            );

            // Verify the data is at least the expected size (original + encryption overhead)
            assert!(
                downstream.data().len() >= rsu_broadcast_frame.len(),
                "Encrypted data should be at least as large as original frame"
            );
        } else {
            panic!("Expected downstream data packet");
        }
    }

    // Verify each packet is uniquely encrypted (they should be different from each other)
    if captured.len() == 2 {
        let msg1 = node_lib::messages::message::Message::try_from(captured[0].1.as_slice())
            .expect("Should parse first message");
        let msg2 = node_lib::messages::message::Message::try_from(captured[1].1.as_slice())
            .expect("Should parse second message");

        if let (
            node_lib::messages::packet_type::PacketType::Data(
                node_lib::messages::data::Data::Downstream(downstream1),
            ),
            node_lib::messages::packet_type::PacketType::Data(
                node_lib::messages::data::Data::Downstream(downstream2),
            ),
        ) = (msg1.get_packet_type(), msg2.get_packet_type())
        {
            assert_ne!(
                downstream1.data(),
                downstream2.data(),
                "Each recipient should receive uniquely encrypted data"
            );
        }
    }
}
