pub mod control;
pub mod dev;
mod messages;

use crate::control::{args::NodeType, obu::Obu, rsu::Rsu};
use anyhow::{Context, Result};
use control::{args::Args, node::ReplyType};
use dev::Device;
use std::sync::Arc;
use tokio_tun::Tun;

pub async fn create_with_vdev(args: Args, tun: Arc<Tun>, node_device: Arc<Device>) -> Result<()> {
    let mac_address = node_device.mac_address;
    match args.node_params.node_type {
        NodeType::Rsu => {
            let rsu = Rsu::new(args, mac_address, tun, node_device)?;
            loop {
                rsu.process().await;
            }
        }
        NodeType::Obu => {
            let obu = Obu::new(args, mac_address, tun, node_device)?;
            loop {
                obu.process().await;
            }
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
