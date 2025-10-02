// Benchmark for batch vs individual packet processing
//
// This benchmark measures the performance difference between sending packets
// individually vs in batches using vectored I/O.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use node_lib::messages::control::heartbeat::Heartbeat;
use node_lib::messages::control::Control;
use node_lib::messages::message::Message;
use node_lib::messages::packet_type::PacketType;
use std::io::IoSlice;
use std::time::Duration;

fn create_test_packet(id: u32) -> Vec<u8> {
    let from = [0x02; 6].into();
    let to = [0x03; 6].into();
    let heartbeat = Heartbeat::new(Duration::from_secs(1), id, [0x04; 6].into());
    let packet = PacketType::Control(Control::Heartbeat(heartbeat));
    let message = Message::new(from, to, packet);
    (&message).into()
}

fn bench_individual_serialization(c: &mut Criterion) {
    let mut group = c.benchmark_group("packet_serialization");

    for count in [1, 4, 8, 16, 32].iter() {
        group.throughput(Throughput::Elements(*count as u64));
        group.bench_with_input(BenchmarkId::new("individual", count), count, |b, &count| {
            b.iter(|| {
                let mut packets = Vec::new();
                for i in 0..count {
                    let packet = create_test_packet(i as u32);
                    packets.push(black_box(packet));
                }
                packets
            });
        });
    }

    group.finish();
}

fn bench_batch_io_slice_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("io_slice_creation");

    for count in [1, 4, 8, 16, 32].iter() {
        // Pre-create packets
        let packets: Vec<Vec<u8>> = (0..*count).map(|i| create_test_packet(i as u32)).collect();

        group.throughput(Throughput::Elements(*count as u64));
        group.bench_with_input(BenchmarkId::new("batch", count), count, |b, _| {
            b.iter(|| {
                let slices: Vec<IoSlice> =
                    packets.iter().map(|p| IoSlice::new(black_box(p))).collect();
                black_box(slices)
            });
        });
    }

    group.finish();
}

fn bench_batch_size_overhead(c: &mut Criterion) {
    let mut group = c.benchmark_group("batch_overhead");

    // Create test packets
    let packets: Vec<Vec<u8>> = (0..32).map(create_test_packet).collect();

    for batch_size in [1, 2, 4, 8, 16, 32].iter() {
        group.throughput(Throughput::Elements(*batch_size as u64));
        group.bench_with_input(
            BenchmarkId::new("slice_creation", batch_size),
            batch_size,
            |b, &size| {
                b.iter(|| {
                    let batch = &packets[..size];
                    let slices: Vec<IoSlice> =
                        batch.iter().map(|p| IoSlice::new(black_box(p))).collect();
                    black_box(slices)
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_individual_serialization,
    bench_batch_io_slice_creation,
    bench_batch_size_overhead
);
criterion_main!(benches);
