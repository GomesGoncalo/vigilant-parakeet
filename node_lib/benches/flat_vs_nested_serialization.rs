// This benchmark tests flat serialization performance.
// Previously compared flat vs nested Vec<Vec<u8>> serialization,
// but nested format has been removed as part of performance optimization.
// Results showed 8.7x improvement with flat serialization:
// - Flat: 22.86ns
// - Nested: 198.06ns
// - 88.9% reduction in allocations (1 vs 9)

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use node_lib::messages::control::heartbeat::Heartbeat;
use node_lib::messages::control::Control;
use node_lib::messages::message::Message;
use node_lib::messages::packet_type::PacketType;
use std::time::Duration;

fn bench_flat_serialization(c: &mut Criterion) {
    let from = [0x02; 6].into();
    let to = [0x03; 6].into();
    let heartbeat = Heartbeat::new(Duration::from_secs(1), 42, [0x04; 6].into());
    let packet = PacketType::Control(Control::Heartbeat(heartbeat));
    let message = Message::new(from, to, packet);

    c.bench_function("serialize_flat", |b| {
        b.iter(|| {
            let flat: Vec<u8> = black_box(&message).into();
            black_box(flat);
        })
    });
}

criterion_group!(benches, bench_flat_serialization);
criterion_main!(benches);
