// Metrics tab rendering
use crate::{
    metrics::SimulatorMetrics,
    tui::{
        state::TuiState,
        utils::{format_bits_per_sec, render_chart},
    },
};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    symbols,
    text::{Line, Span},
    widgets::{Axis, Block, Borders, Chart, Dataset, GraphType, List, ListItem, Paragraph},
    Frame,
};
use std::sync::Arc;

use super::TabRenderer;

/// State data for the Metrics tab
pub struct MetricsTabState<'a> {
    pub metrics: &'a Arc<SimulatorMetrics>,
    pub paused: bool,
    pub paused_summary: &'a Option<crate::metrics::MetricsSummary>,
    // Historical data for graphs
    pub loss_percentage_history: &'a [(f64, f64)],
    pub throughput_bps_history: &'a [(f64, f64)],
    pub throughput_history: &'a [(f64, f64)],
    pub latency_history: &'a [(f64, f64)],
    pub p95_history: &'a [(f64, f64)],
    pub p99_history: &'a [(f64, f64)],
    pub jitter_history: &'a [(f64, f64)],
    pub cpu_history: &'a [(f64, f64)],
    pub mem_history: &'a [(f64, f64)],
}

/// Metrics tab renderer
pub struct MetricsTab;

impl TabRenderer for MetricsTab {
    type State<'a> = MetricsTabState<'a>;

