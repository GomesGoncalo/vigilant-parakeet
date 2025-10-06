// Logs tab rendering - filtered log viewer
use crate::tui::{logging::LogFilter, state::TuiState};
use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState},
    Frame,
};

/// Render the logs tab content with filtering and scrolling
pub fn render_logs_tab(f: &mut Frame, area: Rect, state: &mut TuiState) {
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
