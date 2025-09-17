use criterion::{black_box, criterion_group, criterion_main, Criterion};
use node_lib::messages::message::Message;
use std::time::Duration;

fn bench_message_parse(_c: &mut Criterion) {
    // Build a representative serialized message buffer: to(6) + from(6) + marker + payload
    let pkt = [
        vec![1u8; 6],     // to
        vec![2u8; 6],     // from
        vec![0x30, 0x30], // protocol marker
        vec![0u8; 4],     // payload (minimal)
    ];

    let apkt: Vec<u8> = pkt.iter().flat_map(|x| x.iter()).cloned().collect();

    let mut short_cfg = Criterion::default()
        .measurement_time(Duration::from_secs(1))
        .warm_up_time(Duration::from_secs(1))
        .sample_size(10);

    short_cfg.bench_function("message_try_from", |b| {
        b.iter(|| {
            let _ = Message::try_from(black_box(&apkt[..]));
        })
    });
}

criterion_group!(message_parse_group, bench_message_parse);
criterion_main!(message_parse_group);
