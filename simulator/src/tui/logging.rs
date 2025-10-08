//! Logging infrastructure for the TUI
//!
//! Provides a thread-safe log buffer and a custom tracing layer that captures
//! logs for display in the TUI.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
use tracing_subscriber::Layer;

/// Maximum number of log lines to retain in the in-memory buffer
pub const MAX_LOGS: usize = 1000;

/// Log filter mode
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogFilter {
    All,            // Show all logs
    Simulator,      // Show only simulator logs (simulator::, common::)
    Nodes,          // Show only node logs (node_lib::, server_lib::, obu_lib::, rsu_lib::)
    Custom(String), // Show logs containing custom text (e.g., node name)
}

impl LogFilter {
    pub fn next(&self) -> Self {
        match self {
            Self::All => Self::Simulator,
            Self::Simulator => Self::Nodes,
            Self::Nodes => Self::All,
            Self::Custom(_) => Self::All, // Cycling resets custom filter
        }
    }

    pub fn as_str(&self) -> String {
        match self {
            Self::All => "All".to_string(),
            Self::Simulator => "Simulator".to_string(),
            Self::Nodes => "Nodes".to_string(),
            Self::Custom(text) => format!("'{}'", text),
        }
    }

    pub fn matches(&self, target: &str, full_line: &str) -> bool {
        match self {
            Self::All => true,
            Self::Simulator => target.starts_with("simulator") || target.starts_with("common"),
            Self::Nodes => {
                target.starts_with("node_lib")
                    || target.starts_with("server_lib")
                    || target.starts_with("obu_lib")
                    || target.starts_with("rsu_lib")
            }
            Self::Custom(text) => {
                // Search in full log line for custom text (case-insensitive)
                full_line.to_lowercase().contains(&text.to_lowercase())
            }
        }
    }
}

/// Thread-safe log buffer for capturing tracing logs
pub struct LogBuffer {
    lines: Arc<Mutex<VecDeque<String>>>,
}

impl LogBuffer {
    pub fn new() -> Self {
        Self {
            lines: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    #[allow(dead_code)]
    pub fn push(&self, line: String) {
        let mut lines = self.lines.lock().unwrap();
        lines.push_back(line);
        if lines.len() > MAX_LOGS {
            lines.pop_front();
        }
    }

    #[allow(dead_code)]
    pub fn get_lines(&self) -> Vec<String> {
        self.lines.lock().unwrap().iter().cloned().collect()
    }

    pub fn clone_buffer(&self) -> Arc<Mutex<VecDeque<String>>> {
        Arc::clone(&self.lines)
    }
}

/// Custom tracing layer that captures logs to a buffer
pub struct TuiLogLayer {
    buffer: Arc<Mutex<VecDeque<String>>>,
}

impl TuiLogLayer {
    pub fn new(buffer: Arc<Mutex<VecDeque<String>>>) -> Self {
        Self { buffer }
    }
}

impl<S> Layer<S> for TuiLogLayer
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        id: &tracing::span::Id,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        // If this is a "node" span, extract the "name" field and store it
        if let Some(span) = ctx.span(id) {
            if span.name() == "node" {
                let mut visitor = NameFieldVisitor::default();
                attrs.record(&mut visitor);
                if let Some(name) = visitor.name {
                    span.extensions_mut().insert(NodeNameExtension { name });
                }
            }
        }
    }

    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        // Format the event
        let mut visitor = LogVisitor::new();
        event.record(&mut visitor);

        let level = event.metadata().level();
        let target = event.metadata().target();
        let message = visitor.message;

        // Get current timestamp
        let timestamp = chrono::Local::now().format("%H:%M:%S%.3f");

        // Extract node name from current span context
        let node_name = ctx.event_scope(event).and_then(|scope| {
            // Walk up the span stack looking for a "node" span with stored name
            scope.from_root().find_map(|span| {
                if span.name() == "node" {
                    span.extensions()
                        .get::<NodeNameExtension>()
                        .map(|ext| ext.name.clone())
                } else {
                    None
                }
            })
        });

        // Format with compact, aligned structure
        let formatted = if let Some(name) = node_name {
            format!(
                "{} {:>5} [{:<8}] {}: {}",
                timestamp,
                level,
                name,
                target,
                message
            )
        } else {
            format!(
                "{} {:>5} {:10} {}: {}",
                timestamp,
                level,
                "",  // Empty space where node name would be
                target,
                message
            )
        };

        // Add to buffer
        let mut lines = self.buffer.lock().unwrap();
        lines.push_back(formatted);
        if lines.len() > MAX_LOGS {
            lines.pop_front();
        }
    }
}

/// Extension to store node name in span
#[derive(Clone)]
struct NodeNameExtension {
    name: String,
}

#[derive(Default)]
struct NameFieldVisitor {
    name: Option<String>,
}

impl tracing::field::Visit for NameFieldVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "name" {
            self.name = Some(format!("{:?}", value));
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "name" {
            self.name = Some(value.to_string());
        }
    }
}

/// Visitor to extract message from tracing events
struct LogVisitor {
    message: String,
}

impl LogVisitor {
    fn new() -> Self {
        Self {
            message: String::new(),
        }
    }
}

impl tracing::field::Visit for LogVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{:?}", value);
            // Remove quotes from debug output
            if self.message.starts_with('"') && self.message.ends_with('"') {
                self.message = self.message[1..self.message.len() - 1].to_string();
            }
        } else {
            if !self.message.is_empty() {
                self.message.push_str(", ");
            }
            self.message
                .push_str(&format!("{}={:?}", field.name(), value));
        }
    }
}
