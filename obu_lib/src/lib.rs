pub mod args;
pub use args::{ObuArgs, ObuParameters};

pub mod control;

mod session;

pub use control::Obu;

use anyhow::Result;
use common::device::Device;
use common::tun::Tun;
use std::any::Any;
use std::sync::Arc;

pub trait Node: Send + Sync {
    /// For runtime downcasting to concrete node types.
    fn as_any(&self) -> &dyn Any;
}

impl Node for Obu {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(not(any(test, feature = "test_helpers")))]
pub fn create(args: ObuArgs) -> Result<Arc<dyn Node>> {
    // Use the real tokio_tun builder type in non-test builds.
    use tokio_tun::Tun as RealTokioTun;

    let real_tun: RealTokioTun = if let Some(ip) = args.ip {
        RealTokioTun::builder()
            .name(args.tap_name.as_ref().unwrap_or(&String::default()))
            .tap()
            .mtu(args.mtu)
            .up()
            .address(ip)
            .build()?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("no tun devices returned from TokioTun builder"))?
    } else {
        RealTokioTun::builder()
            .name(args.tap_name.as_ref().unwrap_or(&String::default()))
            .mtu(args.mtu)
            .tap()
            .up()
            .build()?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("no tun devices returned from TokioTun builder"))?
    };

    // Construct the common wrapper directly from the real tokio_tun instance
    // to avoid depending on cfg-gated `From` impls or aliasing differences.
    let tun = Arc::new(Tun::new_real(real_tun));

    let device = Arc::new(Device::new(&args.bind)?);

    Ok(Obu::new(args, tun, device)?)
}

pub fn create_with_vdev(
    args: ObuArgs,
    tun: Arc<Tun>,
    node_device: Arc<Device>,
) -> Result<Arc<dyn Node>> {
    Ok(Obu::new(args, tun, node_device)?)
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
