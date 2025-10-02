//! Failover and Candidate Management Tests
//!
//! Tests for failover logic and candidate list management including
//! rebuild from latency observations and hops-only backfill.

use super::super::routing::{Routing, Target};
use crate::args::{ObuArgs, ObuParameters};
use indexmap::IndexMap;
use mac_address::MacAddress;
use std::collections::HashMap;
use std::time::Duration;
use tokio::time::Instant;

// Ensure failover_cached_upstream can rebuild candidate list from latency observations
#[test]
fn failover_rebuilds_candidates_from_latency() {
    let args = ObuArgs {
        bind: String::new(),
        tap_name: None,
        ip: None,
        mtu: 1500,
        obu_params: ObuParameters {
            hello_history: 2,
            cached_candidates: 3,
            enable_encryption: false,
        },
    };

    let boot = Instant::now();
    let mut routing = Routing::new(&args, &boot).expect("routing built");

    // Prepare an RSU entry whose downstream map contains latencyed observations
    let src: MacAddress = [9u8; 6].into(); // cached_source we want rebuilt for
    let rsu: MacAddress = [100u8; 6].into();
    let mut seqmap = IndexMap::new();
    let mut downstream_map: HashMap<MacAddress, Vec<Target>> = HashMap::new();

    // Two candidate next-hops with measured latencies; the lower-latency one should be preferred
    downstream_map.insert(
        src,
        vec![
            Target {
                hops: 1,
                mac: [1u8; 6].into(),
                latency: Some(Duration::from_millis(20)),
            },
            Target {
                hops: 1,
                mac: [2u8; 6].into(),
                latency: Some(Duration::from_millis(5)),
            },
        ],
    );

    seqmap.insert(
        0u32,
        (
            Duration::from_millis(0),
            rsu,
            1u32,
            IndexMap::new(),
            downstream_map,
        ),
    );
    routing.routes.insert(rsu, seqmap);

    // Ensure cached_candidates empty so failover path rebuilds
    routing.cache.clear();
    routing.cache.set_upstream([99u8; 6].into(), src); // Set a source for rebuild

    // Call failover; this should rebuild candidates and return a primary
    let promoted = routing.failover_cached_upstream();
    assert!(promoted.is_some());
    // After rebuild, cached_candidates should be populated
    let cands = routing.get_cached_candidates();
    assert!(cands.is_some());
    let c = cands.unwrap();
    assert!(!c.is_empty());
}

// Ensure select_and_cache_upstream backfills using hops-only when no latency data
#[test]
fn select_and_cache_upstream_backfills_by_hops() {
    let args = ObuArgs {
        bind: String::new(),
        tap_name: None,
        ip: None,
        mtu: 1500,
        obu_params: ObuParameters {
            hello_history: 2,
            cached_candidates: 3,
            enable_encryption: false,
        },
    };

    let boot = Instant::now();
    let mut routing = Routing::new(&args, &boot).expect("routing built");

    // Create two upstream routes (no latency measurements) with different hop counts
    let rsu: MacAddress = [200u8; 6].into();
    let mut seqmap = IndexMap::new();

    // target is the RSU itself here; populate upstream entries for this RSU
    let via_a: MacAddress = [11u8; 6].into();
    let via_b: MacAddress = [12u8; 6].into();

    // No downstream entries required for hops-only selection; instead insert upstream_routes
    seqmap.insert(
        1u32,
        (
            Duration::from_millis(0),
            via_a,
            2u32, // hops
            IndexMap::new(),
            HashMap::new(),
        ),
    );
    seqmap.insert(
        2u32,
        (
            Duration::from_millis(0),
            via_b,
            1u32, // fewer hops, should be preferred
            IndexMap::new(),
            HashMap::new(),
        ),
    );
    routing.routes.insert(rsu, seqmap);

    // Now call select_and_cache_upstream and expect cached_candidates to include via_b first
    let selected = routing
        .select_and_cache_upstream(rsu)
        .expect("selected route");
    let cached = routing.get_cached_candidates().expect("cands");
    assert!(!cached.is_empty());
    // First candidate should be the one with fewer hops (via_b)
    assert_eq!(cached[0], via_b);
    // And selected primary should match
    assert_eq!(selected.mac, cached[0]);
}

#[test]
fn test_set_cached_candidates_clears_and_sets() {
    let args = ObuArgs {
        bind: String::new(),
        tap_name: None,
        ip: None,
        mtu: 1500,
        obu_params: ObuParameters {
            hello_history: 2,
            cached_candidates: 3,
            enable_encryption: false,
        },
    };

    let boot = Instant::now();
    let routing = Routing::new(&args, &boot).expect("routing built");

    // Start with empty candidates
    routing.test_set_cached_candidates(vec![]);
    assert!(routing.get_cached_candidates().is_none());
    assert!(routing.get_cached_upstream().is_none());

    // Set some candidates
    let a: MacAddress = [1u8; 6].into();
    let b: MacAddress = [2u8; 6].into();
    routing.test_set_cached_candidates(vec![a, b]);
    let c = routing.get_cached_candidates().expect("cands");
    assert_eq!(c[0], a);
    assert_eq!(routing.get_cached_upstream().expect("primary"), a);
}

#[test]
fn failover_backfills_from_hops_when_no_latency() {
    let args = ObuArgs {
        bind: String::new(),
        tap_name: None,
        ip: None,
        mtu: 1500,
        obu_params: ObuParameters {
            hello_history: 2,
            cached_candidates: 3,
            enable_encryption: false,
        },
    };

    let boot = Instant::now();
    let mut routing = Routing::new(&args, &boot).expect("routing built");

    // Build routes where only hop-count information is available
    let src: MacAddress = [9u8; 6].into();
    // Use src as the RSU key so the hops-based backfill path finds entries
    let rsu: MacAddress = src;
    let via_a: MacAddress = [11u8; 6].into();
    let via_b: MacAddress = [12u8; 6].into();
    let mut seqmap = IndexMap::new();

    seqmap.insert(
        1u32,
        (
            Duration::from_millis(0),
            via_a,
            3u32, // more hops
            IndexMap::new(),
            HashMap::new(),
        ),
    );
    seqmap.insert(
        2u32,
        (
            Duration::from_millis(0),
            via_b,
            1u32, // fewer hops, should be preferred
            IndexMap::new(),
            HashMap::new(),
        ),
    );
    routing.routes.insert(rsu, seqmap);

    // Set cached source so failover can rebuild
    routing.cache.clear();
    routing.cache.set_upstream([99u8; 6].into(), src);

    let promoted = routing.failover_cached_upstream();
    // Should have promoted something
    assert!(promoted.is_some());
    let primary = routing.get_cached_upstream().expect("primary");
    // promoted should match the cached upstream
    assert_eq!(primary, promoted.unwrap());
    // And the primary should be one of the candidates we inserted
    let cands = routing.get_cached_candidates().expect("cands");
    assert!(cands.contains(&primary));
}
