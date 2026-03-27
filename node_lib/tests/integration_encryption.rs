/// VANET-level encryption integration tests.
///
/// These tests verify that OBU nodes encrypt their TAP frames before
/// transmitting over the VANET, so intermediate relay nodes cannot read
/// the plaintext payload.
///
/// All tests use mocked time and the in-process Hub — no real UDP sockets.
/// End-to-end tests involving the Server node live in server_lib/tests/.
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

/// HubCheck that scans every VANET packet for a target byte sequence.
struct PayloadChecker {
    payload_found: Arc<AtomicBool>,
    test_payload: Vec<u8>,
}

impl HubCheck for PayloadChecker {
    fn on_packet(&self, from_idx: usize, data: &[u8]) {
        // Only inspect packets originating from OBU nodes (index > 0).
        if from_idx == 0 {
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

/// Verify that OBU encryption prevents intermediate nodes from reading the payload.
///
/// Topology: RSU ─ OBU1 ─ OBU2
/// OBU2 routes through OBU1.  The HubCheck inspector sits at the wire level
/// and should NOT see the plaintext payload in any VANET packet.
#[tokio::test]
async fn test_payload_encryption_prevents_inspection() {
    node_lib::init_test_tracing();
    tokio::time::pause();

    let mut pairs = mk_shim_pairs(2);
    let (tun_obu1, _tun_obu1_peer) = pairs.remove(0);
    let (tun_obu2, tun_obu2_peer) = pairs.remove(0);

    let (node_fds_v, hub_fds_v) =
        node_lib::test_helpers::util::mk_socketpairs(3).expect("mk_socketpairs failed");

    let mac_rsu: mac_address::MacAddress = [1, 2, 3, 4, 5, 6].into();
    let mac_obu1: mac_address::MacAddress = [10, 11, 12, 13, 14, 15].into();
    let mac_obu2: mac_address::MacAddress = [20, 21, 22, 23, 24, 25].into();

    let dev_rsu = mk_device_from_fd(mac_rsu, node_fds_v[0]);
    let dev_obu1 = mk_device_from_fd(mac_obu1, node_fds_v[1]);
    let dev_obu2 = mk_device_from_fd(mac_obu2, node_fds_v[2]);

    // RSU-OBU1: 2ms, OBU1-OBU2: 2ms, RSU-OBU2: 100ms — forces OBU2→OBU1→RSU path.
    let delays: Vec<Vec<u64>> = vec![vec![0, 2, 100], vec![2, 0, 2], vec![100, 2, 0]];

    let test_payload = b"secret data should not be readable by OBU1";
    let payload_found = Arc::new(AtomicBool::new(false));
    let inspector = Arc::new(PayloadChecker {
        payload_found: payload_found.clone(),
        test_payload: test_payload.to_vec(),
    });

    node_lib::test_helpers::util::mk_hub_with_checks_mocked_time(
        hub_fds_v,
        delays,
        vec![inspector],
    );

    // RSU no longer takes a TUN device; it is a transparent relay only.
    let _rsu = Rsu::new(mk_rsu_args(100), Arc::new(dev_rsu), "test_rsu".to_string()).unwrap();
    let _obu1 = Obu::new(
        mk_obu_args_encrypted(),
        Arc::new(tun_obu1),
        Arc::new(dev_obu1),
        "test_obu1".to_string(),
    )
    .unwrap();
    let obu2 = Obu::new(
        mk_obu_args_encrypted(),
        Arc::new(tun_obu2),
        Arc::new(dev_obu2),
        "test_obu2".to_string(),
    )
    .unwrap();

    tokio::time::advance(Duration::from_millis(500)).await;

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

    // Inject a TAP frame with the secret payload from OBU2.
    let mut frame = Vec::new();
    frame.extend_from_slice(&mac_rsu.bytes());
    frame.extend_from_slice(&mac_obu2.bytes());
    frame.extend_from_slice(test_payload);

    tun_obu2_peer
        .send_all(&frame)
        .await
        .expect("Failed to send test frame");

    tokio::time::advance(Duration::from_millis(200)).await;

    // The plaintext secret must not appear in any VANET packet.
    assert!(
        !payload_found.load(Ordering::SeqCst),
        "Intermediate OBU1 should not be able to read the encrypted payload"
    );
}

/// Verify that disabling encryption leaves the payload visible in transit.
///
/// When encryption is off the VANET packet carries the plaintext, so the
/// HubCheck inspector SHOULD find it.
#[tokio::test]
async fn test_encryption_disabled_allows_inspection() {
    node_lib::init_test_tracing();
    tokio::time::pause();

    let mut pairs = mk_shim_pairs(1);
    let (tun_obu1, tun_obu1_peer) = pairs.remove(0);

    let (node_fds_v, hub_fds_v) =
        node_lib::test_helpers::util::mk_socketpairs(2).expect("mk_socketpairs failed");

    let mac_rsu: mac_address::MacAddress = [1, 2, 3, 4, 5, 6].into();
    let mac_obu1: mac_address::MacAddress = [10, 11, 12, 13, 14, 15].into();

    let dev_rsu = mk_device_from_fd(mac_rsu, node_fds_v[0]);
    let dev_obu1 = mk_device_from_fd(mac_obu1, node_fds_v[1]);

    let delays: Vec<Vec<u64>> = vec![vec![0, 2], vec![2, 0]];

    let test_payload = b"readable data";
    let payload_found = Arc::new(AtomicBool::new(false));
    let checker = Arc::new(PayloadChecker {
        payload_found: payload_found.clone(),
        test_payload: test_payload.to_vec(),
    });

    node_lib::test_helpers::util::mk_hub_with_checks_mocked_time(hub_fds_v, delays, vec![checker]);

    let _rsu = Rsu::new(mk_rsu_args(100), Arc::new(dev_rsu), "test_rsu".to_string()).unwrap();
    let obu1 = Obu::new(
        mk_obu_args(), // encryption disabled
        Arc::new(tun_obu1),
        Arc::new(dev_obu1),
        "test_obu1".to_string(),
    )
    .unwrap();

    tokio::time::advance(Duration::from_millis(200)).await;

    let result = await_condition_with_time_advance(
        Duration::from_millis(10),
        || obu1.cached_upstream_mac(),
        Duration::from_secs(2),
    )
    .await;
    assert!(result.is_ok(), "OBU1 should discover upstream");

    tokio::time::advance(Duration::from_millis(500)).await;

    let mut frame = Vec::new();
    frame.extend_from_slice(&mac_rsu.bytes());
    frame.extend_from_slice(&mac_obu1.bytes());
    frame.extend_from_slice(test_payload);

    tun_obu1_peer
        .send_all(&frame)
        .await
        .expect("Failed to send test frame");

    advance_until(
        || payload_found.load(Ordering::SeqCst),
        Duration::from_millis(1),
        Duration::from_millis(10),
    )
    .await;

    assert!(
        payload_found.load(Ordering::SeqCst),
        "With encryption disabled, payload should be readable in transit"
    );
}

/// Verify that RSU relays encrypted VANET packets opaquely — it cannot read
/// the payload even though it forwards the frame.
#[tokio::test]
async fn test_ping_encryption_prevents_rsu_inspection() {
    node_lib::init_test_tracing();
    tokio::time::pause();

    let mut pairs = mk_shim_pairs(2);
    let (tun_obu1, _tun_obu1_peer) = pairs.remove(0);
    let (tun_obu2, tun_obu2_peer) = pairs.remove(0);

    let (node_fds_v, hub_fds_v) =
        node_lib::test_helpers::util::mk_socketpairs(3).expect("mk_socketpairs failed");

    let mac_rsu: mac_address::MacAddress = [1, 2, 3, 4, 5, 6].into();
    let mac_obu1: mac_address::MacAddress = [10, 11, 12, 13, 14, 15].into();
    let mac_obu2: mac_address::MacAddress = [20, 21, 22, 23, 24, 25].into();

    let dev_rsu = mk_device_from_fd(mac_rsu, node_fds_v[0]);
    let dev_obu1 = mk_device_from_fd(mac_obu1, node_fds_v[1]);
    let dev_obu2 = mk_device_from_fd(mac_obu2, node_fds_v[2]);

    // OBU2 routes through RSU (not OBU1): RSU-OBU2: 4ms, RSU-OBU1: 2ms, OBU1-OBU2: 50ms.
    let delays: Vec<Vec<u64>> = vec![vec![0, 2, 4], vec![2, 0, 50], vec![4, 50, 0]];

    let ping_payload = b"This is a ping payload that should be encrypted";
    let payload_found = Arc::new(AtomicBool::new(false));
    let inspector = Arc::new(PayloadChecker {
        payload_found: payload_found.clone(),
        test_payload: ping_payload.to_vec(),
    });

    node_lib::test_helpers::util::mk_hub_with_checks_mocked_time(
        hub_fds_v,
        delays,
        vec![inspector],
    );

    let _rsu = Rsu::new(mk_rsu_args(100), Arc::new(dev_rsu), "test_rsu".to_string()).unwrap();
    let _obu1 = Obu::new(
        mk_obu_args_encrypted(),
        Arc::new(tun_obu1),
        Arc::new(dev_obu1),
        "test_obu1".to_string(),
    )
    .unwrap();
    let obu2 = Obu::new(
        mk_obu_args_encrypted(),
        Arc::new(tun_obu2),
        Arc::new(dev_obu2),
        "test_obu2".to_string(),
    )
    .unwrap();

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

    let mut frame = Vec::new();
    frame.extend_from_slice(&mac_obu1.bytes()); // to OBU1
    frame.extend_from_slice(&mac_obu2.bytes()); // from OBU2
    frame.extend_from_slice(ping_payload);

    tun_obu2_peer
        .send_all(&frame)
        .await
        .expect("Failed to send ping frame");

    tokio::time::advance(Duration::from_millis(200)).await;

    assert!(
        !payload_found.load(Ordering::SeqCst),
        "RSU should not be able to read the encrypted ping payload while relaying it"
    );
}
