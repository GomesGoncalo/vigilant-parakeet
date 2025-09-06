use node_lib::args::NodeType;
use node_lib::control::obu::Obu;
use node_lib::control::rsu::Rsu;
use node_lib::test_helpers::hub::HubCheck;
use node_lib::test_helpers::util::{
    await_condition_with_time_advance, mk_device_from_fd, mk_node_params, mk_shim_pairs,
};
use node_lib::Args;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

/// Simple broadcast checker for debugging
struct SimpleBroadcastChecker {
    rsu_downstream_count: Arc<AtomicUsize>,
}

impl HubCheck for SimpleBroadcastChecker {
    fn on_packet(&self, from_idx: usize, data: &[u8]) {
        // Count downstream packets from RSU (index 0)
        if from_idx == 0 {
            if let Ok(msg) = node_lib::messages::message::Message::try_from(data) {
                if let node_lib::messages::packet_type::PacketType::Data(
                    node_lib::messages::data::Data::Downstream(_),
                ) = msg.get_packet_type()
                {
                    println!("RSU sent downstream packet");
                    self.rsu_downstream_count.fetch_add(1, Ordering::SeqCst);
                }
            }
        }
    }
}

/// Simple test to verify broadcast functionality works without encryption
#[tokio::test]
async fn test_simple_broadcast_no_encryption() {
    node_lib::init_test_tracing();
    tokio::time::pause();

    // Create 3 shim TUN pairs
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

    // Setup hub with low latency
    let delays: Vec<Vec<u64>> = vec![vec![0, 2, 2], vec![2, 0, 4], vec![2, 4, 0]];

    // Create a simple checker
    let downstream_count = Arc::new(AtomicUsize::new(0));
    let checker = Arc::new(SimpleBroadcastChecker {
        rsu_downstream_count: downstream_count.clone(),
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
        node_params: mk_node_params(NodeType::Rsu, Some(100)),
    };

    let args_obu1 = Args {
        bind: String::from("unused"),
        tap_name: None,
        ip: None,
        mtu: 1500,
        node_params: mk_node_params(NodeType::Obu, None),
    };

    let args_obu2 = Args {
        bind: String::from("unused"),
        tap_name: None,
        ip: None,
        mtu: 1500,
        node_params: mk_node_params(NodeType::Obu, None),
    };

    // Create nodes
    let _rsu = Rsu::new(args_rsu, Arc::new(tun_rsu), Arc::new(dev_rsu)).unwrap();
    let obu1 = Obu::new(args_obu1, Arc::new(tun_obu1), Arc::new(dev_obu1)).unwrap();
    let obu2 = Obu::new(args_obu2, Arc::new(tun_obu2), Arc::new(dev_obu2)).unwrap();

    // Wait for topology discovery 
    tokio::time::advance(Duration::from_millis(500)).await;

    // Wait for OBUs to discover upstream to RSU
    for (name, obu) in [("OBU1", &obu1), ("OBU2", &obu2)] {
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
    tokio::time::advance(Duration::from_millis(1000)).await;

    println!("Topology established, sending broadcast frame");

    // Verify RSU has routes to OBUs before testing broadcast
    // This is just for debugging
    tokio::time::advance(Duration::from_millis(100)).await;

    // Create a broadcast frame
    let broadcast_mac = [255u8; 6]; 
    let mut broadcast_frame = Vec::new();
    broadcast_frame.extend_from_slice(&broadcast_mac); // destination MAC (broadcast)
    broadcast_frame.extend_from_slice(&mac_obu1.bytes()); // source MAC
    broadcast_frame.extend_from_slice(b"test_broadcast_data"); // payload

    // Send broadcast frame from OBU1
    tun_obu1_peer
        .send_all(&broadcast_frame)
        .await
        .expect("Failed to send broadcast frame");

    println!("Broadcast frame sent, waiting for distribution");

    // Give time for the broadcast to propagate
    tokio::time::advance(Duration::from_millis(1000)).await;

    // Check if RSU sent any downstream packets
    let count = downstream_count.load(Ordering::SeqCst);
    println!("RSU sent {} downstream packets", count);
    
    // Should be 1 for OBU2 (not back to OBU1 as it's the sender)
    assert_eq!(count, 1, "RSU should send broadcast to 1 other OBU (OBU2)");
}