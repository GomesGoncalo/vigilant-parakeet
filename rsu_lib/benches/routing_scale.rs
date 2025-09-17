use criterion::{black_box, criterion_group, criterion_main, Criterion};
use mac_address::MacAddress;
use rsu_lib::args::{RsuArgs, RsuParameters};
use rsu_lib::control::routing::Routing;

fn bench_routing_scale(c: &mut Criterion) {
    let args = RsuArgs {
        bind: String::default(),
        tap_name: None,
        ip: None,
        mtu: 1500,
        rsu_params: RsuParameters {
            hello_history: 256,
            hello_periodicity: 5000,
            cached_candidates: 64,
            enable_encryption: false,
        },
    };

    // Build routing with many entries
    let mut routing = Routing::new(&args).expect("build");
    for _i in 0..1024u32 {
        let _ = routing.send_heartbeat([255u8; 6].into());
    }

    let meas_secs = std::env::var("CRITERION_MEASUREMENT_TIME")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(1);
    let sample_size = std::env::var("CRITERION_SAMPLE_SIZE")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(20);

    let mut group = c.benchmark_group("routing_get_route_group");
    group.measurement_time(std::time::Duration::from_secs(meas_secs));
    group.warm_up_time(std::time::Duration::from_secs(1));
    group.sample_size(sample_size);
    group.bench_function("routing_get_route_1024", |b| {
        b.iter(|| {
            let _ = routing.get_route_to(black_box(Some(MacAddress::new([50u8; 6]))));
        })
    });
    group.finish();
}

criterion_group!(routing_scale_group, bench_routing_scale);
criterion_main!(routing_scale_group);
