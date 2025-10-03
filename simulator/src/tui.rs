//! Terminal User Interface for real-time simulation monitoring
//!
//! Provides an interactive dashboard displaying:
//! - Packet statistics (sent, dropped, delayed)
//! - Performance metrics (drop rate, latency, throughput)
//! - Resource information (active nodes, channels)
//! - Live sparkline graphs showing trends over time
//! - Captured logs in a separate tab

use crate::metrics::SimulatorMetrics;
use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    symbols,
    text::{Line, Span},
    widgets::{
        Axis, Block, Borders, Chart, Dataset, GraphType, List, ListItem, Paragraph, Tabs,
    },
    Frame, Terminal,
};
use std::{
    collections::VecDeque,
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
    Logs,
}

/// Thread-safe log buffer for capturing tracing logs
pub struct LogBuffer {
    lines: Arc<Mutex<VecDeque<String>>>,
}

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
            self.message.push_str(&format!("{}={:?}", field.name(), value));
        }
    }
}

/// TUI state maintaining historical data for graphs
struct TuiState {
    metrics: Arc<SimulatorMetrics>,
    start_time: Instant,
    
    // Historical data for graphs
    packets_sent_history: Vec<(f64, f64)>,
    packets_dropped_history: Vec<(f64, f64)>,
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
}

impl TuiState {
    fn new(metrics: Arc<SimulatorMetrics>, log_buffer: Arc<Mutex<VecDeque<String>>>) -> Self {
        Self {
            metrics,
            start_time: Instant::now(),
            packets_sent_history: Vec::new(),
            packets_dropped_history: Vec::new(),
            throughput_history: Vec::new(),
            latency_history: Vec::new(),
            prev_packets_sent: 0,
            prev_packets_dropped: 0,
            prev_timestamp: 0.0,
            active_tab: Tab::Metrics,
            log_buffer,
            log_scroll: 0,
        }
    }

    /// Update historical data with current metrics
    fn update(&mut self) {
        let summary = self.metrics.summary();
        let elapsed = self.start_time.elapsed().as_secs_f64();

        // Calculate deltas for rate-based metrics
        let packets_sent_delta = summary.packets_sent.saturating_sub(self.prev_packets_sent);
        let _packets_dropped_delta = summary.packets_dropped.saturating_sub(self.prev_packets_dropped);
        let time_delta = elapsed - self.prev_timestamp;
        
        let current_throughput = if time_delta > 0.0 {
            packets_sent_delta as f64 / time_delta
        } else {
            0.0
        };

        // Add new data points
        self.packets_sent_history.push((elapsed, summary.packets_sent as f64));
        self.packets_dropped_history.push((elapsed, summary.packets_dropped as f64));
        self.throughput_history.push((elapsed, current_throughput));
        self.latency_history.push((elapsed, summary.avg_latency_us / 1000.0)); // Convert to ms

        // Keep only recent history
        if self.packets_sent_history.len() > MAX_HISTORY {
            self.packets_sent_history.remove(0);
            self.packets_dropped_history.remove(0);
            self.throughput_history.remove(0);
            self.latency_history.remove(0);
        }

        // Update previous values
        self.prev_packets_sent = summary.packets_sent;
        self.prev_packets_dropped = summary.packets_dropped;
        self.prev_timestamp = elapsed;
    }
}

