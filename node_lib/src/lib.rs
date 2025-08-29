pub mod args;
pub use args::Args;
pub mod control;
mod data;
pub mod messages;

use anyhow::{Context, Result};
use args::NodeType;
use common::device::Device;
use common::tun::Tun;
use std::sync::Arc;

pub trait Node {}

impl Node for control::rsu::Rsu {}
impl Node for control::obu::Obu {}

pub fn create_with_vdev(
    args: Args,
    tun: Arc<Tun>,
    node_device: Arc<Device>,
) -> Result<Arc<dyn Node>> {
    match args.node_params.node_type {
        NodeType::Rsu => Ok(control::rsu::Rsu::new(args, tun, node_device)?),
        NodeType::Obu => Ok(control::obu::Obu::new(args, tun, node_device)?),
    }
}

#[cfg(not(any(test, feature = "test_helpers")))]
pub fn create(args: Args) -> Result<Arc<dyn Node>> {
    // Use the real tokio_tun builder type in non-test builds.
    use tokio_tun::Tun as RealTokioTun;

    let real_tun: RealTokioTun = if args.ip.is_some() {
        RealTokioTun::builder()
            .name(args.tap_name.as_ref().unwrap_or(&String::default()))
            .tap()
            .mtu(args.mtu)
            .up()
            .address(args.ip.context("no ip")?)
            .build()?
            .into_iter()
            .next()
            .expect("Expecting at least 1 item in vec")
    } else {
        RealTokioTun::builder()
            .name(args.tap_name.as_ref().unwrap_or(&String::default()))
            .mtu(args.mtu)
            .tap()
            .up()
            .build()?
            .into_iter()
            .next()
            .expect("Expecting at least 1 item in vec")
    };

    // Use From/Into impl to convert the concrete real_tun into our `Tun`.
    let tun = Arc::new(Tun::from(real_tun));

    let dev = Device::new(&args.bind)?;
    create_with_vdev(args, tun, dev.into())
}

#[cfg(any(test, feature = "test_helpers"))]
pub fn create(_args: Args) -> Result<Arc<dyn Node>> {
    anyhow::bail!("create() with TokioTun::builder() is unavailable in test builds")
}
