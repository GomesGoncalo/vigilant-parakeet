//! Main UI rendering logic for the TUI

use crate::tui::state::{Tab, TuiState};
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Tabs},
    Frame,
};

/// Render the main TUI dashboard
pub fn render_ui(f: &mut Frame, state: &mut TuiState) {
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

    render_title(f, chunks[0], state.paused);
    render_tabs(f, chunks[1], state.active_tab);

    // Render content based on active tab
    // TODO: Extract to tabs module
    render_tab_content(f, chunks[2], state);

    render_help(f, chunks[3], state);
}

/// Render the title bar
fn render_title(f: &mut Frame, area: ratatui::layout::Rect, paused: bool) {
    let paused_label = if paused { " (PAUSED)" } else { "" };
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
    f.render_widget(title, area);
}

/// Render the tab bar
fn render_tabs(f: &mut Frame, area: ratatui::layout::Rect, active_tab: Tab) {
    let tab_titles = vec![
        "📊 Metrics",
        "📡 Channels",
        "🔼 Upstreams",
        "📜 Logs",
        "🌳 Topology",
    ];
    let tabs = Tabs::new(tab_titles)
        .block(Block::default().borders(Borders::ALL).title("View"))
        .select(match active_tab {
            Tab::Metrics => 0,
            Tab::Channels => 1,
            Tab::Upstreams => 2,
            Tab::Logs => 3,
            Tab::Topology => 4,
        })
        .style(Style::default().fg(Color::White))
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
    f.render_widget(tabs, area);
}

/// Render tab content - delegates to tab-specific modules
fn render_tab_content(f: &mut Frame, area: ratatui::layout::Rect, state: &mut TuiState) {
    use crate::tui::tabs;

    match state.active_tab {
        Tab::Metrics => tabs::render_metrics_tab(f, area, state),
        Tab::Channels => tabs::render_channels_tab(f, area, state),
        Tab::Upstreams => tabs::render_upstreams_tab(f, area, state),
        Tab::Logs => tabs::render_logs_tab(f, area, state),
        Tab::Topology => tabs::render_topology_tab(f, area, state),
    }
}

/// Render context-sensitive help text at the bottom
fn render_help(f: &mut Frame, area: ratatui::layout::Rect, state: &TuiState) {
    let sort_mode_text = format!(" sort: {}  │  ", state.channel_sort_mode.as_str());
    let sort_dir_text = format!(" dir: {}  │  ", state.channel_sort_direction.as_str());
    let auto_scroll_text = format!(
        " auto-scroll: {}  │  ",
        if state.log_auto_scroll { "ON" } else { "OFF" }
    );
    let filter_text = format!(" filter: {}  │  ", state.log_filter.as_str());

    let help_spans = match state.active_tab {
        Tab::Metrics => vec![
            key_span("Q/Esc/Ctrl+C"),
            text_span(" quit  │  "),
            key_span("P"),
            text_span(" pause  │  "),
            key_span("R"),
            text_span(" reset  │  "),
            key_span("Tab/1/2/3/4/5"),
            text_span(" switch tabs"),
        ],
        Tab::Channels => vec![
            key_span("Q/Esc/Ctrl+C"),
            text_span(" quit  │  "),
            key_span("P"),
            text_span(" pause  │  "),
            key_span("S"),
            Span::styled(sort_mode_text, Style::default().fg(Color::Gray)),
            key_span("D"),
            Span::styled(sort_dir_text, Style::default().fg(Color::Gray)),
            key_span("Tab/1/2/3/4/5"),
            text_span(" switch tabs"),
        ],
        Tab::Upstreams => vec![
            key_span("Q/Esc/Ctrl+C"),
            text_span(" quit  │  "),
            key_span("P"),
            text_span(" pause  │  "),
            key_span("Tab/1/2/3/4/5"),
            text_span(" switch tabs"),
        ],
        Tab::Logs => {
            if state.log_input_mode {
                vec![
                    key_span("Enter"),
                    text_span(" apply filter  │  "),
                    key_span("Esc"),
                    text_span(" cancel  │  "),
                    key_span("Backspace"),
                    text_span(" delete char  │  "),
                    Span::styled(
                        "Type to filter (e.g., 'obu1', 'ERROR', etc.)",
                        Style::default().fg(Color::Cyan),
                    ),
                ]
            } else {
                vec![
                    key_span("Q/Esc"),
                    text_span(" quit  │  "),
                    key_span("F"),
                    Span::styled(filter_text, Style::default().fg(Color::Gray)),
                    key_span("/"),
                    text_span(" custom filter  │  "),
                    key_span("↑/↓"),
                    text_span(" scroll  │  "),
                    key_span("End"),
                    Span::styled(auto_scroll_text, Style::default().fg(Color::Gray)),
                    key_span("Tab"),
                    text_span(" switch"),
                ]
            }
        }
        Tab::Topology => vec![
            key_span("Q/Esc/Ctrl+C"),
            text_span(" quit  │  "),
            key_span("P"),
            text_span(" pause  │  "),
            key_span("↑/↓/PgUp/PgDn"),
            text_span(" navigate  │  "),
            key_span("Tab/1/2/3/4/5"),
            text_span(" switch tabs"),
        ],
    };

    let help = Paragraph::new(Line::from(help_spans))
        .block(Block::default().borders(Borders::ALL).title("Controls"));
    f.render_widget(help, area);
}

/// Helper to create a styled key span
fn key_span(text: &str) -> Span<'static> {
    Span::styled(
        text.to_string(),
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )
}

/// Helper to create a styled text span
fn text_span(text: &str) -> Span<'static> {
    Span::styled(text.to_string(), Style::default().fg(Color::Gray))
}
