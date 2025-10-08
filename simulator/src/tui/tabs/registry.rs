//! Tab registry for polymorphic tab rendering
//!
//! This module provides a registry pattern that eliminates match statements
//! when dispatching to tabs. Instead of matching on Tab enum, we use the
//! registry to look up and invoke the appropriate tab renderer.

use super::{ChannelsTab, LogsTab, MetricsTab, Tab, TabRenderer, TopologyTab, UpstreamsTab};
use crate::tui::state::TuiState;
use ratatui::{layout::Rect, text::Span, Frame};

/// Registry entry that can render a tab and extract its state
///
/// This uses dynamic dispatch internally but provides a type-safe interface
/// through position-based array lookup.
#[derive(Debug)]
struct TabEntry {
    render_fn: fn(&mut Frame, Rect, &mut TuiState),
    help_fn: fn(&TuiState) -> Vec<Span<'static>>,
    display_name_fn: fn() -> &'static str,
}

impl TabEntry {
    /// Create a new tab entry for a specific tab renderer type
    fn new<T>() -> Self
    where
        T: TabRenderer,
    {
        Self {
            render_fn: |f, area, state| {
                let tab_state = T::extract_state(state);
                T::render(f, area, tab_state);
            },
            help_fn: |state| {
                let tab_state = T::extract_help_state(state);
                T::help_text(tab_state)
            },
            display_name_fn: || T::display_name(),
        }
    }

    fn render(&self, f: &mut Frame, area: Rect, state: &mut TuiState) {
        (self.render_fn)(f, area, state)
    }

    fn help_text(&self, state: &TuiState) -> Vec<Span<'static>> {
        (self.help_fn)(state)
    }

    fn display_name(&self) -> &'static str {
        (self.display_name_fn)()
    }
}

/// Global tab registry
///
/// Uses a static array with position-based lookups via HashMap for O(1) access.
/// The array is initialized once at startup and never modified.
///
/// This is the single source of truth for all tab information. The Tab enum
/// variants are decoupled from array positions via the lookup map.
pub struct TabRegistry {
    entries: Vec<TabEntry>,
    lookup: std::collections::HashMap<Tab, usize>,
}

impl TabRegistry {
    /// Create a new tab registry with all tabs registered
    fn new() -> Self {
        let entries = [
            (Tab::Metrics, TabEntry::new::<MetricsTab>()), // Position 0
            (Tab::Channels, TabEntry::new::<ChannelsTab>()), // Position 1
            (Tab::Upstreams, TabEntry::new::<UpstreamsTab>()), // Position 2
            (Tab::Logs, TabEntry::new::<LogsTab>()),       // Position 3
            (Tab::Topology, TabEntry::new::<TopologyTab>()), // Position 4
        ];

        let size = entries.len();
        entries.into_iter().enumerate().fold(Self {
            entries: Vec::with_capacity(size),
            lookup: std::collections::HashMap::with_capacity(size),
        }, |mut acc, (index, (tab, entry))| {
            acc.entries.push(entry);
            acc.lookup.insert(tab, index);
            acc
        })
    }

    /// Get the global tab registry instance
    pub fn global() -> &'static Self {
        static REGISTRY: std::sync::OnceLock<TabRegistry> = std::sync::OnceLock::new();
        REGISTRY.get_or_init(TabRegistry::new)
    }

    /// Render the specified tab
    pub fn render(&self, tab: Tab, f: &mut Frame, area: Rect, state: &mut TuiState) {
        let index = self.lookup[&tab];
        self.entries[index].render(f, area, state);
    }

    /// Generate help text for the specified tab
    pub fn help_text(&self, tab: Tab, state: &TuiState) -> Vec<Span<'static>> {
        let index = self.lookup[&tab];
        self.entries[index].help_text(state)
    }

    /// Get all tab titles in order for rendering tab bar
    pub fn all_tab_titles(&self) -> Vec<&'static str> {
        self.entries
            .iter()
            .map(|entry| entry.display_name())
            .collect()
    }

    /// Get the position index of a tab in the registry
    pub fn position(&self, tab: Tab) -> usize {
        self.lookup[&tab]
    }
}
