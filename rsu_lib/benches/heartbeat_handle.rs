use criterion::{black_box, criterion_group, criterion_main, Criterion};
use node_lib::messages::control::heartbeat::HeartbeatReply;
use node_lib::messages::{control::Control, message::Message, packet_type::PacketType};
use rsu_lib::args::{RsuArgs, RsuParameters};
use rsu_lib::control::routing::Routing;

fn bench_handle_heartbeat(_c: &mut Criterion) {
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

    // Prepare a HeartbeatReply message to feed into handle_heartbeat_reply
    // create a Heartbeat reply by sending a heartbeat and copying its fields, then dropping the message
    // Build a heartbeat reply to feed the handler
    let hb_msg = routing.send_heartbeat([255u8; 6].into());
    use node_lib::messages::control::heartbeat::Heartbeat;

    // Create an owned Heartbeat that will live for the duration of this bench.
    let mut hb_owned = Heartbeat::new(std::time::Duration::from_secs(0), 0, [0u8; 6].into());
    if let PacketType::Control(Control::Heartbeat(hb)) = hb_msg.get_packet_type() {
        let hb_id = hb.id();
        let hb_dur = hb.duration();
        let hb_src = hb.source();
        drop(hb_msg);

        hb_owned = Heartbeat::new(hb_dur, hb_id, hb_src);
    }

    let hbr = HeartbeatReply::from_sender(&hb_owned, [9u8; 6].into());
    let reply = Message::new(
        [9u8; 6].into(),
        [255u8; 6].into(),
        PacketType::Control(Control::HeartbeatReply(hbr)),
    );

    let meas_secs = std::env::var("CRITERION_MEASUREMENT_TIME")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(1);
    let sample_size = std::env::var("CRITERION_SAMPLE_SIZE")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(20);

    let mut cfg = Criterion::default()
        .measurement_time(std::time::Duration::from_secs(meas_secs))
        .warm_up_time(std::time::Duration::from_secs(1))
        .sample_size(sample_size);

    cfg.bench_function("handle_heartbeat_reply", |b| {
        b.iter(|| {
            let _ = routing.handle_heartbeat_reply(black_box(&reply), [1u8; 6].into());
        })
    });
}

criterion_group!(heartbeat_handle_group, bench_handle_heartbeat);
criterion_main!(heartbeat_handle_group);
