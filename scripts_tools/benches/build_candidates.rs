use criterion::{criterion_group, criterion_main, Criterion};
use ipnetwork::Ipv4Network;

fn bench_build_candidates(_c: &mut Criterion) {
    let net = Ipv4Network::new("10.0.0.0".parse().unwrap(), 24).unwrap();
    let mut cfg = Criterion::default();
    cfg.bench_function("build_candidates_10_24", |b| {
        b.iter(|| {
            let _ = scripts_tools::autofix_configs::build_candidates(net, 10);
        })
    });
}

criterion_group!(build_candidates_group, bench_build_candidates);
criterion_main!(build_candidates_group);
