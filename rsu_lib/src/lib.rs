pub mod admin;

pub mod args;
pub use args::{RsuArgs, RsuParameters};

pub mod builder;
pub use builder::RsuBuilder;

pub mod control;

pub use control::Rsu;

#[cfg(feature = "libp2p_gossipsub")]
pub mod gossipsub;

#[cfg(any(test, feature = "test_helpers"))]
pub mod test_helpers;

use anyhow::Result;
use common::device::Device;
use std::sync::Arc;

// Re-export the shared Node trait from node_lib
pub use node_lib::Node;

impl Node for Rsu {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(not(any(test, feature = "test_helpers")))]
pub fn create(args: RsuArgs) -> Result<Arc<dyn Node>> {
    Ok(RsuBuilder::from_args(args).build()?)
}

/// Create an RSU node with a pre-built device (for simulator and tests).
///
/// RSU nodes no longer require a TUN device. They only use the VANET device
/// for wireless communication and a cloud socket for server connectivity.
pub fn create_with_vdev(
    args: RsuArgs,
    node_device: Arc<Device>,
    node_name: String,
) -> Result<Arc<dyn Node>> {
    Ok(create_rsu(args, node_device, node_name)?)
}

/// Like `create_with_vdev` but returns `Arc<Rsu>` directly for callers that
/// need access to the concrete type (e.g. to start the admin interface).
pub fn create_rsu(args: RsuArgs, node_device: Arc<Device>, node_name: String) -> Result<Arc<Rsu>> {
    #[cfg(any(test, feature = "test_helpers"))]
    {
        RsuBuilder::from_args(args)
            .with_device(node_device)
            .with_node_name(node_name)
            .build()
    }
    #[cfg(not(any(test, feature = "test_helpers")))]
    {
        Rsu::new(args, node_device, node_name)
    }
}

// Provide a test-friendly stub when the crate is compiled with the
// `test_helpers` feature (or under `cfg(test)`) so downstream binaries
// that reference `rsu_lib::create` still link during coverage runs.
#[cfg(any(test, feature = "test_helpers"))]
pub fn create(_args: RsuArgs) -> Result<Arc<dyn Node>> {
    // Return an error directing tests to use the test helpers / create_with_vdev
    Err(anyhow::anyhow!(
        "rsu_lib::create is disabled when test_helpers is enabled; use create_with_vdev in tests"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_stub_returns_error_under_test_helpers() {
        let args = RsuArgs {
            bind: String::new(),
            mtu: 1500,
            cloud_ip: None,
            rsu_params: RsuParameters {
                hello_history: 1,
                hello_periodicity: 5000,
                cached_candidates: 1,
                server_ip: None,
                server_port: 8080,
            },
        };
        let res = create(args);
        assert!(res.is_err());
    }
}
