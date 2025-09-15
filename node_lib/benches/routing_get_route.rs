use criterion::{black_box, criterion_group, criterion_main, Criterion};
use mac_address::MacAddress;
use obu_lib::control::routing::Routing as ObuRouting;
use obu_lib::{ObuArgs, ObuParameters};
use rsu_lib::control::routing::Routing as RsuRouting;
use rsu_lib::{RsuArgs, RsuParameters};
use tokio::time::Instant;

fn bench_obu_get_route(_c: &mut Criterion) {
    let obu_args = ObuArgs {
        bind: String::default(),
        tap_name: None,
        ip: None,
        mtu: 1500,
        obu_params: ObuParameters {
            hello_history: 8,
            cached_candidates: 3,
            enable_encryption: false,
        },
    };

    let boot = Instant::now();
    let mut routing = ObuRouting::new(&obu_args, &boot).expect("build");

    // populate routing with many entries
    for i in 0..100u32 {
        let src: MacAddress = [i as u8; 6].into();
        let msg = node_lib::messages::message::Message::new(
            src,
            [255u8; 6].into(),
            node_lib::messages::packet_type::PacketType::Control(
                node_lib::messages::control::Control::Heartbeat(
                    node_lib::messages::control::heartbeat::Heartbeat::new(
                        std::time::Duration::from_millis(0),
                        i,
                        src,
                    ),
                ),
            ),
        );

        let _ = routing.handle_heartbeat(&msg, [9u8; 6].into());
    }

    let mut short_cfg = Criterion::default()
        .measurement_time(std::time::Duration::from_secs(1))
        .warm_up_time(std::time::Duration::from_secs(1))
        .sample_size(10);

    short_cfg.bench_function("obu_get_route_100", |b| {
        b.iter(|| {
            let _ = routing.get_route_to(black_box(Some(MacAddress::new([50u8; 6]))));
        })
    });
}

fn bench_rsu_get_route(_c: &mut Criterion) {
    let rsu_args = RsuArgs {
        bind: String::default(),
        tap_name: None,
        ip: None,
        mtu: 1500,
        rsu_params: RsuParameters {
            hello_history: 8,
            hello_periodicity: 5000,
            cached_candidates: 3,
            enable_encryption: false,
        },
    };

    let _boot = Instant::now();
    let mut routing = RsuRouting::new(&rsu_args).expect("build");

    for i in 0..100u32 {
        let src: MacAddress = [i as u8; 6].into();
        let _ = routing.send_heartbeat(src);
    }

    let mut short_cfg = Criterion::default()
        .measurement_time(std::time::Duration::from_secs(1))
        .warm_up_time(std::time::Duration::from_secs(1))
        .sample_size(10);

    short_cfg.bench_function("rsu_get_route_100", |b| {
        b.iter(|| {
            let _ = routing.get_route_to(black_box(Some(MacAddress::new([50u8; 6]))));
        })
    });
}

criterion_group!(benches, bench_obu_get_route, bench_rsu_get_route);
criterion_main!(benches);
