use mac_address::MacAddress;
use std::collections::HashMap;

/// Per-next-hop aggregated statistics used for scoring.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NextHopStats {
    pub min_us: u128,
    pub sum_us: u128,
    pub count: u32,
}

/// Given a map of next-hop -> NextHopStats, pick the best next hop by score = min + avg.
/// Returns (MacAddress, avg_us) or None when map is empty.
pub fn pick_best_next_hop(
    per_next: HashMap<MacAddress, NextHopStats>,
) -> Option<(MacAddress, u128)> {
    let mut best: Option<(u128, MacAddress, u128)> = None; // (score, mac, avg)

    for (mac, stats) in per_next.into_iter() {
        let avg_us = if stats.count > 0 {
            stats.sum_us / (stats.count as u128)
        } else {
            u128::MAX
        };
        let score = if stats.min_us == u128::MAX || avg_us == u128::MAX {
            u128::MAX
        } else {
            stats.min_us + avg_us
        };
        match &mut best {
            None => best = Some((score, mac, avg_us)),
            Some((bscore, bmac, bavg)) => {
                if score < *bscore || (score == *bscore && mac.bytes() < bmac.bytes()) {
                    *bscore = score;
                    *bmac = mac;
                    *bavg = avg_us;
                }
            }
        }
    }

    let (_score, mac, avg) = best?;
    Some((mac, avg))
}

/// Given a latency_candidates map (mac -> (min_us, sum_us, count, hops)),
/// compute (score=min+avg, hops, mac, avg) for each and return a sorted Vec
/// ordered by score then hops.
pub fn score_and_sort_latency_candidates(
    latency_candidates: HashMap<MacAddress, (u128, u128, u32, u32)>,
) -> Vec<(u128, u32, MacAddress, u128)> {
    let mut scored: Vec<(u128, u32, MacAddress, u128)> = Vec::new();
    for (mac, (min_us, sum_us, n, hops_val)) in latency_candidates.into_iter() {
        let avg_us = if n > 0 {
            sum_us / (n as u128)
        } else {
            u128::MAX
        };
        let score = if min_us == u128::MAX || avg_us == u128::MAX {
            u128::MAX
        } else {
            min_us + avg_us
        };
        scored.push((score, hops_val, mac, avg_us));
    }
    scored.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.cmp(&b.1))
            .then(a.2.bytes().cmp(&b.2.bytes()))
    });
    scored
}

