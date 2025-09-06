use node_lib::args::NodeType;
use node_lib::control::obu::Obu;
use node_lib::control::rsu::Rsu;
use node_lib::test_helpers::hub::HubCheck;
use node_lib::test_helpers::util::{
    await_condition_with_time_advance, mk_device_from_fd, mk_node_params, mk_shim_pairs,
};
use node_lib::Args;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

/// Common payload checker that can be used to inspect packets for test payloads
struct PayloadChecker {
    payload_found: Arc<AtomicBool>,
    test_payload: Vec<u8>,
    search_anywhere: bool, // If true, search anywhere in data; if false, search at offset 12
}

impl HubCheck for PayloadChecker {
    fn on_packet(&self, from_idx: usize, data: &[u8]) {
        // Check packets from OBU1 (index 1)
        if from_idx == 1 {
            if let Ok(msg) = node_lib::messages::message::Message::try_from(data) {
                if let node_lib::messages::packet_type::PacketType::Data(
                    node_lib::messages::data::Data::Upstream(upstream),
                ) = msg.get_packet_type()
                {
                    if self.search_anywhere {
                        // Search anywhere in the data for the payload (used for encryption tests)
                        if upstream
                            .data()
                            .windows(self.test_payload.len())
                            .any(|window| window == self.test_payload)
                        {
                            self.payload_found.store(true, Ordering::SeqCst);
                        }
                    } else {
                        // Search at specific offset 12 (used for plaintext tests)
                        if upstream.data().len() >= 12 + self.test_payload.len() {
                            let payload_part = &upstream.data()[12..];
                            if payload_part.starts_with(&self.test_payload) {
                                self.payload_found.store(true, Ordering::SeqCst);
                            }
                        }
                    }
                }
            }
        }
    }
}

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

    let inspector = Arc::new(PayloadChecker {
        payload_found: payload_inspected_clone,
        test_payload: test_payload_vec.clone(),
        search_anywhere: true, // Search anywhere in the data for encrypted payloads
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
    assert!(
        !payload_inspected.load(Ordering::SeqCst),
        "Intermediate OBU1 should not be able to read the encrypted payload"
    );

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

    let checker = Arc::new(PayloadChecker {
        payload_found: payload_seen_clone,
        test_payload: test_payload_vec.clone(),
        search_anywhere: false, // Search at offset 12 for plaintext payloads
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
    assert!(
        payload_seen.load(Ordering::SeqCst),
        "With encryption disabled, payload should be readable in transit"
    );
}

/// Helper function to create an ICMP ping packet (IPv4)
fn create_ping_packet(src_ip: [u8; 4], dst_ip: [u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut packet = Vec::new();

    // Ethernet frame (we'll skip this as it's handled by the MAC layer)
    // IPv4 header (20 bytes)
    packet.push(0x45); // Version (4) + IHL (5)
    packet.push(0x00); // Type of Service
    let total_length = 20 + 8 + payload.len(); // IP header + ICMP header + payload
    packet.extend_from_slice(&(total_length as u16).to_be_bytes()); // Total Length
    packet.extend_from_slice(&[0x12, 0x34]); // Identification
    packet.extend_from_slice(&[0x00, 0x00]); // Flags + Fragment Offset
    packet.push(0x40); // TTL
    packet.push(0x01); // Protocol (ICMP)
    packet.extend_from_slice(&[0x00, 0x00]); // Header Checksum (will calculate later)
    packet.extend_from_slice(&src_ip); // Source IP
    packet.extend_from_slice(&dst_ip); // Destination IP

    // Calculate IPv4 header checksum
    let checksum = calculate_checksum(&packet[0..20]);
    packet[10] = (checksum >> 8) as u8;
    packet[11] = (checksum & 0xff) as u8;

    // ICMP header (8 bytes)
    let icmp_start = packet.len();
    packet.push(0x08); // Type (Echo Request)
    packet.push(0x00); // Code
    packet.extend_from_slice(&[0x00, 0x00]); // Checksum (will calculate later)
    packet.extend_from_slice(&[0x12, 0x34]); // Identifier
    packet.extend_from_slice(&[0x00, 0x01]); // Sequence Number
    packet.extend_from_slice(payload); // Payload

    // Calculate ICMP checksum
    let icmp_checksum = calculate_checksum(&packet[icmp_start..]);
    packet[icmp_start + 2] = (icmp_checksum >> 8) as u8;
    packet[icmp_start + 3] = (icmp_checksum & 0xff) as u8;

    packet
}

/// Calculate Internet checksum (RFC 1071)
fn calculate_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;

    // Sum all 16-bit words
    for chunk in data.chunks(2) {
        if chunk.len() == 2 {
            sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
        } else {
            // Odd length, pad with zero
            sum += (chunk[0] as u32) << 8;
        }
    }

    // Add carry bits
    while (sum >> 16) != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }

    // One's complement
    !sum as u16
}

/// Test that verifies ping packets are encrypted and cannot be inspected by intermediate nodes,
/// but are correctly decrypted at the destination RSU.
/// Topology: OBU1 (sender) -> OBU2 (intermediate) -> RSU (destination)
#[tokio::test]
async fn test_ping_encryption_prevents_inspection_but_rsu_receives_correctly() {
    node_lib::init_test_tracing();
    tokio::time::pause();

    // Create 3 shim TUN pairs - we need the RSU peer to check what it receives
    let mut pairs = mk_shim_pairs(3);
    let (tun_rsu, tun_rsu_peer) = pairs.remove(0);
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

    // Create a ping packet
    let ping_payload = b"This is a ping payload that should be encrypted";
    let src_ip = [192, 168, 1, 10]; // OBU1 IP
    let dst_ip = [192, 168, 1, 1]; // RSU IP
    let ping_packet = create_ping_packet(src_ip, dst_ip, ping_payload);

    // Setup hub with delays that force OBU1 to route through OBU2
    // Make direct RSU->OBU1 path high latency (50ms) so OBU1 prefers two-hop via OBU2 (2+2=4ms)
    let delays: Vec<Vec<u64>> = vec![vec![0, 50, 2], vec![50, 0, 2], vec![2, 2, 0]];

    // Create a custom checker to inspect packets going through the intermediate OBU2
    let ping_content_found = Arc::new(AtomicBool::new(false));
    let ping_content_found_clone = ping_content_found.clone();

    let inspector = Arc::new(PayloadChecker {
        payload_found: ping_content_found_clone,
        test_payload: ping_payload.to_vec(),
        search_anywhere: true, // Search anywhere in the data for ping payload
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
    let obu1 = Obu::new(args_obu1, Arc::new(tun_obu1), Arc::new(dev_obu1)).unwrap();
    let _obu2 = Obu::new(args_obu2, Arc::new(tun_obu2), Arc::new(dev_obu2)).unwrap();

    // Wait for topology discovery - OBU1 should route through OBU2 to reach RSU
    tokio::time::advance(Duration::from_millis(500)).await;

    // Wait for OBU1 to discover upstream route through OBU2
    let result = await_condition_with_time_advance(
        Duration::from_millis(10),
        || {
            if let Some(mac) = obu1.cached_upstream_mac() {
                if mac == mac_obu2 {
                    return Some(mac);
                }
            }
            None
        },
        Duration::from_secs(5),
    )
    .await;

    assert!(result.is_ok(), "OBU1 should discover upstream through OBU2");

    // Create a frame with MAC headers to send the ping
    let mut frame = Vec::new();
    frame.extend_from_slice(&mac_rsu.bytes()); // destination MAC
    frame.extend_from_slice(&mac_obu1.bytes()); // source MAC
    frame.extend_from_slice(&ping_packet); // ping packet

    // Send the ping from OBU1
    tun_obu1_peer
        .send_all(&frame)
        .await
        .expect("Failed to send ping frame");

    // Give time for the frame to propagate through the network
    tokio::time::advance(Duration::from_millis(200)).await;

    // Verify that OBU2 (intermediate node) did not see the ping content
    assert!(
        !ping_content_found.load(Ordering::SeqCst),
        "Intermediate OBU2 should not be able to read the encrypted ping payload"
    );

    // Now verify that RSU received the ping correctly decrypted
    // The RSU should forward the decrypted payload to its TUN interface (without MAC headers)
    let mut rsu_recv_buf = vec![0u8; 1500];
    let received = node_lib::test_helpers::util::poll_tun_recv_with_timeout_mocked(
        &tun_rsu_peer,
        &mut rsu_recv_buf,
        50, // timeout per attempt
        20, // max attempts
    )
    .await;

    assert!(
        received.is_some(),
        "RSU should have received the ping packet"
    );
    let received_size = received.unwrap();
    let received_data = &rsu_recv_buf[..received_size];

    // The RSU forwards the full frame (including MAC headers) to TUN after decryption
    assert_eq!(
        received_data, &frame,
        "RSU should receive the decrypted frame (including MAC headers)"
    );

    // Additionally verify that the ping payload is present in what RSU received
    assert!(
        received_data
            .windows(ping_payload.len())
            .any(|window| window == ping_payload),
        "RSU should receive the original ping payload in plaintext"
    );
}
