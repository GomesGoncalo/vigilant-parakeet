pub mod args;
pub mod control;
mod data;
mod messages;

use anyhow::{Context, Result};
use args::{Args, NodeType};
use common::device::Device;
use control::node::ReplyType;
use std::sync::Arc;
use common::tun::Tun;
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
            .tap(true)
            .mtu(args.mtu)
            .packet_info(false)
            .up()
            .address(args.ip.context("no ip")?)
            .try_build()?
    } else {
        TokioTun::builder()
            .name(args.tap_name.as_ref().unwrap_or(&String::default()))
            .mtu(args.mtu)
            .tap(true)
            .packet_info(false)
            .up()
            .try_build()?
    }));

    let dev = Device::new(&args.bind)?;
    create_with_vdev(args, tun, dev.into())
}
