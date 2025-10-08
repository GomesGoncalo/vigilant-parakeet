pub mod args;
pub use args::{RsuArgs, RsuParameters};

pub mod builder;
pub use builder::RsuBuilder;

pub mod control;

pub use control::Rsu;

#[cfg(any(test, feature = "test_helpers"))]
pub mod test_helpers;

use anyhow::Result;
use common::device::Device;
use common::tun::Tun;
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

pub fn create_with_vdev(
    args: RsuArgs,
    tun: Arc<Tun>,
    node_device: Arc<Device>,
    node_name: String,
) -> Result<Arc<dyn Node>> {
    #[cfg(any(test, feature = "test_helpers"))]
    {
        Ok(RsuBuilder::from_args(args)
            .with_tun(tun)
            .with_device(node_device)
            .with_node_name(node_name)
            .build()?)
    }
    #[cfg(not(any(test, feature = "test_helpers")))]
    {
        Ok(Rsu::new(args, tun, node_device, node_name)?)
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
            tap_name: None,
            ip: None,
            mtu: 1500,
            rsu_params: RsuParameters {
                hello_history: 1,
                hello_periodicity: 5000,
                cached_candidates: 1,
                enable_encryption: false,
            },
        };
        let res = create(args);
        assert!(res.is_err());
    }
}
