use mac_address::MacAddress;
use std::collections::HashMap;

/// A candidate next-hop after scoring and sorting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScoredCandidate {
    /// Primary sort key: average latency in microseconds (u128::MAX when unmeasured).
    pub score_us: u128,
    /// Secondary sort key: hop count.
    pub hops: u32,
    /// Next-hop MAC address.
    pub mac: MacAddress,
    /// Average round-trip latency in microseconds.
    pub avg_us: u128,
}

/// Per-next-hop aggregated statistics used for scoring.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NextHopStats {
    pub min_us: u128,
    pub sum_us: u128,
    pub count: u32,
    /// Hop count for this next-hop candidate.
    pub hops: u32,
}

/// Given a map of next-hop -> NextHopStats, pick the best next hop by score = avg.
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
        let score = avg_us;
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

/// Given a map of next-hop -> `NextHopStats`, score each entry and return a
/// `Vec<ScoredCandidate>` sorted by score, then hops, then MAC.
pub fn score_and_sort_latency_candidates(
    candidates: HashMap<MacAddress, NextHopStats>,
) -> Vec<ScoredCandidate> {
    let mut scored: Vec<ScoredCandidate> = candidates
        .into_iter()
        .map(|(mac, stats)| {
            let avg_us = if stats.count > 0 {
                stats.sum_us / (stats.count as u128)
            } else {
                u128::MAX
            };
            ScoredCandidate {
                score_us: avg_us,
                hops: stats.hops,
                mac,
                avg_us,
            }
        })
        .collect();
    scored.sort_by(|a, b| {
        a.score_us
            .cmp(&b.score_us)
            .then(a.hops.cmp(&b.hops))
            .then(a.mac.bytes().cmp(&b.mac.bytes()))
    });
    scored
}

