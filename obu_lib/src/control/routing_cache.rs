// Cache management for routing

use arc_swap::ArcSwapOption;
use mac_address::MacAddress;
use std::sync::Arc;

/// Cache manager for upstream routing decisions and failover candidates
#[derive(Debug)]
pub(crate) struct RoutingCache {
    /// Primary cached upstream (first candidate)
    cached_upstream: ArcSwapOption<MacAddress>,
    /// Last source MAC for which we selected/cached an upstream (e.g., RSU MAC)
    cached_source: ArcSwapOption<MacAddress>,
    /// Ordered list of N-best candidate upstreams for fast failover
    cached_candidates: ArcSwapOption<Vec<MacAddress>>,
    /// Configuration parameters
    cached_candidates_count: u32,
}

impl RoutingCache {
    pub(crate) fn new(cached_candidates_count: u32) -> Self {
        Self {
            cached_upstream: ArcSwapOption::from(None),
            cached_source: ArcSwapOption::from(None),
            cached_candidates: ArcSwapOption::from(None),
            cached_candidates_count,
        }
    }

    /// Return the cached upstream MAC if present.
    pub(crate) fn get_cached_upstream(&self) -> Option<MacAddress> {
        self.cached_upstream.load().as_ref().map(|m| **m)
    }

    /// Return the last source MAC we cached upstream for.
    #[allow(dead_code)]
    pub(crate) fn get_cached_source(&self) -> Option<MacAddress> {
        self.cached_source.load().as_ref().map(|m| **m)
    }

    /// Clear the cached upstream (useful when topology changes).
    pub(crate) fn clear(&self) {
        self.cached_upstream.store(None);
        self.cached_candidates.store(None);
        #[cfg(feature = "stats")]
        crate::metrics::inc_cache_clear();
    }

    /// Return the ordered cached candidates (primary first) when present.
    pub(crate) fn get_cached_candidates(&self) -> Option<Vec<MacAddress>> {
        self.cached_candidates
            .load()
            .as_ref()
            .map(|arcv| (**arcv).clone())
    }

    /// Set the primary cached upstream and the source it's for.
    pub(crate) fn set_upstream(&self, upstream: MacAddress, source: MacAddress) {
        self.cached_upstream.store(Some(upstream.into()));
        self.cached_source.store(Some(source.into()));
    }

    /// Set the cached candidates list.
    pub(crate) fn set_candidates(&self, candidates: Vec<MacAddress>) {
        if candidates.is_empty() {
            self.cached_candidates.store(None);
        } else {
            self.cached_candidates.store(Some(Arc::new(candidates)));
        }
    }

    /// Get the configured number of candidates to cache.
    pub(crate) fn candidates_count(&self) -> usize {
        usize::try_from(self.cached_candidates_count)
            .unwrap_or(3)
            .max(1)
    }

    /// Test helper: directly set cached candidates and primary for tests.
    #[cfg(test)]
    pub(crate) fn test_set_cached_candidates(&self, cands: Vec<MacAddress>) {
        if cands.is_empty() {
            self.cached_candidates.store(None);
            self.cached_upstream.store(None);
        } else {
            self.cached_candidates.store(Some(Arc::new(cands.clone())));
            self.cached_upstream.store(Some(cands[0].into()));
        }
    }

