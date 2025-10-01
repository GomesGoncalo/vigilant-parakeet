use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

/// Snapshot of statistics at a point in time.
/// This is the serializable representation returned by stats() calls.
#[derive(Serialize, Default, Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub struct Stats {
    pub received_packets: u128,
    pub received_bytes: u128,
    pub transmitted_packets: u128,
    pub transmitted_bytes: u128,
}

/// Lock-free atomic statistics tracker.
/// Uses AtomicU64 internally for high-performance concurrent updates.
/// Note: Uses u64 instead of u128 because AtomicU128 is not stable/portable.
#[derive(Debug)]
pub struct AtomicStats {
    received_packets: AtomicU64,
    received_bytes: AtomicU64,
    transmitted_packets: AtomicU64,
    transmitted_bytes: AtomicU64,
}

impl Default for AtomicStats {
    fn default() -> Self {
        Self::new()
    }
}

impl AtomicStats {
    /// Create a new AtomicStats with all counters at zero.
    pub const fn new() -> Self {
        Self {
            received_packets: AtomicU64::new(0),
            received_bytes: AtomicU64::new(0),
            transmitted_packets: AtomicU64::new(0),
            transmitted_bytes: AtomicU64::new(0),
        }
    }

    /// Increment received packet count and bytes atomically.
    #[inline]
    pub fn record_receive(&self, bytes: usize) {
        self.received_packets.fetch_add(1, Ordering::Relaxed);
        self.received_bytes
            .fetch_add(bytes as u64, Ordering::Relaxed);
    }

    /// Increment transmitted packet count and bytes atomically.
    #[inline]
    pub fn record_transmit(&self, bytes: usize) {
        self.transmitted_packets.fetch_add(1, Ordering::Relaxed);
        self.transmitted_bytes
            .fetch_add(bytes as u64, Ordering::Relaxed);
    }

    /// Get a snapshot of current statistics.
    /// This is lock-free but may see slightly inconsistent values
    /// if updates happen concurrently (accepted trade-off for performance).
    pub fn snapshot(&self) -> Stats {
        Stats {
            received_packets: self.received_packets.load(Ordering::Relaxed) as u128,
            received_bytes: self.received_bytes.load(Ordering::Relaxed) as u128,
            transmitted_packets: self.transmitted_packets.load(Ordering::Relaxed) as u128,
            transmitted_bytes: self.transmitted_bytes.load(Ordering::Relaxed) as u128,
        }
    }

    /// Reset all counters to zero.
    #[cfg(test)]
    pub fn reset(&self) {
        self.received_packets.store(0, Ordering::Relaxed);
        self.received_bytes.store(0, Ordering::Relaxed);
        self.transmitted_packets.store(0, Ordering::Relaxed);
        self.transmitted_bytes.store(0, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_default_is_zeroed() {
        let s = Stats::default();
        assert_eq!(s.received_packets, 0);
        assert_eq!(s.transmitted_bytes, 0);
    }

    #[test]
    fn atomic_stats_new_is_zeroed() {
        let stats = AtomicStats::new();
        let snapshot = stats.snapshot();
        assert_eq!(snapshot.received_packets, 0);
        assert_eq!(snapshot.received_bytes, 0);
        assert_eq!(snapshot.transmitted_packets, 0);
        assert_eq!(snapshot.transmitted_bytes, 0);
    }

    #[test]
    fn atomic_stats_record_receive() {
        let stats = AtomicStats::new();
        stats.record_receive(100);
        stats.record_receive(200);

        let snapshot = stats.snapshot();
        assert_eq!(snapshot.received_packets, 2);
        assert_eq!(snapshot.received_bytes, 300);
        assert_eq!(snapshot.transmitted_packets, 0);
        assert_eq!(snapshot.transmitted_bytes, 0);
    }

    #[test]
    fn atomic_stats_record_transmit() {
        let stats = AtomicStats::new();
        stats.record_transmit(50);
        stats.record_transmit(75);

        let snapshot = stats.snapshot();
        assert_eq!(snapshot.received_packets, 0);
        assert_eq!(snapshot.received_bytes, 0);
        assert_eq!(snapshot.transmitted_packets, 2);
        assert_eq!(snapshot.transmitted_bytes, 125);
    }

    #[test]
    fn atomic_stats_mixed_operations() {
        let stats = AtomicStats::new();
        stats.record_receive(100);
        stats.record_transmit(200);
        stats.record_receive(150);
        stats.record_transmit(250);

        let snapshot = stats.snapshot();
        assert_eq!(snapshot.received_packets, 2);
        assert_eq!(snapshot.received_bytes, 250);
        assert_eq!(snapshot.transmitted_packets, 2);
        assert_eq!(snapshot.transmitted_bytes, 450);
    }

    #[test]
    fn atomic_stats_reset() {
        let stats = AtomicStats::new();
        stats.record_receive(100);
        stats.record_transmit(200);

        let before = stats.snapshot();
        assert_eq!(before.received_packets, 1);
        assert_eq!(before.transmitted_packets, 1);

        stats.reset();

        let after = stats.snapshot();
        assert_eq!(after.received_packets, 0);
        assert_eq!(after.received_bytes, 0);
        assert_eq!(after.transmitted_packets, 0);
        assert_eq!(after.transmitted_bytes, 0);
    }

    #[test]
    fn atomic_stats_concurrent_updates() {
        use std::sync::Arc;
        use std::thread;

        let stats = Arc::new(AtomicStats::new());
        let mut handles = vec![];

        // Spawn multiple threads updating concurrently
        for _ in 0..4 {
            let stats_clone = Arc::clone(&stats);
            let handle = thread::spawn(move || {
                for _ in 0..100 {
                    stats_clone.record_receive(10);
                    stats_clone.record_transmit(20);
                }
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.join().unwrap();
        }

        let snapshot = stats.snapshot();
        // 4 threads * 100 iterations = 400 operations each
        assert_eq!(snapshot.received_packets, 400);
        assert_eq!(snapshot.received_bytes, 4000);
        assert_eq!(snapshot.transmitted_packets, 400);
        assert_eq!(snapshot.transmitted_bytes, 8000);
    }

    #[test]
    fn stats_struct_is_copy() {
        let s1 = Stats::default();
        let s2 = s1; // Should copy, not move
        assert_eq!(s1, s2);
    }
}
