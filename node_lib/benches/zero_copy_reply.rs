use criterion::{black_box, Criterion};
use mac_address::MacAddress;
use node_lib::messages::{
    control::{heartbeat::Heartbeat, heartbeat::HeartbeatReply, Control},
    message::Message,
    packet_type::PacketType,
};
use std::time::Duration;

fn bench_zero_copy_reply(_c: &mut Criterion) {
    // Create a sample heartbeat packet
    let hb_wire = {
        let hb = Heartbeat::new(Duration::from_millis(100), 42, [1u8; 6].into());
        let msg = Message::new(
            [1u8; 6].into(),
            [255u8; 6].into(),
            PacketType::Control(Control::Heartbeat(hb)),
        );
        let wire: Vec<u8> = (&msg).into();
        wire
    };

    // Parse it once to get a borrowed Heartbeat
    let parsed_msg = Message::try_from(&hb_wire[..]).expect("parse");
    let hb = match parsed_msg.get_packet_type() {
        PacketType::Control(Control::Heartbeat(h)) => h,
        _ => panic!("wrong type"),
    };

    let sender: MacAddress = [9u8; 6].into();
    let from: MacAddress = [9u8; 6].into();
    let to: MacAddress = [1u8; 6].into();

    let mut cfg = Criterion::default()
        .measurement_time(Duration::from_secs(2))
        .warm_up_time(Duration::from_secs(1))
        .sample_size(100);

    // Benchmark 1: Traditional approach with intermediate allocations
    cfg.bench_function("heartbeat_reply_traditional", |b| {
        b.iter(|| {
            let reply = HeartbeatReply::from_sender(black_box(hb), black_box(sender));
            let msg = Message::new(
                black_box(from),
                black_box(to),
                PacketType::Control(Control::HeartbeatReply(reply)),
            );
            let wire: Vec<u8> = (&msg).into();
            black_box(wire)
        })
    });

    // Benchmark 2: Zero-copy reply construction (HeartbeatReply level)
    cfg.bench_function("heartbeat_reply_zero_copy_partial", |b| {
        let mut buf = Vec::with_capacity(36);
        b.iter(|| {
            HeartbeatReply::serialize_from_heartbeat_into(
                black_box(hb),
                black_box(sender),
                black_box(&mut buf),
            );
            // Still need to wrap in Message
            let msg = Message::new(
                black_box(from),
                black_box(to),
                PacketType::Control(Control::HeartbeatReply(HeartbeatReply::from_sender(
                    hb, sender,
                ))),
            );
            let wire: Vec<u8> = (&msg).into();
            black_box(wire)
        })
    });

    // Benchmark 3: Full zero-copy path (Message level)
    cfg.bench_function("heartbeat_reply_zero_copy_full", |b| {
        let mut buf = Vec::with_capacity(52);
        b.iter(|| {
            let size = Message::serialize_heartbeat_reply_into(
                black_box(hb),
                black_box(sender),
                black_box(from),
                black_box(to),
                black_box(&mut buf),
            );
            black_box(size)
        })
    });
}

criterion::criterion_group!(benches, bench_zero_copy_reply);
criterion::criterion_main!(benches);
