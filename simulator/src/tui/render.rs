//! Main UI rendering logic for the TUI

use crate::tui::{state::TuiState, tabs::Tab};
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

    // Render content based on active tab (delegated to tabs module)
    crate::tui::tabs::render_tab_content(f, chunks[2], state);

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
    // Get tab titles from the TabRegistry (single source of truth via O(1) lookup)
    use crate::tui::tabs::TabRegistry;
    let registry = TabRegistry::global();
    let tab_titles = registry.all_tab_titles();

    let tabs = Tabs::new(tab_titles)
        .block(Block::default().borders(Borders::ALL).title("View"))
        .select(registry.position(active_tab))
        .style(Style::default().fg(Color::White))
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
    f.render_widget(tabs, area);
}

// Tab-specific rendering is implemented in `tui::tabs` module (see `tabs::render_tab_content`).

/// Render context-sensitive help text at the bottom
fn render_help(f: &mut Frame, area: ratatui::layout::Rect, state: &TuiState) {
    let help_spans = crate::tui::tabs::generate_help_text(state.active_tab, state);
    let help = Paragraph::new(Line::from(help_spans))
        .block(Block::default().borders(Borders::ALL).title("Controls"));
    f.render_widget(help, area);
}
