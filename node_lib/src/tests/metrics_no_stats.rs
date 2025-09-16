#[cfg(test)]
mod metrics_no_stats {
    use super::super::metrics;

    #[test]
    fn metrics_no_stats_default_returns_zero_and_noops() {
        // When compiled without the `stats` feature the metric helpers are no-ops
        // and should return zero for counts.
        let before_loop = metrics::loop_detected_count();
        metrics::inc_loop_detected();
        let after_loop = metrics::loop_detected_count();
        assert_eq!(before_loop, 0);
        assert_eq!(after_loop, 0);

        let before_cache = metrics::cache_select_count();
        metrics::inc_cache_select();
        let after_cache = metrics::cache_select_count();
        assert_eq!(before_cache, 0);
        assert_eq!(after_cache, 0);

        let before_clear = metrics::cache_clear_count();
        metrics::inc_cache_clear();
        let after_clear = metrics::cache_clear_count();
        assert_eq!(before_clear, 0);
        assert_eq!(after_clear, 0);
    }
}
#[cfg(test)]
mod metrics_no_stats {
    // These tests run without the `stats` feature and exercise the no-op paths
    use crate::metrics::{
        cache_clear_count, cache_select_count, inc_cache_clear, inc_cache_select, inc_loop_detected,
        loop_detected_count,
    };

    #[test]
    fn metrics_no_stats_return_zero_and_noop() {
        // Values start at 0 and increments are no-ops
        assert_eq!(loop_detected_count(), 0);
        assert_eq!(cache_select_count(), 0);
        assert_eq!(cache_clear_count(), 0);

        inc_loop_detected();
        inc_cache_select();
        inc_cache_clear();

        assert_eq!(loop_detected_count(), 0);
        assert_eq!(cache_select_count(), 0);
        assert_eq!(cache_clear_count(), 0);
    }
}
