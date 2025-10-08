// Tab-specific rendering modules
mod channels;
mod logs;
mod metrics;
mod registry;
mod topology;
mod upstreams;

use crate::tui::state::TuiState;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::Span,
    Frame,
};

/// Top-level tabs in the TUI
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tab {
    Metrics,
    Channels,
    Upstreams,
    Logs,
    Topology,
}

// Re-export tab renderer structs and Tab enum
pub use channels::ChannelsTab;
pub use logs::LogsTab;
pub use metrics::MetricsTab;
pub use registry::TabRegistry;
pub use topology::TopologyTab;
pub use upstreams::UpstreamsTab;

/// Trait for tab rendering with focused state
///
/// Each tab receives only the state it needs via a dedicated state struct,
/// following the Interface Segregation Principle. This makes dependencies
/// explicit and reduces coupling between tabs and the global TUI state.
pub trait TabRenderer {
    /// The focused state type this tab requires
    type State<'a>;

    /// Get the display name for this tab (with emoji)
    fn display_name() -> &'static str;

    /// Extract focused state from TuiState for rendering (mutable access)
    fn extract_state<'a>(tui_state: &'a mut TuiState) -> Self::State<'a>;

    /// Extract focused state from TuiState for help text (immutable access)
    fn extract_help_state<'a>(tui_state: &'a TuiState) -> Self::State<'a>;

    /// Render the tab content to the given area using focused state
    fn render(f: &mut Frame, area: Rect, state: Self::State<'_>);

    /// Generate context-sensitive help text for this tab
    ///
    /// Note: Help text generation may need access to some state fields
    /// for dynamic help (e.g., showing current sort mode). This uses
    /// the same focused state type.
    fn help_text(state: Self::State<'_>) -> Vec<Span<'static>>;
}

/// Renders the content for the currently selected tab.
///
/// This function uses the TabRegistry to delegate to the appropriate tab renderer.
/// Each tab renderer receives only the state it needs via focused state extraction.
pub fn render_tab_content(f: &mut Frame, area: Rect, state: &mut TuiState) {
    let registry = TabRegistry::global();
    registry.render(state.active_tab, f, area, state);
}

/// Generate context-sensitive help text for the given tab
///
/// This function uses the TabRegistry to generate help text for the specified tab.
pub fn generate_help_text(tab: Tab, state: &TuiState) -> Vec<Span<'static>> {
    let registry = TabRegistry::global();
    registry.help_text(tab, state)
}

/// Helper to create a styled key span
pub(super) fn key_span(text: &str) -> Span<'static> {
    Span::styled(
        text.to_string(),
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )
}

/// Helper to create a styled text span
pub(super) fn text_span(text: &str) -> Span<'static> {
    Span::styled(text.to_string(), Style::default().fg(Color::Gray))
}
