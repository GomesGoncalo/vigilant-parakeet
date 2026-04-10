pub mod admin;

pub mod args;
pub use args::{ObuArgs, ObuParameters};

pub mod builder;
pub use builder::ObuBuilder;

pub mod control;

pub use control::routing::RssiTable;
pub use control::Obu;

#[cfg(any(test, feature = "test_helpers"))]
pub mod test_helpers;

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
    node_name: String,
) -> Result<Arc<dyn Node>> {
    Ok(create_obu(args, tun, node_device, node_name)?)
}

/// Like `create_with_vdev` but returns `Arc<Obu>` directly for callers that
/// need access to the concrete type (e.g. to start the admin interface).
pub fn create_obu(
    args: ObuArgs,
    tun: Arc<Tun>,
    node_device: Arc<Device>,
    node_name: String,
) -> Result<Arc<Obu>> {
    #[cfg(any(test, feature = "test_helpers"))]
    {
        ObuBuilder::from_args(args)
            .with_tun(tun)
            .with_device(node_device)
            .with_node_name(node_name)
            .build()
    }
    #[cfg(not(any(test, feature = "test_helpers")))]
    {
        Obu::new(args, tun, node_device, node_name)
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
                enable_dh_signatures: false,
                signing_key_seed: None,
                server_signing_pubkey: None,
                dh_rekey_interval_ms: 60_000,
                dh_key_lifetime_ms: 120_000,
                dh_reply_timeout_ms: 5_000,
                cipher: node_lib::crypto::SymmetricCipher::default(),
                kdf: node_lib::crypto::KdfAlgorithm::default(),
                dh_group: node_lib::crypto::DhGroup::default(),
                signing_algorithm: node_lib::crypto::SigningAlgorithm::default(),
            },
        };
        let res = create(args);
        assert!(res.is_err());
    }
}
