use criterion::{black_box, criterion_group, criterion_main, Criterion};
use node_lib::messages::{message::Message, packet_type::PacketType};

fn bench_serialize_message(_c: &mut Criterion) {
    // Build a Message with a small payload and measure From<&Message> -> Vec<Vec<u8>> serialization
    let msg = Message::new(
        [1u8; 6].into(),
        [2u8; 6].into(),
        PacketType::Data(node_lib::messages::data::Data::Upstream(
            node_lib::messages::data::ToUpstream::new([3u8; 6].into(), &[1, 2, 3, 4]),
        )),
    );

    // Allow overriding measurement parameters via env vars for CI runs
    let meas_secs = std::env::var("CRITERION_MEASUREMENT_TIME")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(1);
    let sample_size = std::env::var("CRITERION_SAMPLE_SIZE")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(20);

    // Use a local Criterion config so the bench reliably registers and produces artifacts
    let mut cfg = Criterion::default()
        .measurement_time(std::time::Duration::from_secs(meas_secs))
        .warm_up_time(std::time::Duration::from_secs(1))
        .sample_size(sample_size);

    cfg.bench_function("serialize_message", |b| {
        b.iter(|| {
            let _v: Vec<Vec<u8>> = (&msg).into();
            black_box(_v);
        })
    });
}

criterion_group!(serialize_message_group, bench_serialize_message);
criterion_main!(serialize_message_group);
