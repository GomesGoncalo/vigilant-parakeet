// Metrics tab rendering
use crate::tui::{
    state::TuiState,
    utils::{format_bits_per_sec, render_chart},
};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    symbols,
    text::{Line, Span},
    widgets::{Axis, Block, Borders, Chart, Dataset, GraphType, List, ListItem, Paragraph},
    Frame,
};

/// Render the metrics tab content
pub fn render_metrics_tab(f: &mut Frame, area: Rect, state: &TuiState) {
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

    // Compute overall p95/p99 latency from channel samples and total throughput bytes/sec
    let channel_map = state.metrics.channel_stats();
    let mut all_samples: Vec<u64> = Vec::new();
    let mut total_bytes_last10: u64 = 0;
    for (_k, stats) in channel_map.iter() {
        for &s in stats.latency_samples.iter() {
            all_samples.push(s);
        }
        // Sum throughput window bytes
        total_bytes_last10 = total_bytes_last10
            .saturating_add(stats.throughput_window.iter().map(|(_, b)| *b).sum::<u64>());
    }
    all_samples.sort_unstable();
    let p95_ms = if !all_samples.is_empty() {
        let idx = ((all_samples.len() as f64) * 0.95).ceil() as usize - 1;
        (all_samples[idx] as f64) / 1000.0
    } else {
        0.0
    };
    let p99_ms = if !all_samples.is_empty() {
        let idx = ((all_samples.len() as f64) * 0.99).ceil() as usize - 1;
        (all_samples[idx] as f64) / 1000.0
    } else {
        0.0
    };

    // Compute jitter (stddev) in ms from samples
    let jitter_ms = if !all_samples.is_empty() {
        let mean_us =
            all_samples.iter().map(|&v| v as f64).sum::<f64>() / (all_samples.len() as f64);
        let var = all_samples
            .iter()
            .map(|&v| {
                let d = (v as f64) - mean_us;
                d * d
            })
            .sum::<f64>()
            / (all_samples.len() as f64);
        (var.sqrt()) / 1000.0
    } else {
        0.0
    };

    // Approximate throughput bytes/sec by summing bytes in last 10s windows
    let throughput_bps = if total_bytes_last10 > 0 {
        total_bytes_last10 as f64 / 10.0
    } else {
        0.0
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

    // Packet loss percentage graph (top-left)
    render_chart(
        f,
        top_graph_chunks[0],
        "Packet Loss (%)",
        &state.loss_percentage_history,
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
        &state.throughput_history,
        Color::Magenta,
    );

    // Latency graph (bottom-right)
    render_chart(
        f,
        bottom_graph_chunks[1],
        "Avg Latency (ms)",
        &state.latency_history,
        Color::Cyan,
    );
}
