// Channels tab rendering
use crate::{
    metrics::{ChannelStats, SimulatorMetrics},
    tui::{
        state::{ChannelSortMode, DisplayChannelStats, SortDirection, TuiState},
        utils::format_bits_per_sec,
    },
};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::Span,
    widgets::{Block, Borders, Cell, Row, Table},
    Frame,
};
use std::{collections::HashMap, sync::Arc};

use super::TabRenderer;

/// State data for the Channels tab
pub struct ChannelsTabState<'a> {
    pub metrics: &'a Arc<SimulatorMetrics>,
    pub channel_sort_mode: ChannelSortMode,
    pub channel_sort_direction: SortDirection,
    #[allow(dead_code)]
    pub paused: bool,
    pub paused_channel_display: &'a Option<HashMap<String, DisplayChannelStats>>,
}

/// Channels tab renderer
pub struct ChannelsTab;

impl TabRenderer for ChannelsTab {
    type State<'a> = ChannelsTabState<'a>;

    fn display_name() -> &'static str {
        "📡 Channels"
    }

    fn extract_state<'a>(tui_state: &'a mut TuiState) -> Self::State<'a> {
        ChannelsTabState {
            metrics: &tui_state.metrics,
            channel_sort_mode: tui_state.channel_sort_mode,
            channel_sort_direction: tui_state.channel_sort_direction,
            paused: tui_state.paused,
            paused_channel_display: &tui_state.paused_channel_display,
        }
    }

    fn extract_help_state<'a>(tui_state: &'a TuiState) -> Self::State<'a> {
        ChannelsTabState {
            metrics: &tui_state.metrics,
            channel_sort_mode: tui_state.channel_sort_mode,
            channel_sort_direction: tui_state.channel_sort_direction,
            paused: tui_state.paused,
            paused_channel_display: &tui_state.paused_channel_display,
        }
    }

    fn render(f: &mut Frame, area: Rect, state: Self::State<'_>) {
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
            Cell::from(latency_label).style(
                if state.channel_sort_mode == ChannelSortMode::Latency {
                    header_highlight
                } else {
                    header_normal
                },
            ),
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
                channel_data
                    .sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal));
                if state.channel_sort_direction == SortDirection::Desc {
                    channel_data.reverse();
                }
            }
            ChannelSortMode::Throughput => {
                channel_data
                    .sort_by(|a, b| a.3.partial_cmp(&b.3).unwrap_or(std::cmp::Ordering::Equal));
                if state.channel_sort_direction == SortDirection::Desc {
                    channel_data.reverse();
                }
            }
            ChannelSortMode::Latency => {
                channel_data
                    .sort_by(|a, b| a.4.partial_cmp(&b.4).unwrap_or(std::cmp::Ordering::Equal));
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
                            ChannelSortMode::Name => {
                                cell_channel = cell_channel.style(data_highlight)
                            }
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
            ratatui::layout::Constraint::Percentage(40), // Channel
            ratatui::layout::Constraint::Percentage(20), // Loss %
            ratatui::layout::Constraint::Percentage(20), // Throughput
            ratatui::layout::Constraint::Percentage(20), // Avg Latency
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

    fn help_text(state: Self::State<'_>) -> Vec<Span<'static>> {
        let sort_mode_text = format!(" sort: {}  │  ", state.channel_sort_mode.as_str());
        let sort_dir_text = format!(" dir: {}  │  ", state.channel_sort_direction.as_str());
        vec![
            super::key_span("Q/Esc/Ctrl+C"),
            super::text_span(" quit  │  "),
            super::key_span("P"),
            super::text_span(" pause  │  "),
            super::key_span("S"),
            Span::styled(sort_mode_text, Style::default().fg(Color::Gray)),
            super::key_span("D"),
            Span::styled(sort_dir_text, Style::default().fg(Color::Gray)),
            super::key_span("Tab/1/2/3/4/5"),
            super::text_span(" switch tabs"),
        ]
    }
}
