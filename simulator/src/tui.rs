//! Terminal User Interface for real-time simulation monitoring
//!
//! Provides an interactive dashboard displaying:
//! - Packet statistics (sent, dropped, delayed)
//! - Performance metrics (drop rate, latency, throughput)
//! - Resource information (active nodes, channels)
//! - Live sparkline graphs showing trends over time
//! - Captured logs in a separate tab

use crate::metrics::{ChannelStats, MetricsSummary, SimulatorMetrics};
use anyhow::Result;
use common::network_interface::NetworkInterface;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use human_format::Formatter;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    symbols,
    text::{Line, Span},
    widgets::{
        Axis, Block, Borders, Chart, Dataset, GraphType, List, ListItem, ListState, Paragraph, Row,
        Tabs,
    },
    Frame, Terminal,
};
use std::{
    collections::{HashMap, VecDeque},
    io,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::time::interval;
use tracing_subscriber::Layer;

/// Maximum number of data points to keep for sparkline graphs
const MAX_HISTORY: usize = 60;

/// Maximum number of log lines to keep
const MAX_LOGS: usize = 1000;

/// Active tab in the TUI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Metrics,
    Channels,
    Upstreams,
    Logs,
}

/// Channel sorting mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChannelSortMode {
    Loss,       // Sort by loss percentage (default)
    Throughput, // Sort by throughput
    Latency,    // Sort by latency
    Name,       // Sort alphabetically by name
}

/// Sort direction for columns
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortDirection {
    Asc,
    Desc,
}

impl SortDirection {
    fn toggle(&self) -> Self {
        match self {
            SortDirection::Asc => SortDirection::Desc,
            SortDirection::Desc => SortDirection::Asc,
        }
    }

    fn arrow(&self) -> &'static str {
        match self {
            SortDirection::Asc => "▲",
            SortDirection::Desc => "▼",
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            SortDirection::Asc => "Asc",
            SortDirection::Desc => "Desc",
        }
    }
}

/// Log filter mode
#[derive(Debug, Clone, PartialEq, Eq)]
enum LogFilter {
    All,            // Show all logs
    Simulator,      // Show only simulator logs (simulator::, common::)
    Nodes,          // Show only node logs (node_lib::, server_lib::, obu_lib::, rsu_lib::)
    Custom(String), // Show logs containing custom text (e.g., node name)
}

impl ChannelSortMode {
    fn next(&self) -> Self {
        match self {
            Self::Loss => Self::Throughput,
            Self::Throughput => Self::Latency,
            Self::Latency => Self::Name,
            Self::Name => Self::Loss,
        }
    }

    fn as_str(&self) -> &str {
        match self {
            Self::Loss => "Loss %",
            Self::Throughput => "Throughput",
            Self::Latency => "Latency",
            Self::Name => "Name",
        }
    }
}

impl LogFilter {
    fn next(&self) -> Self {
        match self {
            Self::All => Self::Simulator,
            Self::Simulator => Self::Nodes,
            Self::Nodes => Self::All,
            Self::Custom(_) => Self::All, // Cycling resets custom filter
        }
    }

    fn as_str(&self) -> String {
        match self {
            Self::All => "All".to_string(),
            Self::Simulator => "Simulator".to_string(),
            Self::Nodes => "Nodes".to_string(),
            Self::Custom(text) => format!("'{}'", text),
        }
    }

    fn matches(&self, target: &str, full_line: &str) -> bool {
        match self {
            Self::All => true,
            Self::Simulator => target.starts_with("simulator") || target.starts_with("common"),
            Self::Nodes => {
                target.starts_with("node_lib")
                    || target.starts_with("server_lib")
                    || target.starts_with("obu_lib")
                    || target.starts_with("rsu_lib")
            }
            Self::Custom(text) => {
                // Search in full log line for custom text (case-insensitive)
                full_line.to_lowercase().contains(&text.to_lowercase())
            }
        }
    }
}

/// Thread-safe log buffer for capturing tracing logs
pub struct LogBuffer {
    lines: Arc<Mutex<VecDeque<String>>>,
}

// Type alias for paused upstream snapshot entries:
// (obu_name, obu_mac, upstream_display, upstream_mac, hops, next_hop_mac)
type UpstreamSnapshotEntry = (String, String, String, String, String, String);

impl LogBuffer {
    pub fn new() -> Self {
        Self {
            lines: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    #[allow(dead_code)]
    pub fn push(&self, line: String) {
        let mut lines = self.lines.lock().unwrap();
        lines.push_back(line);
        if lines.len() > MAX_LOGS {
            lines.pop_front();
        }
    }

    #[allow(dead_code)]
    pub fn get_lines(&self) -> Vec<String> {
        self.lines.lock().unwrap().iter().cloned().collect()
    }

    pub fn clone_buffer(&self) -> Arc<Mutex<VecDeque<String>>> {
        Arc::clone(&self.lines)
    }
}

/// Custom tracing layer that captures logs to a buffer
pub struct TuiLogLayer {
    buffer: Arc<Mutex<VecDeque<String>>>,
}

impl TuiLogLayer {
    pub fn new(buffer: Arc<Mutex<VecDeque<String>>>) -> Self {
        Self { buffer }
    }
}

impl<S> Layer<S> for TuiLogLayer
where
    S: tracing::Subscriber,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        // Format the event
        let mut visitor = LogVisitor::new();
        event.record(&mut visitor);

        let level = event.metadata().level();
        let target = event.metadata().target();
        let message = visitor.message;

        let formatted = format!("[{:5}] {}: {}", level, target, message);

        // Add to buffer
        let mut lines = self.buffer.lock().unwrap();
        lines.push_back(formatted);
        if lines.len() > MAX_LOGS {
            lines.pop_front();
        }
    }
}

/// Visitor to extract message from tracing events
struct LogVisitor {
    message: String,
}

impl LogVisitor {
    fn new() -> Self {
        Self {
            message: String::new(),
        }
    }
}

/// Concrete channel stats computed at pause time for stable display (no Instants)
#[derive(Debug, Clone)]
struct DisplayChannelStats {
    packets_sent: u64,
    packets_dropped: u64,
    throughput_bps: f64,
    avg_latency_ms: f64,
}

