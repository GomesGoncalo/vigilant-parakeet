//! TUI state management and data structures

use crate::metrics::{MetricsSummary, SimulatorMetrics};
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
    time::Instant,
};

use super::{logging::LogFilter, tabs::Tab};

/// TUI update frequency in Hz
pub(crate) const TUI_UPDATES_PER_SECOND: usize = 4;

/// Target history duration in seconds
const HISTORY_DURATION_SECONDS: usize = 30;

/// Maximum number of history points to keep for sparkline/chart series
/// Calculated to hold approximately HISTORY_DURATION_SECONDS of data at TUI_UPDATES_PER_SECOND
pub const MAX_HISTORY: usize = TUI_UPDATES_PER_SECOND * HISTORY_DURATION_SECONDS;

/// Channel sorting mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelSortMode {
    Loss,       // Sort by loss percentage (default)
    Throughput, // Sort by throughput
    Latency,    // Sort by latency
    Name,       // Sort alphabetically by name
}

impl ChannelSortMode {
    pub fn next(&self) -> Self {
        match self {
            Self::Loss => Self::Throughput,
            Self::Throughput => Self::Latency,
            Self::Latency => Self::Name,
            Self::Name => Self::Loss,
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Loss => "Loss %",
            Self::Throughput => "Throughput",
            Self::Latency => "Latency",
            Self::Name => "Name",
        }
    }
}

/// Sort direction for columns
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    Asc,
    Desc,
}

impl SortDirection {
    pub fn toggle(&self) -> Self {
        match self {
            SortDirection::Asc => SortDirection::Desc,
            SortDirection::Desc => SortDirection::Asc,
        }
    }

    pub fn arrow(&self) -> &'static str {
        match self {
            SortDirection::Asc => "▲",
            SortDirection::Desc => "▼",
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            SortDirection::Asc => "Asc",
            SortDirection::Desc => "Desc",
        }
    }
}

/// Concrete channel stats computed at pause time for stable display (no Instants)
#[derive(Debug, Clone)]
pub struct DisplayChannelStats {
    pub packets_sent: u64,
    pub packets_dropped: u64,
    pub throughput_bps: f64,
    pub avg_latency_ms: f64,
}

/// Type alias for paused upstream snapshot entries:
/// (obu_name, obu_mac, upstream_display, upstream_mac, hops, next_hop_mac)
pub type UpstreamSnapshotEntry = (String, String, String, String, String, String);

/// TUI state maintaining historical data for graphs
pub struct TuiState {
    pub metrics: Arc<SimulatorMetrics>,
    // Map of nodes: name -> (device mac string, node_type_string, SimNode)
    pub nodes: HashMap<String, (String, String, crate::simulator::SimNode)>,
    // Last time nodes were refreshed
    pub last_nodes_refresh: Instant,
    pub start_time: Instant,

    // Historical data for graphs
    pub packets_sent_history: Vec<(f64, f64)>,
    pub loss_percentage_history: Vec<(f64, f64)>,
    pub throughput_history: Vec<(f64, f64)>,
    // Throughput history in bits-per-second
    pub throughput_bps_history: Vec<(f64, f64)>,
    pub latency_history: Vec<(f64, f64)>,
    // Percentile history for tail latency
    pub p95_history: Vec<(f64, f64)>,
    pub p99_history: Vec<(f64, f64)>,

    // Previous values for calculating deltas
    pub prev_packets_sent: u64,
    pub prev_packets_dropped: u64,
    pub prev_timestamp: f64,

    // UI state
    pub active_tab: Tab,
    // Selected index in the topology view (flattened list order)
    pub selected_topology_index: usize,
    // Number of items in the last rendered topology (for navigation bounds)
    pub topology_item_count: usize,
    pub log_buffer: Arc<Mutex<VecDeque<String>>>,
    pub log_scroll: usize,
    pub log_horizontal_scroll: usize,
    pub log_wrap: bool,
    pub log_auto_scroll: bool,
    pub channel_sort_mode: ChannelSortMode,
    pub channel_sort_direction: SortDirection,
    // Snapshots captured when paused
    pub paused_summary: Option<MetricsSummary>,
    // Concrete display snapshot for channels (precomputed throughput/latency at pause time)
    pub paused_channel_display: Option<HashMap<String, DisplayChannelStats>>,
    // Snapshot of upstream entries when paused
    pub paused_upstreams: Option<Vec<UpstreamSnapshotEntry>>,
    pub log_filter: LogFilter,
    pub log_input_mode: bool,
    pub log_input_buffer: String,
    pub paused: bool,
}

impl TuiState {
    pub fn new(metrics: Arc<SimulatorMetrics>, log_buffer: Arc<Mutex<VecDeque<String>>>) -> Self {
        Self {
            metrics,
            nodes: HashMap::new(),
            last_nodes_refresh: Instant::now(),
            start_time: Instant::now(),
            packets_sent_history: Vec::new(),
            loss_percentage_history: Vec::new(),
            throughput_history: Vec::new(),
            throughput_bps_history: Vec::new(),
            latency_history: Vec::new(),
            p95_history: Vec::new(),
            p99_history: Vec::new(),
            prev_packets_sent: 0,
            prev_packets_dropped: 0,
            prev_timestamp: 0.0,
            active_tab: Tab::Metrics,
            selected_topology_index: 0,
            topology_item_count: 0,
            log_buffer,
            log_scroll: 0,
            log_horizontal_scroll: 0,
            log_wrap: false,
            log_auto_scroll: true,
            channel_sort_mode: ChannelSortMode::Loss,
            channel_sort_direction: SortDirection::Desc,
            paused_summary: None,
            paused_channel_display: None,
            paused_upstreams: None,
            log_filter: LogFilter::All,
            log_input_mode: false,
            log_input_buffer: String::new(),
            paused: false,
        }
    }

