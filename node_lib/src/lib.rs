pub mod args;
pub use args::Args;
pub mod control;
mod data;
pub mod messages;

use anyhow::{Context, Result};
use args::NodeType;
use common::device::Device;
use common::tun::Tun;
use control::node::ReplyType;
use std::sync::Arc;
use tokio_tun::Tun as TokioTun;

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

pub fn create(args: Args) -> Result<Arc<dyn Node>> {
    let tun = Arc::new(Tun::new(if args.ip.is_some() {
        TokioTun::builder()
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
        TokioTun::builder()
            .name(args.tap_name.as_ref().unwrap_or(&String::default()))
            .mtu(args.mtu)
            .tap()
            .up()
            .build()?
            .into_iter()
            .next()
            .expect("Expecting at least 1 item in vec")
    }));

    let dev = Device::new(&args.bind)?;
    create_with_vdev(args, tun, dev.into())
}
