//! Simulation metrics and observability.
//!
//! This module provides real-time metrics collection for the simulator,
//! including packet statistics, channel performance, and simulation health.
//!
//! # Example
//!
//! ```rust,no_run
//! use simulator::metrics::SimulatorMetrics;
//!
//! let metrics = SimulatorMetrics::new();
//! metrics.record_packet_sent();
//! metrics.record_packet_dropped();
//! metrics.record_latency(15); // 15ms
//!
//! let summary = metrics.summary();
//! println!("Packets sent: {}", summary.packets_sent);
//! println!("Average latency: {:.2}ms", summary.avg_latency_ms);
//! ```

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Per-channel packet statistics (from source -> destination)
#[derive(Debug, Clone, Default)]
pub struct ChannelStats {
    /// Number of packets successfully delivered through this channel
    pub packets_sent: u64,
    /// Number of packets dropped on this channel (due to simulated loss)
    pub packets_dropped: u64,
    /// Total bytes sent through this channel
    pub bytes_sent: u64,
    /// Cumulative latency in microseconds for this channel
    pub total_latency_us: u64,
    /// Number of packets delayed on this channel
    pub packets_delayed: u64,
    /// Recent latency samples in microseconds (for percentile calculations)
    pub latency_samples: VecDeque<u64>,
    /// Rolling window of (timestamp, bytes) for throughput calculation (last 10 seconds)
    pub throughput_window: VecDeque<(Instant, u64)>,
}

impl ChannelStats {
    /// Calculate throughput over the last `n` seconds in bytes per second
    #[allow(dead_code)]
    pub fn throughput_last(&self, n: u32) -> f64 {
        let now = Instant::now();
        let cutoff = now - Duration::from_secs(n.into());

        // Sum bytes from entries within the last `n` seconds
        let total_bytes: u64 = self
            .throughput_window
            .iter()
            .filter(|(timestamp, _)| *timestamp >= cutoff)
            .map(|(_, bytes)| bytes)
            .sum();

        // Calculate actual time span of data
        let oldest_timestamp = self
            .throughput_window
            .iter()
            .filter(|(timestamp, _)| *timestamp >= cutoff)
            .map(|(timestamp, _)| *timestamp)
            .min();

        if let Some(oldest) = oldest_timestamp {
            let time_span = now.duration_since(oldest).as_secs_f64();
            if time_span > 0.0 {
                return total_bytes as f64 / time_span;
            }
        }

        0.0
    }
}

/// Real-time metrics for simulation observability.
///
/// All operations are thread-safe using atomic operations, allowing
/// metrics to be collected from multiple async tasks simultaneously.
#[derive(Debug)]
pub struct SimulatorMetrics {
    /// Timestamp when metrics collection started
    start_time: Instant,
    /// Total packets successfully sent through channels
    packets_sent: AtomicU64,
    /// Total packets dropped due to packet loss simulation
    packets_dropped: AtomicU64,
    /// Total packets delayed due to latency simulation
    packets_delayed: AtomicU64,
    /// Cumulative latency in microseconds
    total_latency_us: AtomicU64,
    /// Number of active channels
    active_channels: AtomicU64,
    /// Number of active nodes
    active_nodes: AtomicU64,
    /// Per-channel statistics (key: "source->destination")
    channel_stats: Mutex<HashMap<String, ChannelStats>>,
}

impl SimulatorMetrics {
    /// Create a new metrics collector.
    pub fn new() -> Self {
        Self {
            start_time: Instant::now(),
            packets_sent: AtomicU64::new(0),
            packets_dropped: AtomicU64::new(0),
            packets_delayed: AtomicU64::new(0),
            total_latency_us: AtomicU64::new(0),
            active_channels: AtomicU64::new(0),
            active_nodes: AtomicU64::new(0),
            channel_stats: Mutex::new(HashMap::new()),
        }
    }