/// Upstream information for a node (used by the Upstreams tab)
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct UpstreamInfo {
    /// Human-readable node name of the upstream RSU, if known
    pub upstream_node: Option<String>,
    /// Number of hops to the upstream
    pub hops: u32,
    /// Next-hop MAC address (as string)
    pub next_hop_mac: String,
}

impl tracing::field::Visit for LogVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{:?}", value);
            // Remove quotes from debug output
            if self.message.starts_with('"') && self.message.ends_with('"') {
                self.message = self.message[1..self.message.len() - 1].to_string();
            }
        } else {
            if !self.message.is_empty() {
                self.message.push_str(", ");
            }
            self.message
                .push_str(&format!("{}={:?}", field.name(), value));
        }
    }
}

/// TUI state maintaining historical data for graphs
struct TuiState {
    metrics: Arc<SimulatorMetrics>,
    // Map of nodes: name -> (device mac string, node_type_string, SimNode)
    nodes: std::collections::HashMap<String, (String, String, crate::simulator::SimNode)>,
    // Last time nodes were refreshed
    last_nodes_refresh: Instant,
    start_time: Instant,

    // Historical data for graphs
    packets_sent_history: Vec<(f64, f64)>,
    loss_percentage_history: Vec<(f64, f64)>,
    throughput_history: Vec<(f64, f64)>,
    latency_history: Vec<(f64, f64)>,

    // Previous values for calculating deltas
    prev_packets_sent: u64,
    prev_packets_dropped: u64,
    prev_timestamp: f64,

    // UI state
    active_tab: Tab,
    log_buffer: Arc<Mutex<VecDeque<String>>>,
    log_scroll: usize,
    log_auto_scroll: bool,
    channel_sort_mode: ChannelSortMode,
    channel_sort_direction: SortDirection,
    // Snapshots captured when paused
    paused_summary: Option<MetricsSummary>,
    // Concrete display snapshot for channels (precomputed throughput/latency at pause time)
    paused_channel_display: Option<HashMap<String, DisplayChannelStats>>,
    // Snapshot of upstream entries when paused: Vec of (obu_name, obu_mac, upstream_display, upstream_mac, hops, next_hop_mac)
    paused_upstreams: Option<Vec<UpstreamSnapshotEntry>>,
    log_filter: LogFilter,
    log_input_mode: bool,
    log_input_buffer: String,
    paused: bool,
}

