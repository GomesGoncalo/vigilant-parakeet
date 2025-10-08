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
    pub log_horizontal_scroll: &'a mut usize,
    pub log_wrap: bool,
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
            log_horizontal_scroll: &mut tui_state.log_horizontal_scroll,
            log_wrap: tui_state.log_wrap,
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
        static mut DUMMY_H_SCROLL: usize = 0;
        #[allow(static_mut_refs)]
        LogsTabState {
            log_buffer: &tui_state.log_buffer,
            log_scroll: unsafe { &mut DUMMY_SCROLL },
            log_horizontal_scroll: unsafe { &mut DUMMY_H_SCROLL },
            log_wrap: tui_state.log_wrap,
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

        // Convert logs to ListItems with color-coded levels and horizontal scrolling
        let log_items: Vec<ListItem> = filtered_logs
            .iter()
            .map(|line| {
                // Apply horizontal scroll offset
                let scrolled_line = if *state.log_horizontal_scroll > 0
                    && line.len() > *state.log_horizontal_scroll
                {
                    &line[*state.log_horizontal_scroll..]
                } else if *state.log_horizontal_scroll > 0 {
                    "" // Line is too short, show empty
                } else {
                    line.as_str()
                };

                // Try to detect log level and colorize accordingly
                let styled_line = if line.contains("ERROR") {
                    Line::from(Span::styled(
                        scrolled_line.to_string(),
                        Style::default().fg(Color::Red),
                    ))
                } else if line.contains("WARN") {
                    Line::from(Span::styled(
                        scrolled_line.to_string(),
                        Style::default().fg(Color::Yellow),
                    ))
                } else if line.contains("INFO") {
                    Line::from(Span::styled(
                        scrolled_line.to_string(),
                        Style::default().fg(Color::Green),
                    ))
                } else if line.contains("DEBUG") {
                    Line::from(Span::styled(
                        scrolled_line.to_string(),
                        Style::default().fg(Color::Cyan),
                    ))
                } else if line.contains("TRACE") {
                    Line::from(Span::styled(
                        scrolled_line.to_string(),
                        Style::default().fg(Color::Gray),
                    ))
                } else {
                    Line::from(scrolled_line.to_string())
                };

                ListItem::new(styled_line)
            })
            .collect();

        let auto_scroll_indicator = if state.log_auto_scroll { "🔄 " } else { "" };
        let filter_text = state.log_filter.as_str();
        let h_scroll_indicator = if *state.log_horizontal_scroll > 0 {
            format!(" ←→{} ", state.log_horizontal_scroll)
        } else {
            String::new()
        };
        let input_indicator = if state.log_input_mode {
            format!(" [INPUT: {}█]", state.log_input_buffer)
        } else {
            String::new()
        };
        // paragraph/wrap not needed after switching to per-line wrapping

        // Show explicit wrap indicator in title
        let wrap_indicator = format!(" wrap: {}", if state.log_wrap { "ON" } else { "OFF" });

        let title = format!(
            "{}Logs ({} lines, filter: {}){}{}{}",
            auto_scroll_indicator,
            log_count,
            filter_text,
            h_scroll_indicator,
            input_indicator,
            wrap_indicator
        );

        if state.log_wrap {
            // Render wrapped logs while preserving color and allow vertical scrolling.
            // We'll flatten each log into multiple wrapped sub-lines (taking
            // horizontal scroll into account) and render them as a List so we
            // can reuse the same scrolling/select logic.
            let inner_width = area.width.saturating_sub(2) as usize; // account for borders
            let wrap_width = if inner_width == 0 { 1 } else { inner_width };

            let mut wrapped_items: Vec<ListItem> = Vec::new();

            for line in filtered_logs.iter() {
                // Apply horizontal scroll offset
                let scrolled_line = if *state.log_horizontal_scroll > 0
                    && line.len() > *state.log_horizontal_scroll
                {
                    &line[*state.log_horizontal_scroll..]
                } else if *state.log_horizontal_scroll > 0 {
                    "" // Line is too short, show empty
                } else {
                    line.as_str()
                };

                // Determine color/style for the whole logical line
                let style = if line.contains("ERROR") {
                    Style::default().fg(Color::Red)
                } else if line.contains("WARN") {
                    Style::default().fg(Color::Yellow)
                } else if line.contains("INFO") {
                    Style::default().fg(Color::Green)
                } else if line.contains("DEBUG") {
                    Style::default().fg(Color::Cyan)
                } else if line.contains("TRACE") {
                    Style::default().fg(Color::Gray)
                } else {
                    Style::default().fg(Color::White)
                };

                // Grapheme-aware wrapping: iterate over grapheme clusters and
                // measure their display width so we wrap at column boundaries.
                use unicode_segmentation::UnicodeSegmentation;
                use unicode_width::UnicodeWidthStr;

                let mut buf = String::new();
                let mut cur_width = 0usize;
                for g in UnicodeSegmentation::graphemes(scrolled_line, true) {
                    let gw = UnicodeWidthStr::width(g);
                    // If adding this grapheme would exceed the wrap width,
                    // flush buffer first.
                    if cur_width + gw > wrap_width && !buf.is_empty() {
                        let li = ListItem::new(Line::from(Span::styled(buf.clone(), style)));
                        wrapped_items.push(li);
                        buf.clear();
                        cur_width = 0;
                    }
                    buf.push_str(g);
                    cur_width += gw;
                }
                if !buf.is_empty() {
                    let li = ListItem::new(Line::from(Span::styled(buf.clone(), style)));
                    wrapped_items.push(li);
                }
            }

            // Clamp scroll to wrapped items length
            let wrapped_count = wrapped_items.len();
            if *state.log_scroll >= wrapped_count && wrapped_count > 0 {
                *state.log_scroll = wrapped_count - 1;
            }

            let logs_list = List::new(wrapped_items)
                .block(Block::default().borders(Borders::ALL).title(title))
                .style(Style::default().fg(Color::White));

            // Create list state for scrolling into the wrapped list
            let mut list_state = ListState::default();
            if wrapped_count > 0 {
                list_state.select(Some(*state.log_scroll));
            }

            f.render_stateful_widget(logs_list, area, &mut list_state);
        } else {
            let logs_list = List::new(log_items)
                .block(Block::default().borders(Borders::ALL).title(title))
                .style(Style::default().fg(Color::White));

            // Create list state for scrolling
            let mut list_state = ListState::default();
            list_state.select(Some(*state.log_scroll));

            f.render_stateful_widget(logs_list, area, &mut list_state);
        }
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
                super::key_span("←/→"),
                super::text_span(" h-scroll  │  "),
                super::key_span("End"),
                Span::styled(auto_scroll_text, Style::default().fg(Color::Gray)),
                super::key_span("Tab"),
                super::text_span(" switch"),
            ]
        }
    }
}