    /// Record a successfully sent packet for a specific channel.
    pub fn record_packet_sent_for_channel(&self, from: &str, to: &str, bytes: usize) {
        self.packets_sent.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut stats) = self.channel_stats.lock() {
            let key = format!("{}->{}", from, to);
            let entry = stats.entry(key).or_default();

            let now = Instant::now();

            // Add new data point to throughput window
            entry.throughput_window.push_back((now, bytes as u64));

            // Remove entries older than 10 seconds
            let cutoff = now - Duration::from_secs(10);
            while let Some((timestamp, _)) = entry.throughput_window.front() {
                if *timestamp < cutoff {
                    entry.throughput_window.pop_front();
                } else {
                    break;
                }
            }

            entry.packets_sent += 1;
            entry.bytes_sent += bytes as u64;
        }
    }

    /// Record a successfully sent packet (global counter).
    #[allow(dead_code)]
    pub fn record_packet_sent(&self) {
        self.packets_sent.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a dropped packet for a specific channel.
    pub fn record_packet_dropped_for_channel(&self, from: &str, to: &str) {
        self.packets_dropped.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut stats) = self.channel_stats.lock() {
            let key = format!("{}->{}", from, to);
            stats.entry(key).or_default().packets_dropped += 1;
        }
    }

    /// Record a dropped packet (global counter).
    #[allow(dead_code)]
    pub fn record_packet_dropped(&self) {
        self.packets_dropped.fetch_add(1, Ordering::Relaxed);
    }

    /// Record latency for a specific channel.
    pub fn record_latency_for_channel(&self, from: &str, to: &str, latency: Duration) {
        if let Ok(mut stats) = self.channel_stats.lock() {
            let key = format!("{}->{}", from, to);
            let entry = stats.entry(key).or_default();
            entry.total_latency_us += latency.as_micros() as u64;
            entry.packets_delayed += 1;
            // Record sample for percentile estimates; keep last 1000 samples
            entry.latency_samples.push_back(latency.as_micros() as u64);
            if entry.latency_samples.len() > 1000 {
                entry.latency_samples.pop_front();
            }
        }
    }

    /// Record a delayed packet with its latency.
    ///
    /// # Arguments
    /// * `latency` - The delay duration
    #[allow(dead_code)]
    pub fn record_packet_delayed(&self, latency: Duration) {
        self.packets_delayed.fetch_add(1, Ordering::Relaxed);
        self.total_latency_us
            .fetch_add(latency.as_micros() as u64, Ordering::Relaxed);
    }

    /// Set the number of active channels.
    pub fn set_active_channels(&self, count: u64) {
        self.active_channels.store(count, Ordering::Relaxed);
    }

    /// Set the number of active nodes.
    pub fn set_active_nodes(&self, count: u64) {
        self.active_nodes.store(count, Ordering::Relaxed);
    }

    /// Get the elapsed time since metrics collection started.
    #[allow(dead_code)]
    pub fn elapsed_time(&self) -> Duration {
        self.start_time.elapsed()
    }

    /// Get per-channel statistics snapshot.
    #[allow(dead_code)]
    pub fn channel_stats(&self) -> HashMap<String, ChannelStats> {
        self.channel_stats.lock().unwrap().clone()
    }

    /// Get current metrics summary.
    pub fn summary(&self) -> MetricsSummary {
        let packets_sent = self.packets_sent.load(Ordering::Relaxed);
        let packets_dropped = self.packets_dropped.load(Ordering::Relaxed);
        let packets_delayed = self.packets_delayed.load(Ordering::Relaxed);
        let total_latency_us = self.total_latency_us.load(Ordering::Relaxed);
        let active_channels = self.active_channels.load(Ordering::Relaxed);
        let active_nodes = self.active_nodes.load(Ordering::Relaxed);

        let total_packets = packets_sent + packets_dropped;
        let drop_rate = if total_packets > 0 {
            (packets_dropped as f64 / total_packets as f64) * 100.0
        } else {
            0.0
        };

        let avg_latency_us = if packets_delayed > 0 {
            total_latency_us as f64 / packets_delayed as f64
        } else {
            0.0
        };

        let uptime = self.start_time.elapsed();

        MetricsSummary {
            packets_sent,
            packets_dropped,
            packets_delayed,
            total_packets,
            drop_rate,
            avg_latency_us,
            active_channels,
            active_nodes,
            uptime,
        }
    }

    /// Reset all metrics.
    #[allow(dead_code)]
    pub fn reset(&self) {
        self.packets_sent.store(0, Ordering::Relaxed);
        self.packets_dropped.store(0, Ordering::Relaxed);
        self.packets_delayed.store(0, Ordering::Relaxed);
        self.total_latency_us.store(0, Ordering::Relaxed);
    }
}

impl Default for SimulatorMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Snapshot of simulation metrics at a point in time.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Fields are used via serde_json macro in webview
pub struct MetricsSummary {
    /// Total packets successfully sent
    pub packets_sent: u64,
    /// Total packets dropped
    pub packets_dropped: u64,
    /// Total packets delayed
    pub packets_delayed: u64,
    /// Total packets processed (sent + dropped)
    pub total_packets: u64,
    /// Packet drop rate as percentage (0-100)
    pub drop_rate: f64,
    /// Average packet latency in microseconds
    pub avg_latency_us: f64,
    /// Number of active channels
    pub active_channels: u64,
    /// Number of active nodes
    pub active_nodes: u64,
    /// Simulation uptime
    pub uptime: Duration,
}

impl MetricsSummary {
    /// Get average latency in milliseconds.
    pub fn avg_latency_ms(&self) -> f64 {
        self.avg_latency_us / 1000.0
    }

