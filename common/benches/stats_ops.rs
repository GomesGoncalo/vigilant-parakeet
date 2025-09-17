// Enable these benches only when the `stats` feature is enabled.
// Criterion generates a `main` function via `criterion_main!` which must be
// defined at the crate root level. Do not wrap the macros in an inner module.
#[cfg(feature = "stats")]
use common::stats::Stats;
#[cfg(feature = "stats")]
use criterion::{black_box, criterion_group, criterion_main, Criterion};

#[cfg(feature = "stats")]
fn bench_stats_default(_c: &mut Criterion) {
    let mut cfg = Criterion::default();
    cfg.bench_function("stats_default", |b| {
        b.iter(|| {
            let s = Stats::default();
            black_box(s);
        })
    });
}

#[cfg(feature = "stats")]
criterion_group!(stats_ops_group, bench_stats_default);
#[cfg(feature = "stats")]
criterion_main!(stats_ops_group);