impl TuiState {
    fn new(metrics: Arc<SimulatorMetrics>, log_buffer: Arc<Mutex<VecDeque<String>>>) -> Self {
        Self {
            metrics,
            nodes: std::collections::HashMap::new(),
            last_nodes_refresh: Instant::now(),
            start_time: Instant::now(),
            packets_sent_history: Vec::new(),
            loss_percentage_history: Vec::new(),
            throughput_history: Vec::new(),
            latency_history: Vec::new(),
            prev_packets_sent: 0,
            prev_packets_dropped: 0,
            prev_timestamp: 0.0,
            active_tab: Tab::Metrics,
            log_buffer,
            log_scroll: 0,
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
    fn refresh_nodes(&mut self, simulator: &crate::simulator::Simulator) {
        let sim_nodes = simulator.get_nodes();
        let map = sim_nodes
            .into_iter()
            .map(|(name, (device, _tun, node))| {
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
    fn update(&mut self) {
        let summary = self.metrics.summary();
        let elapsed = self.start_time.elapsed().as_secs_f64();

        // Calculate deltas for rate-based metrics
        let packets_sent_delta = summary.packets_sent.saturating_sub(self.prev_packets_sent);
        let _packets_dropped_delta = summary
            .packets_dropped
            .saturating_sub(self.prev_packets_dropped);
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

        // Keep only recent history
        if self.packets_sent_history.len() > MAX_HISTORY {
            self.packets_sent_history.remove(0);
            self.loss_percentage_history.remove(0);
            self.throughput_history.remove(0);
            self.latency_history.remove(0);
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

/// Render the TUI dashboard
fn ui(f: &mut Frame, state: &mut TuiState) {
    // Create main layout
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(3), // Title
            Constraint::Length(3), // Tabs
            Constraint::Min(10),   // Content
            Constraint::Length(3), // Help text
        ])
        .split(f.area());

    // Title
    let paused_label = if state.paused { " (PAUSED)" } else { "" };
    let title = Paragraph::new(vec![Line::from(vec![
        Span::styled(
            "Vigilant Parakeet ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("Simulator Dashboard{}", paused_label),
            Style::default().fg(Color::White),
        ),
    ])])
    .block(
        Block::default()
            .borders(Borders::ALL)
            .style(Style::default()),
    );
    f.render_widget(title, chunks[0]);

    // Tabs
    let tab_titles = vec!["📊 Metrics", "� Channels", "🔼 Upstreams", "�📜 Logs"];
    let tabs = Tabs::new(tab_titles)
        .block(Block::default().borders(Borders::ALL).title("View"))
        .select(match state.active_tab {
            Tab::Metrics => 0,
            Tab::Channels => 1,
            Tab::Upstreams => 2,
            Tab::Logs => 3,
        })
        .style(Style::default().fg(Color::White))
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
    f.render_widget(tabs, chunks[1]);

    // Render content based on active tab
    match state.active_tab {
        Tab::Metrics => render_metrics_tab(f, chunks[2], state),
        Tab::Channels => render_channels_tab(f, chunks[2], state),
        Tab::Upstreams => render_upstreams_tab(f, chunks[2], state),
        Tab::Logs => render_logs_tab(f, chunks[2], state),
    }

    // Help text - context-sensitive based on active tab
    let sort_mode_text = format!(" sort: {}  │  ", state.channel_sort_mode.as_str());
    let sort_dir_text = format!(" dir: {}  │  ", state.channel_sort_direction.as_str());
    let auto_scroll_text = format!(
        " auto-scroll: {}  │  ",
        if state.log_auto_scroll { "ON" } else { "OFF" }
    );
    let filter_text = format!(" filter: {}  │  ", state.log_filter.as_str());

    let help_spans = match state.active_tab {
        Tab::Metrics => vec![
            Span::styled(
                "Q/Esc/Ctrl+C",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" quit  │  ", Style::default().fg(Color::Gray)),
            Span::styled(
                "P",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" pause  │  ", Style::default().fg(Color::Gray)),
            Span::styled(
                "R",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" reset  │  ", Style::default().fg(Color::Gray)),
            Span::styled(
                "Tab/1/2/3/4",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" switch tabs", Style::default().fg(Color::Gray)),
        ],
        Tab::Channels => vec![
            Span::styled(
                "Q/Esc/Ctrl+C",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" quit  │  ", Style::default().fg(Color::Gray)),
            Span::styled(
                "P",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" pause  │  ", Style::default().fg(Color::Gray)),
            Span::styled(
                "S",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(sort_mode_text, Style::default().fg(Color::Gray)),
            Span::styled(
                "D",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(sort_dir_text, Style::default().fg(Color::Gray)),
            Span::styled(
                "Tab/1/2/3/4",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" switch tabs", Style::default().fg(Color::Gray)),
        ],
        Tab::Upstreams => vec![
            Span::styled(
                "Q/Esc/Ctrl+C",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" quit  │  ", Style::default().fg(Color::Gray)),
            Span::styled(
                "P",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" pause  │  ", Style::default().fg(Color::Gray)),
            Span::styled(
                "Tab/1/2/3/4",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" switch tabs", Style::default().fg(Color::Gray)),
        ],
        Tab::Logs => {
            if state.log_input_mode {
                vec![
                    Span::styled(
                        "Enter",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" apply filter  │  ", Style::default().fg(Color::Gray)),
                    Span::styled(
                        "Esc",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" cancel  │  ", Style::default().fg(Color::Gray)),
                    Span::styled(
                        "Backspace",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" delete char  │  ", Style::default().fg(Color::Gray)),
                    Span::styled(
                        "Type to filter (e.g., 'obu1', 'ERROR', etc.)",
                        Style::default().fg(Color::Cyan),
                    ),
                ]
            } else {
                vec![
                    Span::styled(
                        "Q/Esc",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" quit  │  ", Style::default().fg(Color::Gray)),
                    Span::styled(
                        "F",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(filter_text, Style::default().fg(Color::Gray)),
                    Span::styled(
                        "/",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" custom filter  │  ", Style::default().fg(Color::Gray)),
                    Span::styled(
                        "↑/↓",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" scroll  │  ", Style::default().fg(Color::Gray)),
                    Span::styled(
                        "End",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(auto_scroll_text, Style::default().fg(Color::Gray)),
                    Span::styled(
                        "Tab",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" switch", Style::default().fg(Color::Gray)),
                ]
            }
        }
    };

    let help = Paragraph::new(Line::from(help_spans))
        .block(Block::default().borders(Borders::ALL).title("Controls"));
    f.render_widget(help, chunks[3]);
}

/// Render the metrics tab content
fn render_metrics_tab(f: &mut Frame, area: Rect, state: &TuiState) {
    // If paused, use snapshot; otherwise get live summary
    let summary = if state.paused {
        state
            .paused_summary
            .as_ref()
            .cloned()
            .unwrap_or_else(|| state.metrics.summary())
    } else {
        state.metrics.summary()
    };

    // Create layout for metrics tab
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(6), // Stats summary
            Constraint::Min(10),   // Graphs
        ])
        .split(area);

    // Stats summary
    let uptime_secs = summary.uptime.as_secs();
    let uptime_str = format!(
        "{}h {}m {}s",
        uptime_secs / 3600,
        (uptime_secs % 3600) / 60,
        uptime_secs % 60
    );

    // Calculate loss percentage with color coding
    let loss_percentage = summary.drop_rate;
    let loss_color = if loss_percentage > 10.0 {
        Color::Red
    } else if loss_percentage > 5.0 {
        Color::Yellow
    } else if loss_percentage > 1.0 {
        Color::LightYellow
    } else {
        Color::Green
    };

    let stats_items = vec![
        ListItem::new(Line::from(vec![
            Span::styled("Total Packets: ", Style::default().fg(Color::White)),
            Span::raw(format!("{}", summary.total_packets)),
            Span::styled("  │  Packet Loss: ", Style::default().fg(loss_color)),
            Span::styled(
                format!("{:.2}%", loss_percentage),
                Style::default().fg(loss_color).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  │  Success Rate: ", Style::default().fg(Color::Green)),
            Span::raw(format!("{:.2}%", 100.0 - summary.drop_rate)),
        ])),
        ListItem::new(Line::from(vec![
            Span::styled("Avg Latency: ", Style::default().fg(Color::Cyan)),
            Span::raw(format!("{:.2}ms", summary.avg_latency_us / 1000.0)),
            Span::styled("  │  Throughput: ", Style::default().fg(Color::Magenta)),
            Span::raw(format!(
                "{:.1} pps",
                summary.packets_sent as f64 / summary.uptime.as_secs_f64()
            )),
        ])),
        ListItem::new(Line::from(vec![
            Span::styled("Active Nodes: ", Style::default().fg(Color::Green)),
            Span::raw(format!("{}", summary.active_nodes)),
            Span::styled("  │  Active Channels: ", Style::default().fg(Color::Green)),
            Span::raw(format!("{}", summary.active_channels)),
            Span::styled("  │  Uptime: ", Style::default().fg(Color::Blue)),
            Span::raw(uptime_str),
        ])),
    ];

    let stats_list =
        List::new(stats_items).block(Block::default().borders(Borders::ALL).title("Statistics"));
    f.render_widget(stats_list, chunks[0]);

    // Graphs section
    let graph_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[1]);

    let top_graph_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(graph_chunks[0]);

    let bottom_graph_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(graph_chunks[1]);

    // Packets sent graph
    render_chart(
        f,
        top_graph_chunks[0],
        "Packets Sent",
        &state.packets_sent_history,
        Color::Green,
    );

    // Packet loss percentage graph
    render_chart(
        f,
        top_graph_chunks[1],
        "Packet Loss (%)",
        &state.loss_percentage_history,
        Color::Red,
    );

    // Throughput graph
    render_chart(
        f,
        bottom_graph_chunks[0],
        "Throughput (pps)",
        &state.throughput_history,
        Color::Magenta,
    );

    // Latency graph
    render_chart(
        f,
        bottom_graph_chunks[1],
        "Avg Latency (ms)",
        &state.latency_history,
        Color::Cyan,
    );
}

/// Render the channels tab content showing per-channel statistics
fn render_channels_tab(f: &mut Frame, area: Rect, state: &TuiState) {
    use ratatui::widgets::{Cell, Table};

    // Get per-channel stats from metrics
    // Fetch live channel stats; if paused we'll try to use a precomputed display snapshot
    let live_stats = state.metrics.channel_stats();
    let display_map = state.paused_channel_display.as_ref();

    // Create header row and highlight the active sort column
    let header_highlight = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let header_normal = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);

    // Add arrow to indicate sort direction on the active column
    let name_label = if state.channel_sort_mode == ChannelSortMode::Name {
        format!("Name {}", state.channel_sort_direction.arrow())
    } else {
        "Name".to_string()
    };
    let loss_label = if state.channel_sort_mode == ChannelSortMode::Loss {
        format!("Loss % {}", state.channel_sort_direction.arrow())
    } else {
        "Loss %".to_string()
    };
    let throughput_label = if state.channel_sort_mode == ChannelSortMode::Throughput {
        format!("Throughput {}", state.channel_sort_direction.arrow())
    } else {
        "Throughput".to_string()
    };
    let latency_label = if state.channel_sort_mode == ChannelSortMode::Latency {
        format!("Avg Latency {}", state.channel_sort_direction.arrow())
    } else {
        "Avg Latency".to_string()
    };

    let header_cells = vec![
        Cell::from(name_label).style(if state.channel_sort_mode == ChannelSortMode::Name {
            header_highlight
        } else {
            header_normal
        }),
        Cell::from(loss_label).style(if state.channel_sort_mode == ChannelSortMode::Loss {
            header_highlight
        } else {
            header_normal
        }),
        Cell::from(throughput_label).style(
            if state.channel_sort_mode == ChannelSortMode::Throughput {
                header_highlight
            } else {
                header_normal
            },
        ),
        Cell::from(latency_label).style(if state.channel_sort_mode == ChannelSortMode::Latency {
            header_highlight
        } else {
            header_normal
        }),
    ];

    let header = Row::new(header_cells).bottom_margin(1);

    // Create data rows - compute either from display snapshot (if paused) or live_stats
    let mut channel_data: Vec<_> = live_stats
        .iter()
        .map(|(name, stats)| {
            if let Some(display) = display_map.and_then(|m| m.get(name)) {
                let total = display.packets_sent + display.packets_dropped;
                let loss_rate = if total > 0 {
                    (display.packets_dropped as f64 / total as f64) * 100.0
                } else {
                    0.0
                };
                (
                    name.clone(),
                    // create a lightweight clone of the original stats for compatibility (not used for time-based calc)
                    ChannelStats { ..stats.clone() },
                    loss_rate,
                    display.throughput_bps,
                    display.avg_latency_ms,
                )
            } else {
                let total = stats.packets_sent + stats.packets_dropped;
                let loss_rate = if total > 0 {
                    (stats.packets_dropped as f64 / total as f64) * 100.0
                } else {
                    0.0
                };
                let throughput_bps = stats.throughput_last(10) * 8.0;
                let avg_latency_ms = if stats.packets_delayed > 0 {
                    (stats.total_latency_us as f64 / stats.packets_delayed as f64) / 1000.0
                } else {
                    0.0
                };
                (
                    name.clone(),
                    stats.clone(),
                    loss_rate,
                    throughput_bps,
                    avg_latency_ms,
                )
            }
        })
        .collect();

    // Sort based on current sort mode
    // Apply sorting and respect direction
    match state.channel_sort_mode {
        ChannelSortMode::Loss => {
            channel_data.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal));
            if state.channel_sort_direction == SortDirection::Desc {
                channel_data.reverse();
            }
        }
        ChannelSortMode::Throughput => {
            channel_data.sort_by(|a, b| a.3.partial_cmp(&b.3).unwrap_or(std::cmp::Ordering::Equal));
            if state.channel_sort_direction == SortDirection::Desc {
                channel_data.reverse();
            }
        }
        ChannelSortMode::Latency => {
            channel_data.sort_by(|a, b| a.4.partial_cmp(&b.4).unwrap_or(std::cmp::Ordering::Equal));
            if state.channel_sort_direction == SortDirection::Desc {
                channel_data.reverse();
            }
        }
        ChannelSortMode::Name => {
            if state.channel_sort_direction == SortDirection::Asc {
                channel_data.sort_by(|a, b| a.0.cmp(&b.0));
            } else {
                channel_data.sort_by(|a, b| b.0.cmp(&a.0));
            }
        }
    }

    let rows: Vec<Row> = channel_data
        .iter()
        .map(
            |(channel, stats, loss_rate, throughput_bps, avg_latency_ms)| {
                let total = stats.packets_sent + stats.packets_dropped;

                // Color code the loss rate
                let loss_color = if *loss_rate < 1.0 {
                    Color::Green
                } else if *loss_rate < 10.0 {
                    Color::Yellow
                } else {
                    Color::Red
                };

                // Show ratio in addition to percentage for clarity
                let rate_display = if total > 0 {
                    format!("{:.1}% ({}/{})", loss_rate, stats.packets_dropped, total)
                } else {
                    "N/A".to_string()
                };

                // Format throughput in human-readable units (bits/sec -> Kbps/Mbps/Gbps)
                let throughput_display = format_bits_per_sec(*throughput_bps);

                // Format latency
                let latency_display = if stats.packets_delayed > 0 {
                    format!("{:.2} ms", avg_latency_ms)
                } else {
                    "N/A".to_string()
                };

                // Build cells and apply highlight style to the active sort column
                {
                    let mut cell_channel = Cell::from(channel.to_string());
                    let mut cell_loss = Cell::from(rate_display)
                        .style(Style::default().fg(loss_color).add_modifier(Modifier::BOLD));
                    let mut cell_throughput = Cell::from(throughput_display);
                    let mut cell_latency = Cell::from(latency_display);

                    // Highlight active column (use yellow bold for highlight)
                    let data_highlight = Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD);
                    match state.channel_sort_mode {
                        ChannelSortMode::Name => cell_channel = cell_channel.style(data_highlight),
                        ChannelSortMode::Loss => cell_loss = cell_loss.style(data_highlight),
                        ChannelSortMode::Throughput => {
                            cell_throughput = cell_throughput.style(data_highlight)
                        }
                        ChannelSortMode::Latency => {
                            cell_latency = cell_latency.style(data_highlight)
                        }
                    }

                    Row::new(vec![cell_channel, cell_loss, cell_throughput, cell_latency])
                }
                .height(1)
            },
        )
        .collect();

    let widths = [
        Constraint::Percentage(40), // Channel
        Constraint::Percentage(20), // Loss %
        Constraint::Percentage(20), // Throughput
        Constraint::Percentage(20), // Avg Latency
    ];

    let title = format!(
        "Per-Channel Statistics (sorted by: {})",
        state.channel_sort_mode.as_str()
    );
    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).title(title))
        .style(Style::default().fg(Color::White));

    f.render_widget(table, area);
}

/// Render a tab that shows, for each OBU, which RSU is its upstream, hops, and next hop MAC
fn render_upstreams_tab(f: &mut Frame, area: Rect, state: &TuiState) {
    use ratatui::widgets::{Cell, Row, Table};
    // If paused and we have a snapshot, use that; otherwise compute live entries from state.nodes
    let rows: Vec<Row> = if state.paused {
        if let Some(ref ups) = state.paused_upstreams {
            let mut entries: Vec<(String, Vec<Cell>)> = ups
                .iter()
                .map(|(name, obu_mac, up_display, up_mac, hops, next_hop)| {
                    let obu_label = format!("{} ({})", name, obu_mac);
                    let up_label = if up_display.starts_with("(") || up_display.contains(':') {
                        // if up_display is a mac or placeholder like (no upstream), prefer showing name + mac when possible
                        if up_display.starts_with('(') {
                            format!(
                                "{} ({})",
                                up_display.trim_matches(|c| c == '(' || c == ')'),
                                up_mac
                            )
                        } else {
                            format!("{} ({})", up_display, up_mac)
                        }
                    } else {
                        format!("{} ({})", up_display, up_mac)
                    };
                    let cells = vec![
                        Cell::from(obu_label),
                        Cell::from(up_label),
                        Cell::from(hops.clone()),
                        Cell::from(next_hop.clone()),
                    ];
                    (name.clone(), cells)
                })
                .collect();

            // Ensure alphabetical order while paused
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            entries
                .into_iter()
                .map(|(_n, cells)| Row::new(cells).height(1))
                .collect()
        } else {
            Vec::new()
        }
    } else {
        // Build live entries from state.nodes where node_type == "Obu", then sort by OBU name
        let mut entries: Vec<(String, Vec<Cell>)> = Vec::new();

        for (name, (mac, node_type, simnode)) in state.nodes.iter() {
            if node_type != "Obu" {
                continue;
            }

            // Build labels with MAC addresses
            let obu_label = format!("{} ({})", name, mac);

            // Try to downcast to Obu to get cached upstream route
            let mut upstream_display = "(no upstream)".to_string();
            let mut hops = "-".to_string();
            let mut next_hop = "-".to_string();

            // Use SimNode's as_any to downcast to obu_lib::Obu
            if let crate::simulator::SimNode::Obu(ref o) = simnode {
                // oba is Arc<dyn Node>; try downcast via as_any
                if let Some(obu) = o.as_any().downcast_ref::<obu_lib::Obu>() {
                    if let Some(route) = obu.cached_upstream_route() {
                        // Immediate next hop (first hop on path)
                        let immediate_next = format!("{}", route.mac);
                        next_hop = immediate_next.clone();

                        // Try to resolve final RSU by walking the upstream chain.
                        let mut total_hops = route.hops;
                        let mut current_mac = format!("{}", route.mac);
                        let mut depth = 0;
                        let final_name = loop {
                            if depth > 16 {
                                break None;
                            }
                            depth += 1;
                            if let Some((nname, (_m, ntype, snode))) =
                                state.nodes.iter().find(|(_, (m, _, _))| **m == current_mac)
                            {
                                if ntype == "Rsu" {
                                    break Some(nname.clone());
                                }
                                if ntype == "Obu" {
                                    if let crate::simulator::SimNode::Obu(ref other_o) = snode {
                                        if let Some(other_obu) =
                                            other_o.as_any().downcast_ref::<obu_lib::Obu>()
                                        {
                                            if let Some(next_route) =
                                                other_obu.cached_upstream_route()
                                            {
                                                total_hops =
                                                    total_hops.saturating_add(next_route.hops);
                                                current_mac = format!("{}", next_route.mac);
                                                continue;
                                            }
                                        }
                                    }
                                    break None;
                                }
                                break None;
                            } else {
                                break None;
                            }
                        };

                        hops = final_name
                            .as_ref()
                            .map(|_| format!("{}", total_hops))
                            .unwrap_or_else(|| format!("{}", route.hops));
                        upstream_display = final_name.unwrap_or_else(|| format!("{}", route.mac));
                    }
                }
            }

            // If upstream_display is a name, try to lookup its mac for display
            let upstream_label =
                if upstream_display.starts_with('(') || upstream_display.contains(':') {
                    // Either (no upstream) or a MAC
                    upstream_display.to_string()
                } else {
                    // Lookup by name
                    if let Some((_, (umac, _, _))) =
                        state.nodes.iter().find(|(n, _)| *n == &upstream_display)
                    {
                        format!("{} ({})", upstream_display, umac)
                    } else {
                        upstream_display.to_string()
                    }
                };

            let cells = vec![
                Cell::from(obu_label),
                Cell::from(upstream_label),
                Cell::from(hops),
                Cell::from(next_hop),
            ];
            entries.push((name.clone(), cells));
        }

        // Sort alphabetically by OBU name
        entries.sort_by(|a, b| a.0.cmp(&b.0));

        // Convert to table rows
        entries
            .into_iter()
            .map(|(_name, cells)| Row::new(cells).height(1))
            .collect()
    };

    let header = Row::new(vec![
        Cell::from("OBU"),
        Cell::from("Upstream RSU"),
        Cell::from("Hops"),
        Cell::from("Next Hop MAC"),
    ])
    .bottom_margin(1);

    let widths = [
        Constraint::Percentage(30),
        Constraint::Percentage(40),
        Constraint::Percentage(10),
        Constraint::Percentage(20),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("OBU Upstreams"),
        )
        .style(Style::default().fg(Color::White));

    f.render_widget(table, area);
}

/// Render the logs tab content
fn render_logs_tab(f: &mut Frame, area: Rect, state: &mut TuiState) {
    let logs = state.log_buffer.lock().unwrap();

    // Filter logs based on current filter
    let filtered_logs: Vec<&String> = logs
        .iter()
        .filter(|line| {
            // Extract target from log line format: "[LEVEL] target: message"
            if let Some(colon_pos) = line.find(':') {
                if let Some(bracket_end) = line.find(']') {
                    if bracket_end < colon_pos {
                        let target = line[bracket_end + 2..colon_pos].trim();
                        return state.log_filter.matches(target, line);
                    }
                }
            }
            // For lines that don't match expected format, still apply custom filter
            matches!(state.log_filter, LogFilter::All | LogFilter::Custom(_))
                && state.log_filter.matches("", line)
        })
        .collect();

    let log_count = filtered_logs.len();

    // Clamp scroll position to valid range
    if state.log_scroll >= log_count && log_count > 0 {
        state.log_scroll = log_count - 1;
    }

    // Convert logs to ListItems with color-coded levels
    let log_items: Vec<ListItem> = filtered_logs
        .iter()
        .map(|line| {
            // Try to detect log level and colorize accordingly
            let styled_line = if line.contains("ERROR") {
                Line::from(Span::styled(
                    (*line).clone(),
                    Style::default().fg(Color::Red),
                ))
            } else if line.contains("WARN") {
                Line::from(Span::styled(
                    (*line).clone(),
                    Style::default().fg(Color::Yellow),
                ))
            } else if line.contains("INFO") {
                Line::from(Span::styled(
                    (*line).clone(),
                    Style::default().fg(Color::Green),
                ))
            } else if line.contains("DEBUG") {
                Line::from(Span::styled(
                    (*line).clone(),
                    Style::default().fg(Color::Cyan),
                ))
            } else if line.contains("TRACE") {
                Line::from(Span::styled(
                    (*line).clone(),
                    Style::default().fg(Color::Gray),
                ))
            } else {
                Line::from((*line).clone())
            };

            ListItem::new(styled_line)
        })
        .collect();

    let auto_scroll_indicator = if state.log_auto_scroll { "🔄 " } else { "" };
    let filter_text = state.log_filter.as_str();
    let input_indicator = if state.log_input_mode {
        format!(" [INPUT: {}█]", state.log_input_buffer)
    } else {
        String::new()
    };
    let title = format!(
        "{}Logs ({} lines, filter: {}){}",
        auto_scroll_indicator, log_count, filter_text, input_indicator
    );
    let logs_list = List::new(log_items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .style(Style::default().fg(Color::White));

    // Create list state for scrolling
    let mut list_state = ListState::default();
    list_state.select(Some(state.log_scroll));

    f.render_stateful_widget(logs_list, area, &mut list_state);
}

/// Render a single chart with historical data
fn render_chart(f: &mut Frame, area: Rect, title: &str, data: &[(f64, f64)], color: Color) {
    if data.is_empty() {
        let empty = Paragraph::new("No data yet...")
            .block(Block::default().borders(Borders::ALL).title(title))
            .style(Style::default().fg(Color::Gray));
        f.render_widget(empty, area);
        return;
    }

    let dataset = vec![Dataset::default()
        .name(title)
        .marker(symbols::Marker::Braille)
        .graph_type(GraphType::Line)
        .style(Style::default().fg(color))
        .data(data)];

    let min_x = data.first().map(|(x, _)| *x).unwrap_or(0.0);
    let max_x = data.last().map(|(x, _)| *x).unwrap_or(60.0);
    let min_y = data
        .iter()
        .map(|(_, y)| *y)
        .fold(f64::INFINITY, f64::min)
        .min(0.0);
    let max_y = data
        .iter()
        .map(|(_, y)| *y)
        .fold(f64::NEG_INFINITY, f64::max)
        .max(1.0);

    // Add 10% padding to y-axis
    let y_padding = (max_y - min_y) * 0.1;
    let chart_min_y = (min_y - y_padding).max(0.0);
    let chart_max_y = max_y + y_padding;

    let chart = Chart::new(dataset)
        .block(Block::default().borders(Borders::ALL).title(title))
        .x_axis(
            Axis::default()
                .style(Style::default().fg(Color::Gray))
                .bounds([min_x, max_x])
                .labels(vec![
                    Span::raw(format!("{:.0}s", min_x)),
                    Span::raw(format!("{:.0}s", max_x)),
                ]),
        )
        .y_axis(
            Axis::default()
                .style(Style::default().fg(Color::Gray))
                .bounds([chart_min_y, chart_max_y])
                .labels(vec![
                    Span::raw(format!("{:.1}", chart_min_y)),
                    Span::raw(format!("{:.1}", chart_max_y)),
                ]),
        );

    f.render_widget(chart, area);
}

/// Format bits-per-second into a human-scaled string like "1.23 Mbps" using `human_format`.
fn format_bits_per_sec(bps: f64) -> String {
    if !bps.is_finite() || bps <= 0.0 {
        return "0 bps".to_string();
    }

    // human_format works with f64 and will choose an appropriate suffix.
    // We prefer SI-style scaling (k/M/G) where k == 1000.
    let mut base = Formatter::new();
    let fmt = base.with_decimals(2);
    // human_format returns a string like "1.23K" or "123". We'll parse the optional
    // alphabetic suffix and map it to K/M/G for network units.
    let formatted = fmt.format(bps);

    if let Some(last) = formatted.chars().last() {
        if last.is_ascii_alphabetic() {
            let num_part = &formatted[..formatted.len() - 1];
            let unit = match last {
                'K' => "Kbps",
                'M' => "Mbps",
                'G' => "Gbps",
                'T' => "Tbps",
                other => {
                    // Unknown suffix, fall back to raw suffix + "bps"
                    return format!("{} {}bps", num_part, other);
                }
            };
            return format!("{} {}", num_part, unit);
        }
    }

    // No suffix: treat as plain bps
    format!("{} bps", formatted)
}

/// Run the TUI dashboard
///
/// This function takes over the terminal and displays a real-time dashboard
/// until the user presses 'q', 'Q', Esc, or Ctrl+C to quit.
pub async fn run_tui(
    metrics: Arc<SimulatorMetrics>,
    log_buffer: Arc<Mutex<VecDeque<String>>>,
    simulator: Arc<crate::simulator::Simulator>,
) -> Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app state
    let mut state = TuiState::new(metrics, log_buffer);
    // Initial nodes snapshot
    state.refresh_nodes(&simulator);

    // Set up a panic hook to ensure terminal is restored even on panic
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        original_hook(panic_info);
    }));

    // Run the TUI loop
    let res = run_tui_loop(&mut terminal, &mut state, simulator.clone()).await;

    // Restore terminal - always run this even if error occurred
    let _ = disable_raw_mode();
    let _ = execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    );
    let _ = terminal.show_cursor();

    // Restore original panic hook
    let _ = std::panic::take_hook();

    res
}

