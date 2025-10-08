use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

use mac_address::MacAddress;
use node_lib::test_helpers::hub::HubCheck;
use node_lib::test_helpers::util::{
    mk_device_from_fd, mk_hub_with_checks_mocked_time, mk_shim_pair, mk_socketpairs,
};
use obu_lib::test_helpers::mk_obu_args;
use obu_lib::Obu;

use node_lib::messages::control::heartbeat::{Heartbeat, HeartbeatReply};
use node_lib::messages::{control::Control, message::Message, packet_type::PacketType};

struct ReplyForwardCheck {
    idx: usize,
    expected_to: MacAddress,
    saw: Arc<AtomicBool>,
}

impl HubCheck for ReplyForwardCheck {
    fn on_packet(&self, from_idx: usize, data: &[u8]) {
        if from_idx != self.idx {
            return;
        }
        if let Ok(msg) = Message::try_from(data) {
            if let PacketType::Control(Control::HeartbeatReply(_)) = msg.get_packet_type() {
                if let Ok(to) = msg.to() {
                    if to == self.expected_to {
                        self.saw.store(true, Ordering::SeqCst);
                    }
                }
            }
        }
    }
}

/// Helper to send raw bytes into the node-side socket (so the Hub will read them).
fn send_on_node_fd(fd: i32, data: &[u8]) {
    unsafe {
        let _ = libc::send(fd, data.as_ptr() as *const _, data.len(), 0);
    }
}

#[tokio::test]
async fn hub_reproduce_loop_case_one() {
    node_lib::init_test_tracing();
    tokio::time::pause();

    // Create 2 socketpairs: index 0 = FA (external), 1 = OBU
    let (node_fds_v, hub_fds_v) = mk_socketpairs(2).expect("mk_socketpairs failed");
    let node_fds = [node_fds_v[0], node_fds_v[1]];
    let hub_fds = [hub_fds_v[0], hub_fds_v[1]];

    // Small delays (0ms) so delivery is immediate when we advance time
    let delays: Vec<Vec<u64>> = vec![vec![0, 0], vec![0, 0]];

    // We'll observe whether the OBU forwards a HeartbeatReply to FA (it should NOT).
    let saw = Arc::new(AtomicBool::new(false));
    let check = ReplyForwardCheck {
        idx: 1, // Hub will observe packets from the OBU at index 1
        expected_to: [0xFA, 0x2A, 0x13, 0x98, 0x32, 0xD1].into(),
        saw: saw.clone(),
    };

    mk_hub_with_checks_mocked_time(hub_fds.to_vec(), delays, vec![Arc::new(check)]);

    // Create an OBU device on node_fds[1]
    let mac_obu: MacAddress = [0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F].into();
    let dev_obu = mk_device_from_fd(mac_obu, node_fds[1]);

    // Create a shim TUN for OBU
    let (tun_obu, _peer) = mk_shim_pair();

    // Build and start OBU
    let args_obu = mk_obu_args();
    let _obu = Obu::new(args_obu, Arc::new(tun_obu), Arc::new(dev_obu), "test_obu".to_string()).expect("Obu::new failed");

    // Allow tasks to start
    tokio::time::advance(Duration::from_millis(10)).await;

    // Compose Heartbeat: source = 2E:D9:12:10:9F:47, pkt.from = FA:2A:13:98:32:D1, seq=0
    let source: MacAddress = [0x2E, 0xD9, 0x12, 0x10, 0x9F, 0x47].into();
    let fa: MacAddress = [0xFA, 0x2A, 0x13, 0x98, 0x32, 0xD1].into();
    let hb = Heartbeat::new(std::time::Duration::from_millis(1), 0u32, source);
    let hb_msg = Message::new(
        fa,
        [255u8; 6].into(),
        PacketType::Control(Control::Heartbeat(hb.clone())),
    );
    let hb_wire: Vec<u8> = (&hb_msg).into();

    // Send heartbeat from FA into the hub (write to node side so hub reads it)
    send_on_node_fd(node_fds[0], &hb_wire);
    // Advance time to deliver and process
    tokio::time::advance(Duration::from_millis(20)).await;

    // Now craft a HeartbeatReply reported sender == FA and send it from FA as well
    let hbr = HeartbeatReply::from_sender(&hb, fa);
    let reply_msg = Message::new(
        fa,
        [255u8; 6].into(),
        PacketType::Control(Control::HeartbeatReply(hbr.clone())),
    );
    let reply_wire: Vec<u8> = (&reply_msg).into();
    send_on_node_fd(node_fds[0], &reply_wire);

    // Advance time to allow OBU to process and (not) forward
    tokio::time::advance(Duration::from_millis(20)).await;

    // Assert OBU did NOT forward the reply to FA (loop detected -> no forward)
    assert!(
        !saw.load(Ordering::SeqCst),
        "OBU forwarded HeartbeatReply unexpectedly (case one)"
    );
}

