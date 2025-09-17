use criterion::{black_box, criterion_group, criterion_main, Criterion};
use mac_address::MacAddress;
use node_lib::messages::control::heartbeat::HeartbeatReply;
use node_lib::messages::{control::Control, message::Message, packet_type::PacketType};
use rsu_lib::args::{RsuArgs, RsuParameters};
use rsu_lib::control::routing::Routing;

fn bench_rsu_get_route(_c: &mut Criterion) {
    let args = RsuArgs {
        bind: String::default(),
        tap_name: None,
        ip: None,
        mtu: 1500,
        rsu_params: RsuParameters {
            hello_history: 16,
            hello_periodicity: 5000,
            cached_candidates: 4,
            enable_encryption: false,
        },
    };

    let mut routing = Routing::new(&args).expect("build");

    // populate with many heartbeat replies by sending heartbeats and constructing replies
    for i in 0..200u32 {
        let src: MacAddress = [i as u8; 6].into();
        let msg = routing.send_heartbeat([255u8; 6].into());

        // Extract heartbeat fields (copy out), then drop msg to release borrow on routing.
        if let PacketType::Control(Control::Heartbeat(hb)) = msg.get_packet_type() {
            let hb_id = hb.id();
            let hb_dur = hb.duration();
            let hb_src = hb.source();
            drop(msg);

            // Reconstruct a Heartbeat owned value and build a HeartbeatReply.
            use node_lib::messages::control::heartbeat::Heartbeat;
            let hb_owned = Heartbeat::new(hb_dur, hb_id, hb_src);
            let hbr: HeartbeatReply = HeartbeatReply::from_sender(&hb_owned, src);
            let reply = Message::new(
                [1u8; 6].into(),
                [255u8; 6].into(),
                PacketType::Control(Control::HeartbeatReply(hbr)),
            );
            let _ = routing.handle_heartbeat_reply(&reply, [1u8; 6].into());
        }
    }

    let mut cfg = Criterion::default()
        .measurement_time(std::time::Duration::from_secs(1))
        .warm_up_time(std::time::Duration::from_secs(1))
        .sample_size(10);

    cfg.bench_function("rsu_get_route_200", |b| {
        b.iter(|| {
            let _ = routing.get_route_to(black_box(Some(MacAddress::new([50u8; 6]))));
        })
    });
}

criterion_group!(get_route_group, bench_rsu_get_route);
criterion_main!(get_route_group);
