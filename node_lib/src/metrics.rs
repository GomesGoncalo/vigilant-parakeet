// Feature-gated metrics helpers for node_lib.
// When the "stats" feature is enabled this exposes lightweight atomics.

#[cfg(feature = "stats")]
mod with_stats {
    use std::sync::atomic::{AtomicU64, Ordering};

    static LOOP_DETECTED_COUNT: AtomicU64 = AtomicU64::new(0);
    static CACHE_SELECT_COUNT: AtomicU64 = AtomicU64::new(0);
    static CACHE_CLEAR_COUNT: AtomicU64 = AtomicU64::new(0);

    pub fn inc_loop_detected() {
        LOOP_DETECTED_COUNT.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_cache_select() {
        CACHE_SELECT_COUNT.fetch_add(1, Ordering::Relaxed);
    }

    pub fn loop_detected_count() -> u64 {
        LOOP_DETECTED_COUNT.load(Ordering::Relaxed)
    }

    pub fn cache_select_count() -> u64 {
        CACHE_SELECT_COUNT.load(Ordering::Relaxed)
    }

    pub fn inc_cache_clear() {
        CACHE_CLEAR_COUNT.fetch_add(1, Ordering::Relaxed);
    }

    pub fn cache_clear_count() -> u64 {
        CACHE_CLEAR_COUNT.load(Ordering::Relaxed)
    }
}

#[cfg(not(feature = "stats"))]
mod without_stats {
    pub fn inc_loop_detected() {}
    pub fn loop_detected_count() -> u64 {
        0
    }
    pub fn inc_cache_select() {}
    pub fn cache_select_count() -> u64 {
        0
    }
    pub fn inc_cache_clear() {}
    pub fn cache_clear_count() -> u64 {
        0
    }
}

#[cfg(feature = "stats")]
pub use with_stats::*;

#[cfg(not(feature = "stats"))]
pub use without_stats::*;

#[cfg(test)]
#[cfg(feature = "stats")]
mod tests {
    use super::*;

    #[test]
    fn cache_select_increments_counter() {
        // Read current value, increment, and ensure the counter increases by at least 1.
        let before = cache_select_count();
        inc_cache_select();
        let after = cache_select_count();
        assert!(
            after >= before + 1,
            "counter did not increase: before={} after={}",
            before,
            after
        );
    }

    #[test]
    fn cache_clear_increments_counter() {
        let before = cache_clear_count();
        inc_cache_clear();
        let after = cache_clear_count();
        assert!(
            after >= before + 1,
            "cache clear counter did not increase: before={} after={}",
            before,
            after
        );
    }
}