/// Render the TUI dashboard
fn ui(f: &mut Frame, state: &TuiState) {
    // Create main layout
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(3),  // Title
            Constraint::Length(3),  // Tabs
            Constraint::Min(10),    // Content
            Constraint::Length(3),  // Help text
        ])
        .split(f.area());

    // Title
    let title = Paragraph::new(vec![Line::from(vec![
        Span::styled("Vigilant Parakeet ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::styled("Simulator Dashboard", Style::default().fg(Color::White)),
    ])])
    .block(Block::default().borders(Borders::ALL).style(Style::default()));
    f.render_widget(title, chunks[0]);

    // Tabs
    let tab_titles = vec!["📊 Metrics", "📜 Logs"];
    let tabs = Tabs::new(tab_titles)
        .block(Block::default().borders(Borders::ALL).title("View"))
        .select(match state.active_tab {
            Tab::Metrics => 0,
            Tab::Logs => 1,
        })
        .style(Style::default().fg(Color::White))
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        );
    f.render_widget(tabs, chunks[1]);

    // Render content based on active tab
    match state.active_tab {
        Tab::Metrics => render_metrics_tab(f, chunks[2], state),
        Tab::Logs => render_logs_tab(f, chunks[2], state),
    }

    // Help text
    let help = Paragraph::new(Line::from(vec![
        Span::styled("Q/Esc/Ctrl+C", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        Span::styled(" quit  │  ", Style::default().fg(Color::Gray)),
        Span::styled("R", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        Span::styled(" reset  │  ", Style::default().fg(Color::Gray)),
        Span::styled("Tab/1/2", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        Span::styled(" switch tabs  │  ", Style::default().fg(Color::Gray)),
        Span::styled("↑/↓/PgUp/PgDn", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        Span::styled(" scroll logs", Style::default().fg(Color::Gray)),
    ]))
    .block(Block::default().borders(Borders::ALL).title("Controls"));
    f.render_widget(help, chunks[3]);
}

/// Render the metrics tab content
fn render_metrics_tab(f: &mut Frame, area: Rect, state: &TuiState) {
    let summary = state.metrics.summary();

    // Create layout for metrics tab
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(6),  // Stats summary
            Constraint::Min(10),    // Graphs
        ])
        .split(area);

    // Stats summary
    let uptime_secs = summary.uptime.as_secs();
    let uptime_str = format!("{}h {}m {}s", uptime_secs / 3600, (uptime_secs % 3600) / 60, uptime_secs % 60);
    
    let stats_items = vec![
        ListItem::new(Line::from(vec![
            Span::styled("Packets Sent: ", Style::default().fg(Color::Green)),
            Span::raw(format!("{}", summary.packets_sent)),
            Span::styled("  │  Dropped: ", Style::default().fg(Color::Red)),
            Span::raw(format!("{}", summary.packets_dropped)),
            Span::styled("  │  Total: ", Style::default().fg(Color::White)),
            Span::raw(format!("{}", summary.total_packets)),
        ])),
        ListItem::new(Line::from(vec![
            Span::styled("Drop Rate: ", Style::default().fg(Color::Yellow)),
            Span::raw(format!("{:.2}%", summary.drop_rate * 100.0)),
            Span::styled("  │  Avg Latency: ", Style::default().fg(Color::Cyan)),
            Span::raw(format!("{:.2}ms", summary.avg_latency_us / 1000.0)),
        ])),
        ListItem::new(Line::from(vec![
            Span::styled("Throughput: ", Style::default().fg(Color::Magenta)),
            Span::raw(format!("{:.1} pps", summary.packets_sent as f64 / summary.uptime.as_secs_f64())),
            Span::styled("  │  Uptime: ", Style::default().fg(Color::Blue)),
            Span::raw(uptime_str),
        ])),
        ListItem::new(Line::from(vec![
            Span::styled("Active Nodes: ", Style::default().fg(Color::Green)),
            Span::raw(format!("{}", summary.active_nodes)),
            Span::styled("  │  Active Channels: ", Style::default().fg(Color::Green)),
            Span::raw(format!("{}", summary.active_channels)),
        ])),
    ];
    
    let stats_list = List::new(stats_items)
        .block(Block::default().borders(Borders::ALL).title("Statistics"));
    f.render_widget(stats_list, chunks[0]);

    // Graphs section
    let graph_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(50),
            Constraint::Percentage(50),
        ])
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

    // Packets dropped graph
    render_chart(
        f,
        top_graph_chunks[1],
        "Packets Dropped",
        &state.packets_dropped_history,
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

/// Render the logs tab content
fn render_logs_tab(f: &mut Frame, area: Rect, state: &TuiState) {
    let logs = state.log_buffer.lock().unwrap();
    let log_count = logs.len();
    
    // Convert logs to ListItems with color-coded levels
    let log_items: Vec<ListItem> = logs
        .iter()
        .map(|line| {
            // Try to detect log level and colorize accordingly
            let styled_line = if line.contains("ERROR") {
                Line::from(Span::styled(line.clone(), Style::default().fg(Color::Red)))
            } else if line.contains("WARN") {
                Line::from(Span::styled(line.clone(), Style::default().fg(Color::Yellow)))
            } else if line.contains("INFO") {
                Line::from(Span::styled(line.clone(), Style::default().fg(Color::Green)))
            } else if line.contains("DEBUG") {
                Line::from(Span::styled(line.clone(), Style::default().fg(Color::Cyan)))
            } else if line.contains("TRACE") {
                Line::from(Span::styled(line.clone(), Style::default().fg(Color::Gray)))
            } else {
                Line::from(line.clone())
            };
            
            ListItem::new(styled_line)
        })
        .collect();

    let title = format!("Logs ({} lines)", log_count);
    let logs_list = List::new(log_items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .style(Style::default().fg(Color::White));
    
    f.render_widget(logs_list, area);
}

/// Render a single chart with historical data
fn render_chart(
    f: &mut Frame,
    area: Rect,
    title: &str,
    data: &[(f64, f64)],
    color: Color,
) {
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
    let min_y = data.iter().map(|(_, y)| *y).fold(f64::INFINITY, f64::min).min(0.0);
    let max_y = data.iter().map(|(_, y)| *y).fold(f64::NEG_INFINITY, f64::max).max(1.0);
    
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

/// Run the TUI dashboard
///
/// This function takes over the terminal and displays a real-time dashboard
/// until the user presses 'q', 'Q', Esc, or Ctrl+C to quit.
pub async fn run_tui(metrics: Arc<SimulatorMetrics>, log_buffer: Arc<Mutex<VecDeque<String>>>) -> Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app state
    let mut state = TuiState::new(metrics, log_buffer);

    // Set up a panic hook to ensure terminal is restored even on panic
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        original_hook(panic_info);
    }));

    // Run the TUI loop
    let res = run_tui_loop(&mut terminal, &mut state).await;

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
) -> Result<()> {
    let mut update_interval = interval(Duration::from_millis(1000)); // Update every second

    loop {
        // Draw UI
        terminal.draw(|f| ui(f, state))?;

        // Handle events with timeout - use a shorter timeout for better responsiveness
        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                // Handle both Press and Repeat events (some terminals only send one)
                if key.kind == KeyEventKind::Press || key.kind == KeyEventKind::Repeat {
                    match key.code {
                            KeyCode::Char('q') | KeyCode::Char('Q') => {
                                tracing::info!("Quit command received");
                                return Ok(());
                            }
                            KeyCode::Esc => {
                                tracing::info!("Escape key received");
                                return Ok(());
                            }
                            KeyCode::Char('c') if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
                                tracing::info!("Ctrl+C received");
                                return Ok(());
                            }
                            KeyCode::Tab | KeyCode::BackTab => {
                                // Switch tabs
                                state.active_tab = match state.active_tab {
                                    Tab::Metrics => Tab::Logs,
                                    Tab::Logs => Tab::Metrics,
                                };
                            }
                            KeyCode::Char('1') => {
                                state.active_tab = Tab::Metrics;
                            }
                            KeyCode::Char('2') => {
                                state.active_tab = Tab::Logs;
                            }
                            KeyCode::Char('r') | KeyCode::Char('R') => {
                                state.metrics.reset();
                                state.packets_sent_history.clear();
                                state.packets_dropped_history.clear();
                                state.throughput_history.clear();
                                state.latency_history.clear();
                                state.prev_packets_sent = 0;
                                state.prev_packets_dropped = 0;
                                state.prev_timestamp = 0.0;
                                state.start_time = Instant::now();
                            }
                            KeyCode::Up => {
                                // Scroll up in logs
                                if state.log_scroll > 0 {
                                    state.log_scroll -= 1;
                                }
                            }
                            KeyCode::Down => {
                                // Scroll down in logs
                                state.log_scroll += 1;
                            }
                            KeyCode::PageUp => {
                                // Scroll up by 10 lines
                                state.log_scroll = state.log_scroll.saturating_sub(10);
                            }
                            KeyCode::PageDown => {
                                // Scroll down by 10 lines
                                state.log_scroll = state.log_scroll.saturating_add(10);
                            }
                            KeyCode::Home => {
                                // Go to top
                                state.log_scroll = 0;
                            }
                        _ => {}
                    }
                }
            }
        }

        // Update metrics periodically
        if update_interval.tick().await.elapsed() > Duration::ZERO {
            state.update();
        }
    }
}
