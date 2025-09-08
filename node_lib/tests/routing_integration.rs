use mac_address::MacAddress;
use node_lib::control::{obu, rsu};
use node_lib::messages::control::heartbeat::{Heartbeat, HeartbeatReply};
use node_lib::messages::control::Control;
use node_lib::messages::message::Message;
use node_lib::messages::packet_type::PacketType;
use node_lib::Args;
use std::time::Duration;
use tokio::time::Instant;

#[test]
fn obu_and_rsu_choose_same_next_hop_for_same_messages() {
    // Build Args for both
    let args_obu = Args {
        bind: String::default(),
        tap_name: None,
        ip: None,
        mtu: 1500,
        node_params: node_lib::args::NodeParameters {
            node_type: node_lib::args::NodeType::Obu,
            hello_history: 2,
            hello_periodicity: None,
            cached_candidates: 3,
            enable_encryption: false,
                server_address: None,
        },
    };
    let args_rsu = Args {
        bind: String::default(),
        tap_name: None,
        ip: None,
        mtu: 1500,
        node_params: node_lib::args::NodeParameters {
            node_type: node_lib::args::NodeType::Rsu,
            hello_history: 2,
            hello_periodicity: None,
            cached_candidates: 3,
            enable_encryption: false,
                server_address: None,
        },
    };

    let boot = Instant::now();
    let mut obu_routing = obu::routing::Routing::new(&args_obu, &boot).expect("obu routing");
    let mut rsu_routing = rsu::routing::Routing::new(&args_rsu).expect("rsu routing");

    // Setup a heartbeat from rsu_src observed via pkt_from. We'll then craft a reply
    // from reply_sender forwarded by reply_from so both routings record the same info.
    let rsu_src: MacAddress = [10u8; 6].into();
    let pkt_from: MacAddress = [11u8; 6].into();
    let reply_sender: MacAddress = [20u8; 6].into();
    let reply_from: MacAddress = [21u8; 6].into();

    // Create heartbeat and corresponding messages
    let hb = Heartbeat::new(Duration::from_millis(0), 0u32, rsu_src);
    let hb_msg = Message::new(
        pkt_from,
        [255u8; 6].into(),
        PacketType::Control(Control::Heartbeat(hb.clone())),
    );

    // Feed heartbeat into obu routing (simulates observing the hello)
    let _ = obu_routing
        .handle_heartbeat(&hb_msg, [99u8; 6].into())
        .expect("handled obu hb");

    // For RSU routing, use send_heartbeat (creates internal sent state)
    let _ = rsu_routing.send_heartbeat(rsu_src);

    // Create a heartbeat reply where HeartbeatReply::sender() is reply_sender
    let hbr = HeartbeatReply::from_sender(&hb, reply_sender);
    let reply_msg = Message::new(
        reply_from,
        [255u8; 6].into(),
        PacketType::Control(Control::HeartbeatReply(hbr.clone())),
    );

    // Feed reply into both routings
    let _ = obu_routing
        .handle_heartbeat_reply(&reply_msg, [99u8; 6].into())
        .expect("obu handled reply");
    let _ = rsu_routing
        .handle_heartbeat_reply(&reply_msg, [99u8; 6].into())
        .expect("rsu handled reply");

    // Now query both for a route to reply_sender and assert next hop chosen is the same
    let obu_route = obu_routing
        .get_route_to(Some(reply_sender))
        .expect("obu route");
    let rsu_route = rsu_routing
        .get_route_to(Some(reply_sender))
        .expect("rsu route");

    assert_eq!(obu_route.mac, rsu_route.mac);
}
