use criterion::{black_box, criterion_group, criterion_main, Criterion};
use node_lib::control::obu::routing::Routing as ObuRouting;
use node_lib::control::rsu::routing::Routing as RsuRouting;
use node_lib::Args;
use mac_address::MacAddress;
use std::time::Instant;

fn bench_obu_get_route(c: &mut Criterion) {
    let args = Args {
        bind: String::default(),
        tap_name: None,
        ip: None,
        mtu: 1500,
        node_params: node_lib::args::NodeParameters {
            node_type: node_lib::args::NodeType::Obu,
            hello_history: 8,
            hello_periodicity: None,
        },
    };

    let boot = Instant::now();
    let mut routing = ObuRouting::new(&args, &boot).expect("build");

    // populate routing with many entries
    for i in 0..100u32 {
        let src: MacAddress = [i as u8; 6].into();
        let _ = routing.handle_heartbeat(&(&node_lib::messages::message::Message::new(
            src,
            [255u8;6].into(),
            node_lib::messages::packet_type::PacketType::Control(node_lib::messages::control::Control::Heartbeat(node_lib::messages::control::heartbeat::Heartbeat::new(std::time::Duration::from_millis(0), i, src.clone())))
        )), [9u8;6].into());
    }

    c.bench_function("obu_get_route_100", |b| {
        b.iter(|| {
            let _ = routing.get_route_to(black_box(Some(MacAddress::new([50u8;6]))));
        })
    });
}

fn bench_rsu_get_route(c: &mut Criterion) {
    let args = Args {
        bind: String::default(),
        tap_name: None,
        ip: None,
        mtu: 1500,
        node_params: node_lib::args::NodeParameters {
            node_type: node_lib::args::NodeType::Rsu,
            hello_history: 8,
            hello_periodicity: None,
        },
    };

    let boot = Instant::now();
    let mut routing = RsuRouting::new(&args).expect("build");

    for i in 0..100u32 {
        let src: MacAddress = [i as u8; 6].into();
        let _ = routing.send_heartbeat(src);
    }

    c.bench_function("rsu_get_route_100", |b| {
        b.iter(|| {
            let _ = routing.get_route_to(black_box(Some(MacAddress::new([50u8;6]))));
        })
    });
}

criterion_group!(benches, bench_obu_get_route, bench_rsu_get_route);
criterion_main!(benches);
