use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use mac_address::MacAddress;
use node_lib::messages::{
    data::{Data, ToDownstream, ToUpstream},
    message::Message,
    packet_type::PacketType,
};

fn bench_upstream_forward(c: &mut Criterion) {
    let mut group = c.benchmark_group("upstream_forward");

    let origin: MacAddress = [1u8; 6].into();
    let payload = b"test payload for upstream message";
    let parsed = ToUpstream::new(origin, payload);
    let from: MacAddress = [2u8; 6].into();
    let to: MacAddress = [3u8; 6].into();

    group.bench_function("traditional", |b| {
        b.iter(|| {
            let msg = Message::new(
                black_box(from),
                black_box(to),
                PacketType::Data(Data::Upstream(black_box(parsed.clone()))),
            );
            let wire: Vec<u8> = (&msg).into();
            black_box(wire);
        });
    });

    group.bench_function("zero_copy", |b| {
        let mut buf = Vec::new();
        b.iter(|| {
            Message::serialize_upstream_forward_into(
                black_box(&parsed),
                black_box(from),
                black_box(to),
                &mut buf,
            );
            black_box(&buf);
        });
    });

    group.finish();
}

fn bench_downstream_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("downstream_creation");

    let origin = [4u8; 6];
    let destination: MacAddress = [5u8; 6].into();
    let payload = b"test payload for downstream message creation";
    let from: MacAddress = [2u8; 6].into();
    let to: MacAddress = [3u8; 6].into();

    group.bench_function("traditional", |b| {
        b.iter(|| {
            let td = ToDownstream::new(
                black_box(&origin),
                black_box(destination),
                black_box(payload),
            );
            let msg = Message::new(
                black_box(from),
                black_box(to),
                PacketType::Data(Data::Downstream(td)),
            );
            let wire: Vec<u8> = (&msg).into();
            black_box(wire);
        });
    });

    group.bench_function("zero_copy", |b| {
        let mut buf = Vec::new();
        b.iter(|| {
            Message::serialize_downstream_into(
                black_box(&origin),
                black_box(destination),
                black_box(payload),
                black_box(from),
                black_box(to),
                &mut buf,
            );
            black_box(&buf);
        });
    });

    group.finish();
}

fn bench_downstream_forward(c: &mut Criterion) {
    let mut group = c.benchmark_group("downstream_forward");

    let origin = [6u8; 6];
    let destination: MacAddress = [7u8; 6].into();
    let payload = b"test payload for downstream forwarding";
    let parsed = ToDownstream::new(&origin, destination, payload);
    let from: MacAddress = [2u8; 6].into();
    let to: MacAddress = [3u8; 6].into();

    group.bench_function("traditional", |b| {
        b.iter(|| {
            let msg = Message::new(
                black_box(from),
                black_box(to),
                PacketType::Data(Data::Downstream(black_box(parsed.clone()))),
            );
            let wire: Vec<u8> = (&msg).into();
            black_box(wire);
        });
    });

    group.bench_function("zero_copy", |b| {
        let mut buf = Vec::new();
        b.iter(|| {
            Message::serialize_downstream_forward_into(
                black_box(&parsed),
                black_box(from),
                black_box(to),
                &mut buf,
            );
            black_box(&buf);
        });
    });

    group.finish();
}

fn bench_all_data_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("data_operations_comparison");

    // Setup data for all three scenarios
    let origin_mac: MacAddress = [1u8; 6].into();
    let payload = b"benchmark payload";
    let parsed_upstream = ToUpstream::new(origin_mac, payload);

    let origin_bytes = [4u8; 6];
    let destination: MacAddress = [5u8; 6].into();
    let parsed_downstream = ToDownstream::new(&origin_bytes, destination, payload);

    let from: MacAddress = [2u8; 6].into();
    let to: MacAddress = [3u8; 6].into();

    // Upstream forward
    group.bench_with_input(
        BenchmarkId::new("upstream_forward", "traditional"),
        &(&parsed_upstream, from, to),
        |b, (p, f, t)| {
            b.iter(|| {
                let msg = Message::new(*f, *t, PacketType::Data(Data::Upstream((*p).clone())));
                let wire: Vec<u8> = (&msg).into();
                black_box(wire);
            });
        },
    );

    group.bench_with_input(
        BenchmarkId::new("upstream_forward", "zero_copy"),
        &(&parsed_upstream, from, to),
        |b, (p, f, t)| {
            let mut buf = Vec::new();
            b.iter(|| {
                Message::serialize_upstream_forward_into(p, *f, *t, &mut buf);
                black_box(&buf);
            });
        },
    );

    // Downstream creation
    group.bench_with_input(
        BenchmarkId::new("downstream_creation", "traditional"),
        &(&origin_bytes, destination, payload, from, to),
        |b, (o, d, p, f, t)| {
            b.iter(|| {
                let td = ToDownstream::new(*o, *d, *p);
                let msg = Message::new(*f, *t, PacketType::Data(Data::Downstream(td)));
                let wire: Vec<u8> = (&msg).into();
                black_box(wire);
            });
        },
    );

    group.bench_with_input(
        BenchmarkId::new("downstream_creation", "zero_copy"),
        &(&origin_bytes, destination, payload, from, to),
        |b, (o, d, p, f, t)| {
            let mut buf = Vec::new();
            b.iter(|| {
                Message::serialize_downstream_into(*o, *d, *p, *f, *t, &mut buf);
                black_box(&buf);
            });
        },
    );

    // Downstream forward
    group.bench_with_input(
        BenchmarkId::new("downstream_forward", "traditional"),
        &(&parsed_downstream, from, to),
        |b, (p, f, t)| {
            b.iter(|| {
                let msg = Message::new(*f, *t, PacketType::Data(Data::Downstream((*p).clone())));
                let wire: Vec<u8> = (&msg).into();
                black_box(wire);
            });
        },
    );

    group.bench_with_input(
        BenchmarkId::new("downstream_forward", "zero_copy"),
        &(&parsed_downstream, from, to),
        |b, (p, f, t)| {
            let mut buf = Vec::new();
            b.iter(|| {
                Message::serialize_downstream_forward_into(p, *f, *t, &mut buf);
                black_box(&buf);
            });
        },
    );

    group.finish();
}

criterion_group!(
    benches,
    bench_upstream_forward,
    bench_downstream_creation,
    bench_downstream_forward,
    bench_all_data_operations
);
criterion_main!(benches);