/// Convenience wrapper: pick the best next hop directly from a
/// latency_candidates map (mac -> (min_us, sum_us, count, hops)).
/// Returns (MacAddress, avg_us) or None when empty.
pub fn pick_best_from_latency_candidates(
    latency_candidates: HashMap<MacAddress, (u128, u128, u32, u32)>,
) -> Option<(MacAddress, u128)> {
    let mut scored = score_and_sort_latency_candidates(latency_candidates);
    if scored.is_empty() {
        return None;
    }
    let (_score, _hops, mac, avg) = scored.remove(0);
    Some((mac, avg))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mac_address::MacAddress;

    #[test]
    fn pick_best_next_hop_tie_break() {
        let a: MacAddress = [1u8, 0, 0, 0, 0, 1].into();
        let b: MacAddress = [2u8, 0, 0, 0, 0, 2].into();
        let mut m = HashMap::new();
        m.insert(
            a,
            NextHopStats {
                min_us: 10,
                sum_us: 20,
                count: 2,
            },
        );
        m.insert(
            b,
            NextHopStats {
                min_us: 10,
                sum_us: 20,
                count: 2,
            },
        );
        let best = pick_best_next_hop(m).expect("some");
        // tie-break based on bytes -> a should be chosen
        assert_eq!(best.0.bytes(), a.bytes());
    }

    #[test]
    fn pick_best_next_hop_none_latency_handling() {
        let a: MacAddress = [5u8; 6].into();
        let b: MacAddress = [6u8; 6].into();
        let mut m = HashMap::new();
        m.insert(
            a,
            NextHopStats {
                min_us: u128::MAX,
                sum_us: 0,
                count: 0,
            },
        );
        m.insert(
            b,
            NextHopStats {
                min_us: 50,
                sum_us: 100,
                count: 2,
            },
        );
        let best = pick_best_next_hop(m).expect("some");
        assert_eq!(best.0, b);
    }

    #[test]
    fn pick_best_from_latency_candidates_tie_break() {
        let a: MacAddress = [1u8, 0, 0, 0, 0, 1].into();
        let b: MacAddress = [2u8, 0, 0, 0, 0, 2].into();
        let mut m = HashMap::new();
        // Both candidates have min=10, sum=20, n=2, hops=1 -> tie on score; a should win
        m.insert(a, (10u128, 20u128, 2u32, 1u32));
        m.insert(b, (10u128, 20u128, 2u32, 1u32));
        let best = super::pick_best_from_latency_candidates(m).expect("some");
        assert_eq!(best.0.bytes(), a.bytes());
    }

    #[test]
    fn pick_best_from_latency_candidates_none_latency_handling() {
        let a: MacAddress = [5u8; 6].into();
        let b: MacAddress = [6u8; 6].into();
        let mut m = HashMap::new();
        // a has no measurements (min=MAX, n=0), b has measurements -> b should be chosen
        m.insert(a, (u128::MAX, 0u128, 0u32, 1u32));
        m.insert(b, (50u128, 100u128, 2u32, 1u32));
        let best = super::pick_best_from_latency_candidates(m).expect("some");
        assert_eq!(best.0, b);
    }

    #[test]
    fn pick_best_from_latency_candidates_min_finite_avg_missing() {
        // Candidate `a` has a finite min but no avg measurements (count=0 -> avg=MAX)
        // Candidate `b` has measured avg and should win despite a larger min value.
        let a: MacAddress = [10u8; 6].into();
        let b: MacAddress = [11u8; 6].into();
        let mut m = HashMap::new();
        // a: min=1 but n=0 -> avg=MAX -> score = MAX
        m.insert(a, (1u128, 0u128, 0u32, 1u32));
        // b: min=100, sum=100, n=1 -> avg=100 -> score=200
        m.insert(b, (100u128, 100u128, 1u32, 1u32));
        let best = super::pick_best_from_latency_candidates(m).expect("some");
        assert_eq!(best.0, b);
    }

    #[test]
    fn score_and_sort_latency_candidates_hops_and_mac_tie_break() {
        // Create three candidates with equal score but different hops and MACs.
        // Expected sort order: candidates with fewer hops come first; among equal hops, lower MAC wins.
        let a: MacAddress = [1u8, 0, 0, 0, 0, 1].into(); // hops=2
        let b: MacAddress = [1u8, 0, 0, 0, 0, 2].into(); // hops=1 (lower MAC than c)
        let c: MacAddress = [1u8, 0, 0, 0, 0, 3].into(); // hops=1
        let mut m = HashMap::new();
        // All three will have score = 50 + 50 = 100
        m.insert(a, (50u128, 50u128, 1u32, 2u32));
        m.insert(b, (50u128, 50u128, 1u32, 1u32));
        m.insert(c, (50u128, 50u128, 1u32, 1u32));

        let ordered = super::score_and_sort_latency_candidates(m);
        // Expect first == b, second == c, third == a
        assert_eq!(ordered.len(), 3);
        assert_eq!(ordered[0].2.bytes(), b.bytes());
        assert_eq!(ordered[1].2.bytes(), c.bytes());
        assert_eq!(ordered[2].2.bytes(), a.bytes());
    }

    #[test]
    fn large_permutation_consistency() {
        use std::collections::HashMap;
        // Deterministic pseudo-random generator to build many candidates.
        let mut x: u64 = 1;
        const N: usize = 800;
        let mut m: HashMap<MacAddress, (u128, u128, u32, u32)> = HashMap::new();

        for i in 0..N {
            // simple LCG for deterministic variability
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
            let min = ((x >> 5) as u128 % 400).saturating_add(1); // 1..400
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
            let sum = (x >> 7) as u128 % 2000; // 0..1999
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
            let count = (x >> 11) as u32 % 6; // 0..5 (some zero counts)
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
            let hops = (x >> 13) as u32 % 8; // 0..7

            let ii = i as u64;
            let bytes = [
                (ii & 0xff) as u8,
                ((ii >> 8) & 0xff) as u8,
                ((ii >> 16) & 0xff) as u8,
                ((ii >> 24) & 0xff) as u8,
                ((ii >> 32) & 0xff) as u8,
                ((ii >> 40) & 0xff) as u8,
            ];
            let mac: MacAddress = bytes.into();

            m.insert(mac, (min, sum, count, hops));
        }

        // actual ordering from the helper
        let actual = super::score_and_sort_latency_candidates(m.clone());

        // build expected ordering using the same comparator logic
        let mut expected: Vec<(u128, u32, MacAddress, u128)> = m
            .into_iter()
            .map(|(mac, (min_us, sum_us, n, hops))| {
                let avg = if n > 0 {
                    sum_us / (n as u128)
                } else {
                    u128::MAX
                };
                let score = if min_us == u128::MAX || avg == u128::MAX {
                    u128::MAX
                } else {
                    min_us + avg
                };
                (score, hops, mac, avg)
            })
            .collect();

        expected.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then(a.1.cmp(&b.1))
                .then(a.2.bytes().cmp(&b.2.bytes()))
        });

        assert_eq!(actual.len(), expected.len());
        for (a, e) in actual.iter().zip(expected.iter()) {
            assert_eq!(a.0, e.0, "score mismatch");
            assert_eq!(a.1, e.1, "hops mismatch");
            assert_eq!(a.2.bytes(), e.2.bytes(), "mac mismatch");
            assert_eq!(a.3, e.3, "avg mismatch");
        }
    }

    // proptest fuzz: generate small maps and assert the helper ordering equals an explicit sort
    #[test]
    fn proptest_fuzz_small_maps() {
        use proptest::prelude::*;

        let strategy = prop::collection::hash_map(
            // generate small deterministic 6-byte arrays for MAC
            prop::array::uniform6(prop::num::u8::ANY),
            // tuple: min, sum, count, hops
            (0u128..1000u128, 0u128..2000u128, 0u32..5u32, 0u32..5u32),
            1..30usize,
        );

        proptest::proptest!(|(m in strategy)| {
            // convert keys to MacAddress
            let mapped: std::collections::HashMap<MacAddress, (u128, u128, u32, u32)> =
                m.into_iter().map(|(k, v)| (k.into(), v)).collect();

            let actual = super::score_and_sort_latency_candidates(mapped.clone());

            let mut expected: Vec<(u128, u32, MacAddress, u128)> = mapped
                .into_iter()
                .map(|(mac, (min_us, sum_us, n, hops))| {
                    let avg = if n > 0 { sum_us / (n as u128) } else { u128::MAX };
                    let score = if min_us == u128::MAX || avg == u128::MAX {
                        u128::MAX
                    } else {
                        min_us + avg
                    };
                    (score, hops, mac, avg)
                })
                .collect();

            expected.sort_by(|a, b| {
                a.0
                    .cmp(&b.0)
                    .then(a.1.cmp(&b.1))
                    .then(a.2.bytes().cmp(&b.2.bytes()))
            });

            prop_assert_eq!(actual.len(), expected.len());
            for (a, e) in actual.iter().zip(expected.iter()) {
                prop_assert_eq!(a.0, e.0);
                prop_assert_eq!(a.1, e.1);
                prop_assert_eq!(a.2.bytes(), e.2.bytes());
                prop_assert_eq!(a.3, e.3);
            }
        });
    }
}
