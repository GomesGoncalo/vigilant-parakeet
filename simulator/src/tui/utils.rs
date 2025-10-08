// Utility functions for TUI rendering
use human_format::Formatter;
use ratatui::{
    layout::Rect,
    style::{Color, Style},
    symbols,
    text::Span,
    widgets::{Axis, Block, Borders, Chart, Dataset, GraphType, Paragraph},
    Frame,
};

/// Render a time-series line chart with standard formatting
pub fn render_chart(f: &mut Frame, area: Rect, title: &str, data: &[(f64, f64)], color: Color) {
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
pub fn format_bits_per_sec(bps: f64) -> String {
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