    /// Refresh nodes map from simulator reference. This clones the simulator nodes
    /// snapshot and stores a compact representation for the UI.
    pub fn refresh_nodes(&mut self, simulator: &crate::simulator::Simulator) {
        use common::network_interface::NetworkInterface;

        // Use get_nodes_with_interfaces() which is available without the `webview` feature.
        // We only need the device MAC and the SimNode for the UI snapshot.
        let sim_nodes = simulator.get_nodes_with_interfaces();
        let map = sim_nodes
            .into_iter()
            .map(|(name, (device, _interfaces, node))| {
                let node_type = match &node {
                    crate::simulator::SimNode::Obu(_) => "Obu".to_string(),
                    crate::simulator::SimNode::Rsu(_) => "Rsu".to_string(),
                    crate::simulator::SimNode::Server(_) => "Server".to_string(),
                };
                (name, (format!("{}", device.mac_address()), node_type, node))
            })
            .collect();
        self.nodes = map;
        self.last_nodes_refresh = Instant::now();
    }

    /// Update historical data with current metrics
    pub fn update(&mut self) {
        let summary = self.metrics.summary();
        let elapsed = self.start_time.elapsed().as_secs_f64();

        // Calculate deltas for rate-based metrics
        let packets_sent_delta = summary.packets_sent.saturating_sub(self.prev_packets_sent);
        let time_delta = elapsed - self.prev_timestamp;

        let current_throughput = if time_delta > 0.0 {
            packets_sent_delta as f64 / time_delta
        } else {
            0.0
        };

        // Add new data points
        self.packets_sent_history
            .push((elapsed, summary.packets_sent as f64));
        self.loss_percentage_history
            .push((elapsed, summary.drop_rate * 100.0)); // Convert to percentage
        self.throughput_history.push((elapsed, current_throughput));
        self.latency_history
            .push((elapsed, summary.avg_latency_us / 1000.0)); // Convert to ms

        // Compute p95/p99 based on merged channel latency samples in metrics
        let channel_map = self.metrics.channel_stats();
        let mut all_samples: Vec<u64> = Vec::new();
        let mut total_bytes_last10: u64 = 0;
        for (_k, stats) in channel_map.iter() {
            for &s in stats.latency_samples.iter() {
                all_samples.push(s);
            }
            // Sum bytes from per-channel throughput windows (last ~10s)
            total_bytes_last10 = total_bytes_last10
                .saturating_add(stats.throughput_window.iter().map(|(_, b)| *b).sum::<u64>());
        }
        all_samples.sort_unstable();
        let p95_val = if !all_samples.is_empty() {
            let idx = ((all_samples.len() as f64) * 0.95).ceil() as usize - 1;
            (all_samples[idx] as f64) / 1000.0
        } else {
            0.0
        };
        let p99_val = if !all_samples.is_empty() {
            let idx = ((all_samples.len() as f64) * 0.99).ceil() as usize - 1;
            (all_samples[idx] as f64) / 1000.0
        } else {
            0.0
        };

        self.p95_history.push((elapsed, p95_val));
        self.p99_history.push((elapsed, p99_val));

        // Record throughput in bits/sec (approximate based on last-10s window)
        let throughput_bps = if total_bytes_last10 > 0 {
            (total_bytes_last10 as f64 / 10.0) * 8.0
        } else {
            0.0
        };
        self.throughput_bps_history.push((elapsed, throughput_bps));

        // Keep only recent history
        if self.packets_sent_history.len() > MAX_HISTORY {
            self.packets_sent_history.remove(0);
        }
        if self.loss_percentage_history.len() > MAX_HISTORY {
            self.loss_percentage_history.remove(0);
        }
        if self.throughput_history.len() > MAX_HISTORY {
            self.throughput_history.remove(0);
        }
        if self.throughput_bps_history.len() > MAX_HISTORY {
            self.throughput_bps_history.remove(0);
        }
        if self.latency_history.len() > MAX_HISTORY {
            self.latency_history.remove(0);
        }
        if self.p95_history.len() > MAX_HISTORY {
            self.p95_history.remove(0);
        }
        if self.p99_history.len() > MAX_HISTORY {
            self.p99_history.remove(0);
        }

        // Update previous values
        self.prev_packets_sent = summary.packets_sent;
        self.prev_packets_dropped = summary.packets_dropped;
        self.prev_timestamp = elapsed;

        // Auto-scroll logs to bottom if enabled
        if self.log_auto_scroll {
            let log_count = self.log_buffer.lock().unwrap().len();
            self.log_scroll = log_count.saturating_sub(1);
        }
    }
}
