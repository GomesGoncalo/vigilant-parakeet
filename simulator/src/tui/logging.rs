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
    S: tracing::Subscriber,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        // Format the event
        let mut visitor = LogVisitor::new();
        event.record(&mut visitor);

        let level = event.metadata().level();
        let target = event.metadata().target();
        let message = visitor.message;

        let formatted = format!("[{:5}] {}: {}", level, target, message);

        // Add to buffer
        let mut lines = self.buffer.lock().unwrap();
        lines.push_back(formatted);
        if lines.len() > MAX_LOGS {
            lines.pop_front();
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
