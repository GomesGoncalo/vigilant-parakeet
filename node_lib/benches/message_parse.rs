use criterion::{black_box, criterion_group, criterion_main, Criterion};
use node_lib::messages::message::Message;
use std::time::Duration;

fn bench_message_parse(_c: &mut Criterion) {
    // build a representative message buffer (heartbeat-like)
    let pkt = vec![
        vec![1u8; 6],
        vec![2u8; 6],
        vec![0x30, 0x30],
        vec![0],
        vec![0],
        vec![4; 16],
        vec![0; 4],
        vec![1u8, 1u8, 1u8, 2u8],
        vec![2u8; 6],
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

criterion_group!(benches, bench_message_parse);
criterion_main!(benches);