    /// Get throughput in packets per second.
    pub fn packets_per_second(&self) -> f64 {
        let uptime_secs = self.uptime.as_secs_f64();
        if uptime_secs > 0.0 {
            self.packets_sent as f64 / uptime_secs
        } else {
            0.0
        }
    }

    /// Format metrics as a human-readable string.
    #[allow(dead_code)]
    pub fn to_string_formatted(&self) -> String {
        format!(
            "Simulation Metrics\n\
             ==================\n\
             Uptime: {:.2}s\n\
             Nodes: {}\n\
             Channels: {}\n\
             \n\
             Packets:\n\
             - Sent: {}\n\
             - Dropped: {} ({:.2}%)\n\
             - Delayed: {}\n\
             - Total: {}\n\
             \n\
             Performance:\n\
             - Throughput: {:.2} pkt/s\n\
             - Avg Latency: {:.3}ms\n",
            self.uptime.as_secs_f64(),
            self.active_nodes,
            self.active_channels,
            self.packets_sent,
            self.packets_dropped,
            self.drop_rate,
            self.packets_delayed,
            self.total_packets,
            self.packets_per_second(),
            self.avg_latency_ms(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn metrics_starts_empty() {
        let metrics = SimulatorMetrics::new();
        let summary = metrics.summary();

        assert_eq!(summary.packets_sent, 0);
        assert_eq!(summary.packets_dropped, 0);
        assert_eq!(summary.total_packets, 0);
        assert_eq!(summary.drop_rate, 0.0);
    }

    #[test]
    fn records_packet_sent() {
        let metrics = SimulatorMetrics::new();
        metrics.record_packet_sent();
        metrics.record_packet_sent();

        let summary = metrics.summary();
        assert_eq!(summary.packets_sent, 2);
        assert_eq!(summary.total_packets, 2);
    }

    #[test]
    fn records_packet_dropped() {
        let metrics = SimulatorMetrics::new();
        metrics.record_packet_sent();
        metrics.record_packet_dropped();

        let summary = metrics.summary();
        assert_eq!(summary.packets_sent, 1);
        assert_eq!(summary.packets_dropped, 1);
        assert_eq!(summary.total_packets, 2);
        assert_eq!(summary.drop_rate, 50.0);
    }

    #[test]
    fn calculates_drop_rate() {
        let metrics = SimulatorMetrics::new();
        for _ in 0..7 {
            metrics.record_packet_sent();
        }
        for _ in 0..3 {
            metrics.record_packet_dropped();
        }

        let summary = metrics.summary();
        assert_eq!(summary.drop_rate, 30.0); // 3/10 = 30%
    }

    #[test]
    fn records_latency() {
        let metrics = SimulatorMetrics::new();
        metrics.record_packet_delayed(Duration::from_millis(10));
        metrics.record_packet_delayed(Duration::from_millis(20));

        let summary = metrics.summary();
        assert_eq!(summary.packets_delayed, 2);
        assert_eq!(summary.avg_latency_ms(), 15.0); // (10+20)/2 = 15
    }

    #[test]
    fn tracks_active_resources() {
        let metrics = SimulatorMetrics::new();
        metrics.set_active_nodes(5);
        metrics.set_active_channels(10);

        let summary = metrics.summary();
        assert_eq!(summary.active_nodes, 5);
        assert_eq!(summary.active_channels, 10);
    }

    #[test]
    fn calculates_throughput() {
        let metrics = SimulatorMetrics::new();

        // Wait a bit to have measurable uptime
        thread::sleep(Duration::from_millis(100));

        for _ in 0..10 {
            metrics.record_packet_sent();
        }

        let summary = metrics.summary();
        assert!(summary.packets_per_second() > 0.0);
        assert!(summary.packets_per_second() < 1000.0); // Reasonable bound
    }

    #[test]
    fn resets_metrics() {
        let metrics = SimulatorMetrics::new();
        metrics.record_packet_sent();
        metrics.record_packet_dropped();
        metrics.record_packet_delayed(Duration::from_millis(10));

        metrics.reset();

        let summary = metrics.summary();
        assert_eq!(summary.packets_sent, 0);
        assert_eq!(summary.packets_dropped, 0);
        assert_eq!(summary.packets_delayed, 0);
    }

    #[test]
    fn formats_summary() {
        let metrics = SimulatorMetrics::new();
        metrics.set_active_nodes(3);
        metrics.set_active_channels(6);
        metrics.record_packet_sent();

        let summary = metrics.summary();
        let formatted = summary.to_string_formatted();

        assert!(formatted.contains("Nodes: 3"));
        assert!(formatted.contains("Channels: 6"));
        assert!(formatted.contains("Sent: 1"));
    }

    #[test]
    fn default_creates_new() {
        let metrics = SimulatorMetrics::default();
        let summary = metrics.summary();
        assert_eq!(summary.packets_sent, 0);
    }
}
