pub mod args;
pub use args::Args;
pub mod buffer_pool;
pub mod control;
pub mod crypto;
pub mod data;
pub mod error;
pub mod messages;
pub mod metrics;

/// Shared trait for all node types (OBU and RSU).
/// Provides a common interface for runtime downcasting to concrete node types.
pub trait Node: Send + Sync {
    /// For runtime downcasting to concrete node types.
    fn as_any(&self) -> &dyn std::any::Any;
}
// Re-export test helpers for integration tests.
// Make this available unconditionally so integration tests can import
// `node_lib::test_helpers::hub` without passing a feature flag.
// The helper code is small and test-oriented; keeping it always exported
// avoids CI friction when running integration tests.
pub mod test_helpers {
    pub mod hub;
    pub mod util;
}

/// Initialize a tracing subscriber for tests. Safe to call multiple times.
pub fn init_test_tracing() {
    use std::sync::Once;
    static START: Once = Once::new();
    START.call_once(|| {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .try_init();
    });
}