/// Pick the best next hop from a `NextHopStats` map.
/// Returns `(MacAddress, avg_us)` or `None` when the map is empty.
pub fn pick_best_from_latency_candidates(
    candidates: HashMap<MacAddress, NextHopStats>,
) -> Option<(MacAddress, u128)> {
    let scored = score_and_sort_latency_candidates(candidates);
    scored.first().map(|c| (c.mac, c.avg_us))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mac_address::MacAddress;

    fn stats(min_us: u128, sum_us: u128, count: u32, hops: u32) -> NextHopStats {
        NextHopStats {
            min_us,
            sum_us,
            count,
            hops,
        }
    }

    #[test]
    fn pick_best_next_hop_tie_break() {
        let a: MacAddress = [1u8, 0, 0, 0, 0, 1].into();
        let b: MacAddress = [2u8, 0, 0, 0, 0, 2].into();
        let mut m = HashMap::new();
        m.insert(a, stats(10, 20, 2, 1));
        m.insert(b, stats(10, 20, 2, 1));
        let best = pick_best_next_hop(m).expect("some");
        // tie-break based on bytes -> a should be chosen
        assert_eq!(best.0.bytes(), a.bytes());
    }

    #[test]
    fn pick_best_next_hop_none_latency_handling() {
        let a: MacAddress = [5u8; 6].into();
        let b: MacAddress = [6u8; 6].into();
        let mut m = HashMap::new();
        m.insert(a, stats(u128::MAX, 0, 0, 1));
        m.insert(b, stats(50, 100, 2, 1));
        let best = pick_best_next_hop(m).expect("some");
        assert_eq!(best.0, b);
    }

    #[test]
    fn pick_best_from_latency_candidates_tie_break() {
        let a: MacAddress = [1u8, 0, 0, 0, 0, 1].into();
        let b: MacAddress = [2u8, 0, 0, 0, 0, 2].into();
        let mut m = HashMap::new();
        // Both candidates: min=10, sum=20, n=2, hops=1 -> tie on score; a should win
        m.insert(a, stats(10, 20, 2, 1));
        m.insert(b, stats(10, 20, 2, 1));
        let best = super::pick_best_from_latency_candidates(m).expect("some");
        assert_eq!(best.0.bytes(), a.bytes());
    }

    #[test]
    fn pick_best_from_latency_candidates_none_latency_handling() {
        let a: MacAddress = [5u8; 6].into();
        let b: MacAddress = [6u8; 6].into();
        let mut m = HashMap::new();
        // a has no measurements (count=0 -> avg=MAX); b has measurements -> b wins
        m.insert(a, stats(u128::MAX, 0, 0, 1));
        m.insert(b, stats(50, 100, 2, 1));
        let best = super::pick_best_from_latency_candidates(m).expect("some");
        assert_eq!(best.0, b);
    }

    #[test]
    fn pick_best_from_latency_candidates_min_finite_avg_missing() {
        // a: min=1 but count=0 -> avg=MAX -> score=MAX
        // b: min=100, sum=100, count=1 -> avg=100 -> score=100; b wins
        let a: MacAddress = [10u8; 6].into();
        let b: MacAddress = [11u8; 6].into();
        let mut m = HashMap::new();
        m.insert(a, stats(1, 0, 0, 1));
        m.insert(b, stats(100, 100, 1, 1));
        let best = super::pick_best_from_latency_candidates(m).expect("some");
        assert_eq!(best.0, b);
    }

    #[test]
    fn score_and_sort_latency_candidates_hops_and_mac_tie_break() {
        // Three candidates with equal score but different hops and MACs.
        // Expected order: fewer hops first; among equal hops, lower MAC first.
        let a: MacAddress = [1u8, 0, 0, 0, 0, 1].into(); // hops=2
        let b: MacAddress = [1u8, 0, 0, 0, 0, 2].into(); // hops=1
        let c: MacAddress = [1u8, 0, 0, 0, 0, 3].into(); // hops=1
        let mut m = HashMap::new();
        m.insert(a, stats(50, 50, 1, 2));
        m.insert(b, stats(50, 50, 1, 1));
        m.insert(c, stats(50, 50, 1, 1));

        let ordered = super::score_and_sort_latency_candidates(m);
        assert_eq!(ordered.len(), 3);
        assert_eq!(ordered[0].mac.bytes(), b.bytes());
        assert_eq!(ordered[1].mac.bytes(), c.bytes());
        assert_eq!(ordered[2].mac.bytes(), a.bytes());
    }

    #[test]
    fn large_permutation_consistency() {
        use std::collections::HashMap;
        let mut x: u64 = 1;
        const N: usize = 800;
        let mut m: HashMap<MacAddress, NextHopStats> = HashMap::new();

        for i in 0..N {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
            let min_us = ((x >> 5) as u128 % 400).saturating_add(1);
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
            let sum_us = (x >> 7) as u128 % 2000;
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
            let count = (x >> 11) as u32 % 6;
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
            let hops = (x >> 13) as u32 % 8;

            let ii = i as u64;
            let mac: MacAddress = [
                (ii & 0xff) as u8,
                ((ii >> 8) & 0xff) as u8,
                ((ii >> 16) & 0xff) as u8,
                ((ii >> 24) & 0xff) as u8,
                ((ii >> 32) & 0xff) as u8,
                ((ii >> 40) & 0xff) as u8,
            ]
            .into();
            m.insert(
                mac,
                NextHopStats {
                    min_us,
                    sum_us,
                    count,
                    hops,
                },
            );
        }

        let actual = super::score_and_sort_latency_candidates(m.clone());

        let mut expected: Vec<ScoredCandidate> = m
            .into_iter()
            .map(|(mac, s)| {
                let avg_us = if s.count > 0 {
                    s.sum_us / (s.count as u128)
                } else {
                    u128::MAX
                };
                ScoredCandidate {
                    score_us: avg_us,
                    hops: s.hops,
                    mac,
                    avg_us,
                }
            })
            .collect();
        expected.sort_by(|a, b| {
            a.score_us
                .cmp(&b.score_us)
                .then(a.hops.cmp(&b.hops))
                .then(a.mac.bytes().cmp(&b.mac.bytes()))
        });

        assert_eq!(actual.len(), expected.len());
        for (a, e) in actual.iter().zip(expected.iter()) {
            assert_eq!(a.score_us, e.score_us, "score mismatch");
            assert_eq!(a.hops, e.hops, "hops mismatch");
            assert_eq!(a.mac.bytes(), e.mac.bytes(), "mac mismatch");
            assert_eq!(a.avg_us, e.avg_us, "avg mismatch");
        }
    }

    #[test]
    fn proptest_fuzz_small_maps() {
        use proptest::prelude::*;

        let strategy = prop::collection::hash_map(
            prop::array::uniform6(prop::num::u8::ANY),
            (0u128..1000u128, 0u128..2000u128, 0u32..5u32, 0u32..5u32),
            1..30usize,
        );

        proptest::proptest!(|(m in strategy)| {
            let mapped: std::collections::HashMap<MacAddress, NextHopStats> = m
                .into_iter()
                .map(|(k, (min_us, sum_us, count, hops))| {
                    (k.into(), NextHopStats { min_us, sum_us, count, hops })
                })
                .collect();

            let actual = super::score_and_sort_latency_candidates(mapped.clone());

            let mut expected: Vec<ScoredCandidate> = mapped
                .into_iter()
                .map(|(mac, s)| {
                    let avg_us =
                        if s.count > 0 { s.sum_us / (s.count as u128) } else { u128::MAX };
                    ScoredCandidate { score_us: avg_us, hops: s.hops, mac, avg_us }
                })
                .collect();
            expected.sort_by(|a, b| {
                a.score_us
                    .cmp(&b.score_us)
                    .then(a.hops.cmp(&b.hops))
                    .then(a.mac.bytes().cmp(&b.mac.bytes()))
            });

            prop_assert_eq!(actual.len(), expected.len());
            for (a, e) in actual.iter().zip(expected.iter()) {
                prop_assert_eq!(a.score_us, e.score_us);
                prop_assert_eq!(a.hops, e.hops);
                prop_assert_eq!(a.mac.bytes(), e.mac.bytes());
                prop_assert_eq!(a.avg_us, e.avg_us);
            }
        });
    }
}