#[tokio::test]
async fn hub_reproduce_loop_case_two() {
    node_lib::init_test_tracing();
    tokio::time::pause();

    // Create 2 socketpairs: index 0 = forwarder (A2..), 1 = OBU
    let (node_fds_v, hub_fds_v) = mk_socketpairs(2).expect("mk_socketpairs failed");
    let node_fds = [node_fds_v[0], node_fds_v[1]];
    let hub_fds = [hub_fds_v[0], hub_fds_v[1]];

    let delays: Vec<Vec<u64>> = vec![vec![0, 0], vec![0, 0]];

    let saw = Arc::new(AtomicBool::new(false));
    let check = ReplyForwardCheck {
        idx: 1,
        expected_to: [0x86, 0x96, 0x4D, 0x03, 0x16, 0xDC].into(),
        saw: saw.clone(),
    };
    mk_hub_with_checks_mocked_time(hub_fds.to_vec(), delays, vec![Arc::new(check)]);

    // Create OBU on index 1
    let mac_obu: MacAddress = [0x0C, 0x0D, 0x0E, 0x0F, 0x10, 0x11].into();
    let dev_obu = mk_device_from_fd(mac_obu, node_fds[1]);
    let (tun_obu, _peer) = mk_shim_pair();
    let args_obu = mk_obu_args();
    let _obu = Obu::new(args_obu, Arc::new(tun_obu), Arc::new(dev_obu), "test_obu2".to_string()).expect("Obu::new failed");

    tokio::time::advance(Duration::from_millis(10)).await;

    // Heartbeat observed via next_up = 86:96:4D:03:16:DC
    let source: MacAddress = [0x2E, 0xD9, 0x12, 0x10, 0x9F, 0x47].into();
    let next_up: MacAddress = [0x86, 0x96, 0x4D, 0x03, 0x16, 0xDC].into();
    let forwarder: MacAddress = [0xA2, 0xB9, 0x44, 0x12, 0x56, 0x2B].into();

    let hb = Heartbeat::new(std::time::Duration::from_millis(1), 0u32, source);
    let hb_msg = Message::new(
        next_up,
        [255u8; 6].into(),
        PacketType::Control(Control::Heartbeat(hb.clone())),
    );
    let hb_wire: Vec<u8> = (&hb_msg).into();
    send_on_node_fd(node_fds[0], &hb_wire);
    tokio::time::advance(Duration::from_millis(20)).await;

    // Reply where message.sender == next_up but pkt.from == forwarder
    let hbr = HeartbeatReply::from_sender(&hb, next_up);
    let reply_msg = Message::new(
        forwarder,
        [255u8; 6].into(),
        PacketType::Control(Control::HeartbeatReply(hbr.clone())),
    );
    let reply_wire: Vec<u8> = (&reply_msg).into();
    send_on_node_fd(node_fds[0], &reply_wire);
    tokio::time::advance(Duration::from_millis(20)).await;

    assert!(
        !saw.load(Ordering::SeqCst),
        "OBU forwarded HeartbeatReply unexpectedly (case two)"
    );
}
