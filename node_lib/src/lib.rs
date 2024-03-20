pub mod args;
pub mod control;
mod data;
mod messages;

use anyhow::{Context, Result};
use args::{Args, NodeType};
use common::device::Device;
use control::node::ReplyType;
use std::sync::Arc;
use tokio_tun::Tun;

pub async fn create_with_vdev(args: Args, tun: Arc<Tun>, node_device: Arc<Device>) -> Result<()> {
    match args.node_params.node_type {
        NodeType::Rsu => {
            let rsu = control::rsu::Rsu::new(args, tun, node_device)?;
            let _ = rsu
                .process()
                .await
                .inspect_err(|e| tracing::error!(?e, "error"));
            Ok(())
        }
        NodeType::Obu => {
            let obu = control::obu::Obu::new(args, tun, node_device)?;
            let _ = obu.process().await;
            Ok(())
        }
    }
}

pub async fn create(args: Args) -> Result<()> {
    let tun = Arc::new(if args.ip.is_some() {
        Tun::builder()
            .name(args.tap_name.as_ref().unwrap_or(&String::default()))
            .tap(true)
            .mtu(args.mtu)
            .packet_info(false)
            .up()
            .address(args.ip.context("no ip")?)
            .try_build()?
    } else {
        Tun::builder()
            .name(args.tap_name.as_ref().unwrap_or(&String::default()))
            .mtu(args.mtu)
            .tap(true)
            .packet_info(false)
            .up()
            .try_build()?
    });

    let dev = Device::new(&args.bind)?;
    create_with_vdev(args, tun, dev.into()).await
}