    /// Rotate to the next cached candidate (promote the next candidate to primary).
    /// Returns the newly promoted primary if any.
    /// If only one candidate or empty, attempts to rebuild from provided rebuild function.
    pub(crate) fn failover<F>(&self, rebuild_fn: F) -> Option<MacAddress>
    where
        F: FnOnce(MacAddress, usize) -> Vec<MacAddress>,
    {
        let mut cand_opt = self
            .cached_candidates
            .load()
            .as_ref()
            .map(|arcv| (**arcv).clone());

        let mut cands = cand_opt.take().unwrap_or_default();
        let was_rebuilt = cands.len() <= 1;
        
        if was_rebuilt {
            // Try to rebuild using the provided function
            if let Some(src) = self.cached_source.load().as_ref().map(|m| **m) {
                let n_best = self.candidates_count();
                cands = rebuild_fn(src, n_best);
                
                // Store rebuilt candidates and set first as primary
                if !cands.is_empty() {
                    self.cached_candidates.store(Some(Arc::new(cands.clone())));
                    self.cached_upstream.store(Some(cands[0].into()));
                    return Some(cands[0]);
                }
            }
        }

        if cands.len() <= 1 {
            // Nothing to rotate to
            return cands.first().copied();
        }
        
        // Rotate to next (only if we didn't just rebuild)
        let old = cands.remove(0);
        cands.push(old);
        self.cached_candidates.store(Some(Arc::new(cands.clone())));
        self.cached_upstream.store(Some(cands[0].into()));
        Some(cands[0])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_cached_upstream_returns_none_when_empty() {
        let cache = RoutingCache::new(3);
        assert_eq!(cache.get_cached_upstream(), None);
    }

    #[test]
    fn set_and_get_upstream() {
        let cache = RoutingCache::new(3);
        let upstream: MacAddress = [1, 2, 3, 4, 5, 6].into();
        let source: MacAddress = [7, 8, 9, 10, 11, 12].into();
        
        cache.set_upstream(upstream, source);
        
        assert_eq!(cache.get_cached_upstream(), Some(upstream));
        assert_eq!(cache.get_cached_source(), Some(source));
    }

    #[test]
    fn clear_removes_cache() {
        let cache = RoutingCache::new(3);
        let upstream: MacAddress = [1, 2, 3, 4, 5, 6].into();
        let source: MacAddress = [7, 8, 9, 10, 11, 12].into();
        
        cache.set_upstream(upstream, source);
        cache.clear();
        
        assert_eq!(cache.get_cached_upstream(), None);
        assert_eq!(cache.get_cached_candidates(), None);
    }

    #[test]
    fn set_and_get_candidates() {
        let cache = RoutingCache::new(3);
        let cands = vec![
            [1, 2, 3, 4, 5, 6].into(),
            [7, 8, 9, 10, 11, 12].into(),
        ];
        
        cache.set_candidates(cands.clone());
        
        assert_eq!(cache.get_cached_candidates(), Some(cands));
    }

    #[test]
    fn failover_with_multiple_candidates() {
        let cache = RoutingCache::new(3);
        let cands = vec![
            [1, 1, 1, 1, 1, 1].into(),
            [2, 2, 2, 2, 2, 2].into(),
            [3, 3, 3, 3, 3, 3].into(),
        ];
        
        cache.set_candidates(cands.clone());
        cache.set_upstream(cands[0], [99, 99, 99, 99, 99, 99].into());
        
        let promoted = cache.failover(|_, _| vec![]);
        
        assert_eq!(promoted, Some(cands[1]));
        assert_eq!(cache.get_cached_upstream(), Some(cands[1]));
        
        // Candidates should be rotated: [2, 3, 1]
        let new_cands = cache.get_cached_candidates().unwrap();
        assert_eq!(new_cands[0], cands[1]);
        assert_eq!(new_cands[1], cands[2]);
        assert_eq!(new_cands[2], cands[0]);
    }

    #[test]
    fn failover_with_one_candidate_attempts_rebuild() {
        let cache = RoutingCache::new(3);
        let src: MacAddress = [99, 99, 99, 99, 99, 99].into();
        cache.set_upstream([1, 1, 1, 1, 1, 1].into(), src);
        cache.set_candidates(vec![[1, 1, 1, 1, 1, 1].into()]);
        
        let rebuilt_cands = vec![
            [2, 2, 2, 2, 2, 2].into(),
            [3, 3, 3, 3, 3, 3].into(),
        ];
        
        let promoted = cache.failover(|s, n| {
            assert_eq!(s, src);
            assert_eq!(n, 3);
            rebuilt_cands.clone()
        });
        
        assert_eq!(promoted, Some(rebuilt_cands[0]));
        assert_eq!(cache.get_cached_candidates(), Some(rebuilt_cands));
    }
}
