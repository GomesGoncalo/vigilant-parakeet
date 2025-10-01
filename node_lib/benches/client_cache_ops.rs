use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use mac_address::MacAddress;
use node_lib::control::client_cache::ClientCache;

fn bench_cache_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("client_cache_insert");

    for size in [10, 50, 100, 500].iter() {
        group.throughput(Throughput::Elements(*size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            b.iter(|| {
                let cache = ClientCache::new();
                for i in 0..size {
                    let client: MacAddress = [i as u8, 0, 0, 0, 0, 0].into();
                    let node: MacAddress = [0, i as u8, 0, 0, 0, 0].into();
                    cache.store_mac(black_box(client), black_box(node));
                }
            });
        });
    }
    group.finish();
}

fn bench_cache_update_existing(c: &mut Criterion) {
    let mut group = c.benchmark_group("client_cache_update_existing");

    let cache = ClientCache::new();
    let client: MacAddress = [1, 2, 3, 4, 5, 6].into();
    let node: MacAddress = [6, 5, 4, 3, 2, 1].into();

    // Pre-populate cache
    cache.store_mac(client, node);

    group.bench_function("update_same_value", |b| {
        b.iter(|| {
            // This should be fast due to early return optimization
            cache.store_mac(black_box(client), black_box(node));
        });
    });

    group.finish();
}

fn bench_cache_update_different_value(c: &mut Criterion) {
    let mut group = c.benchmark_group("client_cache_update_different");

    let cache = ClientCache::new();
    let client: MacAddress = [1, 2, 3, 4, 5, 6].into();
    let node1: MacAddress = [6, 5, 4, 3, 2, 1].into();
    let node2: MacAddress = [7, 8, 9, 10, 11, 12].into();

    // Pre-populate cache
    cache.store_mac(client, node1);

    group.bench_function("update_new_value", |b| {
        let mut toggle = false;
        b.iter(|| {
            let node = if toggle { node1 } else { node2 };
            cache.store_mac(black_box(client), black_box(node));
            toggle = !toggle;
        });
    });

    group.finish();
}

fn bench_cache_get(c: &mut Criterion) {
    let mut group = c.benchmark_group("client_cache_get");

    let cache = ClientCache::new();

    // Pre-populate with multiple entries
    for i in 0..100 {
        let client: MacAddress = [i, 0, 0, 0, 0, 0].into();
        let node: MacAddress = [0, i, 0, 0, 0, 0].into();
        cache.store_mac(client, node);
    }

    let test_client: MacAddress = [50, 0, 0, 0, 0, 0].into();

    group.bench_function("get_existing", |b| {
        b.iter(|| cache.get(black_box(test_client)));
    });

    let nonexistent: MacAddress = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff].into();
    group.bench_function("get_nonexistent", |b| {
        b.iter(|| cache.get(black_box(nonexistent)));
    });

    group.finish();
}

fn bench_cache_concurrent_reads(c: &mut Criterion) {
    use std::sync::Arc;
    use std::thread;

    let mut group = c.benchmark_group("client_cache_concurrent");

    group.bench_function("parallel_reads", |b| {
        let cache = Arc::new(ClientCache::new());

        // Pre-populate
        for i in 0..100 {
            let client: MacAddress = [i, 0, 0, 0, 0, 0].into();
            let node: MacAddress = [0, i, 0, 0, 0, 0].into();
            cache.store_mac(client, node);
        }

        b.iter(|| {
            let mut handles = vec![];

            for t in 0..4 {
                let cache_clone = Arc::clone(&cache);
                let handle = thread::spawn(move || {
                    let client: MacAddress = [(t * 25) as u8, 0, 0, 0, 0, 0].into();
                    for _ in 0..25 {
                        black_box(cache_clone.get(black_box(client)));
                    }
                });
                handles.push(handle);
            }

            for handle in handles {
                handle.join().unwrap();
            }
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_cache_insert,
    bench_cache_update_existing,
    bench_cache_update_different_value,
    bench_cache_get,
    bench_cache_concurrent_reads
);
criterion_main!(benches);
