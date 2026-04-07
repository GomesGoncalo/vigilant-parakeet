pub mod args;
pub use args::Args;
pub mod buffer_pool;
pub mod builder;
pub mod control;
pub mod crypto;
pub mod data;
pub mod error;
pub mod messages;
pub mod metrics;

/// Maximum frame size for VANET packet receive buffers.
///
/// Set to 9000 (jumbo-frame size) so that post-quantum key-exchange messages
/// fit in a single frame. The largest current PQ message is a signed
/// ML-KEM-768 + ML-DSA-65 `KeyExchangeInit`: base payload (1197 B) +
/// ML-DSA-65 signed extension (5266 B) + message header (14 B) +
/// control-type bytes (2 B) = 6479 B.
///
/// VANET TAP interfaces must also be configured with MTU ≥ this value;
/// see `simulator/src/node_factory.rs`.
pub const PACKET_BUFFER_SIZE: usize = 9000;

// Type aliases for common complex types to improve readability
use common::device::Device;
use common::tun::Tun;
use std::sync::{Arc, RwLock};

/// Shared reference to a network device
pub type SharedDevice = Arc<Device>;

/// Shared reference to a TUN/TAP interface
pub type SharedTun = Arc<Tun>;

/// Thread-safe shared mutable reference (used for routing state)
pub type Shared<T> = Arc<RwLock<T>>;

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