/// Main TUI event loop
async fn run_tui_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: &mut TuiState,
    simulator: Arc<crate::simulator::Simulator>,
) -> Result<()> {
    let mut update_interval = interval(Duration::from_millis(250)); // Update 4 times per second for responsiveness
    update_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        // Handle events first with very short timeout for instant response
        if event::poll(Duration::from_millis(16))? {
            // ~60 FPS polling
            if let Event::Key(key) = event::read()? {
                // Handle both Press and Repeat events (some terminals only send one)
                if key.kind == KeyEventKind::Press || key.kind == KeyEventKind::Repeat {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Char('Q') => {
                            tracing::info!("Quit command received");
                            return Ok(());
                        }
                        KeyCode::Esc => {
                            // Exit input mode if active, otherwise quit
                            if state.log_input_mode {
                                state.log_input_mode = false;
                                state.log_input_buffer.clear();
                            } else {
                                tracing::info!("Escape key received");
                                return Ok(());
                            }
                        }
                        KeyCode::Char('c')
                            if key
                                .modifiers
                                .contains(crossterm::event::KeyModifiers::CONTROL) =>
                        {
                            tracing::info!("Ctrl+C received");
                            return Ok(());
                        }
                        KeyCode::Tab | KeyCode::BackTab => {
                            // Switch tabs (not in input mode)
                            if !state.log_input_mode {
                                state.active_tab = match state.active_tab {
                                    Tab::Metrics => Tab::Channels,
                                    Tab::Channels => Tab::Upstreams,
                                    Tab::Upstreams => Tab::Logs,
                                    Tab::Logs => Tab::Metrics,
                                };
                            }
                        }
                        KeyCode::Char('1') => {
                            if !state.log_input_mode {
                                state.active_tab = Tab::Metrics;
                            }
                        }
                        KeyCode::Char('2') => {
                            if !state.log_input_mode {
                                state.active_tab = Tab::Channels;
                            }
                        }
                        KeyCode::Char('3') => {
                            if !state.log_input_mode {
                                state.active_tab = Tab::Upstreams;
                            }
                        }
                        KeyCode::Char('4') => {
                            if !state.log_input_mode {
                                state.active_tab = Tab::Logs;
                            }
                        }
                        KeyCode::Char('r') | KeyCode::Char('R') => {
                            state.metrics.reset();
                            state.packets_sent_history.clear();
                            state.loss_percentage_history.clear();
                            state.throughput_history.clear();
                            state.latency_history.clear();
                            state.prev_packets_sent = 0;
                            state.prev_packets_dropped = 0;
                            state.prev_timestamp = 0.0;
                            state.start_time = Instant::now();
                        }
                        KeyCode::Char('s') | KeyCode::Char('S') => {
                            // Cycle through sort modes (only on Channels tab)
                            if state.active_tab == Tab::Channels {
                                state.channel_sort_mode = state.channel_sort_mode.next();
                            }
                        }
                        KeyCode::Char('p') | KeyCode::Char('P') => {
                            // Toggle pause for the UI
                            state.paused = !state.paused;
                            if state.paused {
                                // Capture snapshots
                                state.paused_summary = Some(state.metrics.summary());
                                // Compute concrete display snapshot for channels so values don't change over time
                                let live = state.metrics.channel_stats();
                                let mut display_map = std::collections::HashMap::new();
                                for (k, v) in live.into_iter() {
                                    let throughput_bps = v.throughput_last(10) * 8.0; // bytes/sec -> bits/sec
                                    let avg_latency_ms = if v.packets_delayed > 0 {
                                        (v.total_latency_us as f64 / v.packets_delayed as f64)
                                            / 1000.0
                                    } else {
                                        0.0
                                    };
                                    display_map.insert(
                                        k.clone(),
                                        DisplayChannelStats {
                                            packets_sent: v.packets_sent,
                                            packets_dropped: v.packets_dropped,
                                            throughput_bps,
                                            avg_latency_ms,
                                        },
                                    );
                                }
                                state.paused_channel_display = Some(display_map);
                                // Capture upstreams snapshot
                                let mut ups: Vec<(String, String, String, String, String, String)> =
                                    Vec::new();
                                for (name, (mac, ntype, simnode)) in state.nodes.iter() {
                                    if ntype != "Obu" {
                                        continue;
                                    }
                                    let obu_mac = mac.clone();
                                    let mut upstream_display = "(no upstream)".to_string();
                                    let mut upstream_mac = "-".to_string();
                                    let mut hops = "-".to_string();
                                    let mut next_hop = "-".to_string();

                                    if let crate::simulator::SimNode::Obu(ref o) = simnode {
                                        if let Some(obu) = o.as_any().downcast_ref::<obu_lib::Obu>()
                                        {
                                            if let Some(route) = obu.cached_upstream_route() {
                                                upstream_mac = format!("{}", route.mac);
                                                next_hop = upstream_mac.clone();
                                                // Attempt to resolve final RSU name and total hops like in render_upstreams_tab
                                                let mut total_hops = route.hops;
                                                let mut current_mac = format!("{}", route.mac);
                                                let mut depth = 0;
                                                let final_name = loop {
                                                    if depth > 16 {
                                                        break None;
                                                    }
                                                    depth += 1;
                                                    if let Some((nname, (_m, ntype2, snode))) =
                                                        state.nodes.iter().find(|(_, (m, _, _))| {
                                                            **m == current_mac
                                                        })
                                                    {
                                                        if ntype2 == "Rsu" {
                                                            break Some(nname.clone());
                                                        }
                                                        if ntype2 == "Obu" {
                                                            if let crate::simulator::SimNode::Obu(
                                                                ref other_o,
                                                            ) = snode
                                                            {
                                                                if let Some(other_obu) = other_o
                                                                    .as_any()
                                                                    .downcast_ref::<obu_lib::Obu>()
                                                                {
                                                                    if let Some(next_route) =
                                                                        other_obu
                                                                            .cached_upstream_route()
                                                                    {
                                                                        total_hops = total_hops
                                                                            .saturating_add(
                                                                                next_route.hops,
                                                                            );
                                                                        current_mac = format!(
                                                                            "{}",
                                                                            next_route.mac
                                                                        );
                                                                        continue;
                                                                    }
                                                                }
                                                            }
                                                            break None;
                                                        }
                                                        break None;
                                                    } else {
                                                        break None;
                                                    }
                                                };
                                                hops = final_name
                                                    .as_ref()
                                                    .map(|_| format!("{}", total_hops))
                                                    .unwrap_or_else(|| format!("{}", route.hops));
                                                upstream_display = final_name
                                                    .unwrap_or_else(|| format!("{}", route.mac));
                                            }
                                        }
                                    }

                                    ups.push((
                                        name.clone(),
                                        obu_mac.clone(),
                                        upstream_display,
                                        upstream_mac,
                                        hops,
                                        next_hop,
                                    ));
                                }
                                state.paused_upstreams = Some(ups);
                            } else {
                                // Clear snapshots and enable auto-scroll
                                state.paused_summary = None;
                                state.paused_channel_display = None;
                                state.log_auto_scroll = true;
                                state.paused_upstreams = None;
                            }
                        }
                        KeyCode::Char('d') | KeyCode::Char('D') => {
                            // Toggle sort direction (only on Channels tab)
                            if state.active_tab == Tab::Channels {
                                state.channel_sort_direction =
                                    state.channel_sort_direction.toggle();
                            }
                        }
                        KeyCode::Char('f') | KeyCode::Char('F') => {
                            // Cycle through log filters (only on Logs tab, not in input mode)
                            if state.active_tab == Tab::Logs && !state.log_input_mode {
                                state.log_filter = state.log_filter.next();
                                // Reset scroll when changing filter
                                state.log_scroll = 0;
                                state.log_auto_scroll = true;
                            }
                        }
                        KeyCode::Char('/') => {
                            // Enter custom filter input mode (only on Logs tab, not already in input mode)
                            if state.active_tab == Tab::Logs && !state.log_input_mode {
                                state.log_input_mode = true;
                                state.log_input_buffer.clear();
                            }
                        }
                        KeyCode::Char(c) => {
                            // Add character to input buffer when in input mode
                            if state.log_input_mode {
                                state.log_input_buffer.push(c);
                            }
                        }
                        KeyCode::Backspace => {
                            // Remove last character from input buffer when in input mode
                            if state.log_input_mode {
                                state.log_input_buffer.pop();
                            }
                        }
                        KeyCode::Enter => {
                            // Apply custom filter when in input mode
                            if state.log_input_mode {
                                if state.log_input_buffer.is_empty() {
                                    // Empty input returns to All filter
                                    state.log_filter = LogFilter::All;
                                } else {
                                    // Apply custom filter
                                    state.log_filter =
                                        LogFilter::Custom(state.log_input_buffer.clone());
                                }
                                state.log_input_mode = false;
                                state.log_scroll = 0;
                                state.log_auto_scroll = true;
                            }
                        }
                        KeyCode::Up => {
                            // Scroll up in logs (only on Logs tab)
                            if state.active_tab == Tab::Logs && state.log_scroll > 0 {
                                state.log_scroll -= 1;
                                state.log_auto_scroll = false;
                            }
                        }
                        KeyCode::Down => {
                            // Scroll down in logs (only on Logs tab)
                            if state.active_tab == Tab::Logs {
                                state.log_scroll += 1;
                                state.log_auto_scroll = false;
                            }
                        }
                        KeyCode::PageUp => {
                            // Scroll up by 10 lines (only on Logs tab)
                            if state.active_tab == Tab::Logs {
                                state.log_scroll = state.log_scroll.saturating_sub(10);
                                state.log_auto_scroll = false;
                            }
                        }
                        KeyCode::PageDown => {
                            // Scroll down by 10 lines (only on Logs tab)
                            if state.active_tab == Tab::Logs {
                                state.log_scroll = state.log_scroll.saturating_add(10);
                                state.log_auto_scroll = false;
                            }
                        }
                        KeyCode::Home => {
                            // Go to top (only on Logs tab)
                            if state.active_tab == Tab::Logs {
                                state.log_scroll = 0;
                                state.log_auto_scroll = false;
                            }
                        }
                        KeyCode::End => {
                            // Go to bottom and re-enable auto-scroll (only on Logs tab)
                            if state.active_tab == Tab::Logs {
                                let log_count = state.log_buffer.lock().unwrap().len();
                                state.log_scroll = log_count.saturating_sub(1);
                                state.log_auto_scroll = true;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        // Update metrics and redraw periodically
        tokio::select! {
                _ = update_interval.tick() => {
                if !state.paused {
                    state.update();
                }
                // Refresh nodes every 1s to pick up topology changes
                if state.last_nodes_refresh.elapsed() > Duration::from_secs(1) {
                    state.refresh_nodes(&simulator);
                }
                terminal.draw(|f| ui(f, state))?;
            }
            // If no events, just redraw to keep UI responsive
            _ = tokio::time::sleep(Duration::from_millis(16)) => {
                terminal.draw(|f| ui(f, state))?;
            }
        }
    }
}
