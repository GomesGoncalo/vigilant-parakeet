// Logs tab rendering - filtered log viewer
use crate::tui::{logging::LogFilter, state::TuiState};
use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState},
    Frame,
};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use super::TabRenderer;

/// State data for the Logs tab
pub struct LogsTabState<'a> {
    pub log_buffer: &'a Arc<Mutex<VecDeque<String>>>,
    pub log_scroll: &'a mut usize,
    pub log_auto_scroll: bool,
    pub log_filter: &'a LogFilter,
    pub log_input_mode: bool,
    pub log_input_buffer: &'a str,
}

/// Logs tab renderer
#[derive(Default)]
pub struct LogsTab;

impl TabRenderer for LogsTab {
    type State<'a> = LogsTabState<'a>;

    fn display_name() -> &'static str {
        "📜 Logs"
    }

    fn extract_state<'a>(tui_state: &'a mut TuiState) -> Self::State<'a> {
        LogsTabState {
            log_buffer: &tui_state.log_buffer,
            log_scroll: &mut tui_state.log_scroll,
            log_auto_scroll: tui_state.log_auto_scroll,
            log_filter: &tui_state.log_filter,
            log_input_mode: tui_state.log_input_mode,
            log_input_buffer: &tui_state.log_input_buffer,
        }
    }

    fn extract_help_state<'a>(tui_state: &'a TuiState) -> Self::State<'a> {
        // Help text doesn't mutate log_scroll, so we can use a dummy reference
        // This is a bit awkward but necessary since help_text doesn't need mutation
        static mut DUMMY_SCROLL: usize = 0;
        #[allow(static_mut_refs)]
        LogsTabState {
            log_buffer: &tui_state.log_buffer,
            log_scroll: unsafe { &mut DUMMY_SCROLL },
            log_auto_scroll: tui_state.log_auto_scroll,
            log_filter: &tui_state.log_filter,
            log_input_mode: tui_state.log_input_mode,
            log_input_buffer: &tui_state.log_input_buffer,
        }
    }

    fn render(f: &mut Frame, area: Rect, state: Self::State<'_>) {
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
        if *state.log_scroll >= log_count && log_count > 0 {
            *state.log_scroll = log_count - 1;
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
        list_state.select(Some(*state.log_scroll));

        f.render_stateful_widget(logs_list, area, &mut list_state);
    }

    fn help_text(state: Self::State<'_>) -> Vec<Span<'static>> {
        if state.log_input_mode {
            vec![
                super::key_span("Enter"),
                super::text_span(" apply filter  │  "),
                super::key_span("Esc"),
                super::text_span(" cancel  │  "),
                super::key_span("Backspace"),
                super::text_span(" delete char  │  "),
                Span::styled(
                    "Type to filter (e.g., 'obu1', 'ERROR', etc.)",
                    Style::default().fg(Color::Cyan),
                ),
            ]
        } else {
            let filter_text = format!(" filter: {}  │  ", state.log_filter.as_str());
            let auto_scroll_text = format!(
                " auto-scroll: {}  │  ",
                if state.log_auto_scroll { "ON" } else { "OFF" }
            );
            vec![
                super::key_span("Q/Esc"),
                super::text_span(" quit  │  "),
                super::key_span("F"),
                Span::styled(filter_text, Style::default().fg(Color::Gray)),
                super::key_span("/"),
                super::text_span(" custom filter  │  "),
                super::key_span("↑/↓"),
                super::text_span(" scroll  │  "),
                super::key_span("End"),
                Span::styled(auto_scroll_text, Style::default().fg(Color::Gray)),
                super::key_span("Tab"),
                super::text_span(" switch"),
            ]
        }
    }
}
