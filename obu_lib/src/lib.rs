pub mod args;
pub use args::{ObuArgs, ObuParameters};

pub mod control;

mod routing;
mod session;

pub use control::Obu;

use anyhow::Result;
use common::tun::Tun;
use common::{device::Device, network_interface::NetworkInterface};
use mac_address::MacAddress;
use std::any::Any;
use std::{
    io::IoSlice,
    sync::{Arc, RwLock},
};
use tokio::time::Instant;

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

    let real_tun: RealTokioTun = if args.ip.is_some() {
        RealTokioTun::builder()
            .name(args.tap_name.as_ref().unwrap_or(&String::default()))
            .tap()
            .mtu(args.mtu)
            .up()
            .address(args.ip.unwrap())
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

    let tun = Arc::new(Tun::new(real_tun));

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