    fn display_name() -> &'static str {
        "📊 Metrics"
    }

    fn extract_state<'a>(tui_state: &'a mut TuiState) -> Self::State<'a> {
        MetricsTabState {
            metrics: &tui_state.metrics,
            paused: tui_state.paused,
            paused_summary: &tui_state.paused_summary,
            loss_percentage_history: &tui_state.loss_percentage_history,
            throughput_bps_history: &tui_state.throughput_bps_history,
            throughput_history: &tui_state.throughput_history,
            latency_history: &tui_state.latency_history,
            p95_history: &tui_state.p95_history,
            p99_history: &tui_state.p99_history,
            jitter_history: &tui_state.jitter_history,
            cpu_history: &tui_state.cpu_history,
            mem_history: &tui_state.mem_history,
        }
    }

    fn extract_help_state<'a>(tui_state: &'a TuiState) -> Self::State<'a> {
        MetricsTabState {
            metrics: &tui_state.metrics,
            paused: tui_state.paused,
            paused_summary: &tui_state.paused_summary,
            loss_percentage_history: &tui_state.loss_percentage_history,
            throughput_bps_history: &tui_state.throughput_bps_history,
            throughput_history: &tui_state.throughput_history,
            latency_history: &tui_state.latency_history,
            p95_history: &tui_state.p95_history,
            p99_history: &tui_state.p99_history,
            jitter_history: &tui_state.jitter_history,
            cpu_history: &tui_state.cpu_history,
            mem_history: &tui_state.mem_history,
        }
    }

    fn render(f: &mut Frame, area: Rect, state: Self::State<'_>) {
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
                Constraint::Length(7), // Stats summary (extra row for CPU/mem)
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

        // Use pre-computed history values (computed once per tick in TuiState::update)
        let p95_ms = state.p95_history.last().map(|&(_, v)| v).unwrap_or(0.0);
        let p99_ms = state.p99_history.last().map(|&(_, v)| v).unwrap_or(0.0);
        let jitter_ms = state.jitter_history.last().map(|&(_, v)| v).unwrap_or(0.0);
        // throughput_bps_history stores bits/sec; display as bytes/sec
        let throughput_bps = state
            .throughput_bps_history
            .last()
            .map(|&(_, v)| v / 8.0)
            .unwrap_or(0.0);

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
                Span::raw(format!("{:.1} B/s", throughput_bps)),
                Span::styled("  │  p95/p99: ", Style::default().fg(Color::Yellow)),
                Span::raw(format!("{:.2}ms/{:.2}ms", p95_ms, p99_ms)),
                Span::styled("  │  Jitter: ", Style::default().fg(Color::LightBlue)),
                Span::raw(format!("{:.2}ms", jitter_ms)),
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

        // Current CPU and memory from the last sample
        let (current_cpu, current_mem_mb) = state
            .cpu_history
            .last()
            .and_then(|&(_, c)| state.mem_history.last().map(|&(_, m)| (c, m)))
            .unwrap_or((0.0, 0.0));

        let cpu_color = if current_cpu > 80.0 {
            Color::Red
        } else if current_cpu > 50.0 {
            Color::Yellow
        } else {
            Color::Green
        };

        let mut stats_items = stats_items;
        stats_items.push(ListItem::new(Line::from(vec![
            Span::styled("CPU: ", Style::default().fg(cpu_color)),
            Span::styled(
                format!("{:.1}%", current_cpu),
                Style::default().fg(cpu_color).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  │  Memory: ", Style::default().fg(Color::Cyan)),
            Span::raw(format!("{:.1} MiB", current_mem_mb)),
        ])));

        let stats_list = List::new(stats_items)
            .block(Block::default().borders(Borders::ALL).title("Statistics"));
        f.render_widget(stats_list, chunks[0]);

        // Graphs section
        let graph_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(34),
                Constraint::Percentage(33),
                Constraint::Percentage(33),
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

        let resource_graph_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(graph_chunks[2]);

        // Packet loss percentage graph (top-left)
        render_chart(
            f,
            top_graph_chunks[0],
            "Packet Loss (%)",
            state.loss_percentage_history,
            Color::Red,
        );

        // Throughput (bps) line chart (top-right)
        {
            let sub = top_graph_chunks[1];

            // Build dataset from the time-series (elapsed_seconds, bps)
            let data: Vec<(f64, f64)> = state
                .throughput_bps_history
                .iter()
                .map(|(t, v)| (*t, *v))
                .collect();

            if data.is_empty() {
                let empty = Paragraph::new("No throughput data yet...")
                    .block(Block::default().borders(Borders::ALL).title("Throughput"))
                    .style(Style::default().fg(Color::Gray));
                f.render_widget(empty, sub);
            } else {
                let min_x = data.first().map(|(x, _)| *x).unwrap_or(0.0);
                let max_x = data.last().map(|(x, _)| *x).unwrap_or(min_x + 60.0);
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
                let y_padding = (max_y - min_y) * 0.1;
                let chart_min_y = (min_y - y_padding).max(0.0);
                let chart_max_y = max_y + y_padding;

                let last_bps = data.last().map(|(_, v)| *v).unwrap_or(0.0);
                let title = format!("Throughput — {}", format_bits_per_sec(last_bps));

                let dataset = vec![Dataset::default()
                    .name("Throughput")
                    .marker(symbols::Marker::Braille)
                    .graph_type(GraphType::Line)
                    .style(Style::default().fg(Color::Magenta))
                    .data(data.as_slice())];

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
                                Span::raw(format_bits_per_sec(chart_min_y)),
                                Span::raw(format_bits_per_sec(chart_max_y)),
                            ]),
                    );

                f.render_widget(chart, sub);
            }
        }

        // Throughput graph (bottom-left)
        render_chart(
            f,
            bottom_graph_chunks[0],
            "Throughput (pps)",
            state.throughput_history,
            Color::Magenta,
        );

        // Latency graph (bottom-right)
        render_chart(
            f,
            bottom_graph_chunks[1],
            "Avg Latency (ms)",
            state.latency_history,
            Color::Cyan,
        );

        // CPU usage chart (resource-left)
        render_chart(
            f,
            resource_graph_chunks[0],
            "CPU Usage (%)",
            state.cpu_history,
            Color::Yellow,
        );

        // Memory usage chart (resource-right)
        render_chart(
            f,
            resource_graph_chunks[1],
            "Memory (MiB)",
            state.mem_history,
            Color::LightBlue,
        );
    }

    fn help_text(_state: Self::State<'_>) -> Vec<Span<'static>> {
        vec![
            super::key_span("Q/Esc/Ctrl+C"),
            super::text_span(" quit  │  "),
            super::key_span("P"),
            super::text_span(" pause  │  "),
            super::key_span("R"),
            super::text_span(" reset  │  "),
            super::key_span("Tab/1/2/3/4/5"),
            super::text_span(" switch tabs"),
        ]
    }
}
