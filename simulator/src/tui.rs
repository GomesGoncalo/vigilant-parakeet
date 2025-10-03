//! Terminal User Interface for real-time simulation monitoring
//!
//! Provides an interactive dashboard displaying:
//! - Packet statistics (sent, dropped, delayed)
//! - Performance metrics (drop rate, latency, throughput)
//! - Resource information (active nodes, channels)
//! - Live sparkline graphs showing trends over time

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
        Axis, Block, Borders, Chart, Dataset, GraphType, List, ListItem, Paragraph,
    },
    Frame, Terminal,
};
use std::{
    io,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::time::interval;

/// Maximum number of data points to keep for sparkline graphs
const MAX_HISTORY: usize = 60;

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
}

impl TuiState {
    fn new(metrics: Arc<SimulatorMetrics>) -> Self {
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
    let summary = state.metrics.summary();

    // Create main layout
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(3),  // Title
            Constraint::Length(7),  // Stats summary
            Constraint::Min(10),    // Graphs
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
    f.render_widget(stats_list, chunks[1]);

    // Graphs section
    let graph_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(50),
            Constraint::Percentage(50),
        ])
        .split(chunks[2]);

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

    // Help text
    let help = Paragraph::new(Line::from(vec![
        Span::styled("Press ", Style::default().fg(Color::Gray)),
        Span::styled("'q'", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        Span::styled(" to quit  │  ", Style::default().fg(Color::Gray)),
        Span::styled("'r'", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        Span::styled(" to reset metrics", Style::default().fg(Color::Gray)),
    ]))
    .block(Block::default().borders(Borders::ALL));
    f.render_widget(help, chunks[3]);
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
/// until the user presses 'q' to quit.
pub async fn run_tui(metrics: Arc<SimulatorMetrics>) -> Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app state
    let mut state = TuiState::new(metrics);

    // Run the TUI loop
    let res = run_tui_loop(&mut terminal, &mut state).await;

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

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

        // Handle events with timeout
        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') => return Ok(()),
                        KeyCode::Char('r') => {
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
