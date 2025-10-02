pub mod args;
pub use args::{ObuArgs, ObuParameters};

pub mod builder;
pub use builder::ObuBuilder;

pub mod control;

mod session;

pub use control::Obu;

use anyhow::Result;
use common::device::Device;
use common::tun::Tun;
use std::sync::Arc;

// Re-export the shared Node trait from node_lib
pub use node_lib::Node;

impl Node for Obu {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(not(any(test, feature = "test_helpers")))]
pub fn create(args: ObuArgs) -> Result<Arc<dyn Node>> {
    Ok(ObuBuilder::from_args(args).build()?)
}

pub fn create_with_vdev(
    args: ObuArgs,
    tun: Arc<Tun>,
    node_device: Arc<Device>,
) -> Result<Arc<dyn Node>> {
    #[cfg(any(test, feature = "test_helpers"))]
    {
        Ok(ObuBuilder::from_args(args)
            .with_tun(tun)
            .with_device(node_device)
            .build()?)
    }
    #[cfg(not(any(test, feature = "test_helpers")))]
    {
        Ok(Obu::new(args, tun, node_device)?)
    }
}

// Test-friendly stub exposed when test_helpers is enabled so downstream
// binaries compile during coverage runs. Tests should use create_with_vdev
// to inject shims instead of this stub.
#[cfg(any(test, feature = "test_helpers"))]
pub fn create(_args: ObuArgs) -> Result<Arc<dyn Node>> {
    Err(anyhow::anyhow!(
        "obu_lib::create is disabled when test_helpers is enabled; use create_with_vdev in tests"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_stub_returns_error_under_test_helpers() {
        // When compiled under test_helpers, the create() stub should return Err.
        let args = ObuArgs {
            bind: String::new(),
            tap_name: None,
            ip: None,
            mtu: 1500,
            obu_params: ObuParameters {
                hello_history: 1,
                cached_candidates: 1,
                enable_encryption: false,
            },
        };
        let res = create(args);
        assert!(res.is_err());
    }
}
