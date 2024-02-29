pub mod control;
pub mod dev;
mod messages;

use crate::control::{args::NodeType, obu::Obu, rsu::Rsu};
use anyhow::{Context, Result};
use control::{
    args::Args,
    node::{Node, ReplyType},
};
use dev::{Device, OutgoingMessage};
use futures::{stream::FuturesUnordered, StreamExt};
use messages::{Data, Message};
use std::{io::IoSlice, sync::Arc};
use tokio_tun::Tun;
use tracing::Instrument;
use uninit::uninit_array;

async fn create_with_vdev_with_node<N>(
    tun: Arc<Tun>,
    node_device: Arc<Device>,
    node: Arc<N>,
) -> Result<()>
where
    N: Node + Sync + Send + 'static,
{
    let node_devicec = node_device.clone();
    let tunc = tun.clone();
    let nodec = node.clone();
    tokio::task::spawn(
        async move {
            let tun = tunc;
            let node = nodec;
            let node_device = node_devicec;
            let mut rx = node_device.get_channel();
            loop {
                let Some(pkt) = rx.recv().await else {
                    continue;
                };

                let Ok(pkt) = Message::try_from(pkt) else {
                    continue;
                };

                let reply_vec = match node.handle_msg(pkt) {
                    Ok(reply_vec) => reply_vec,
                    Err(e) => {
                        tracing::error!(?e, "returned error when handling message");
                        continue;
                    }
                };

                let Some(reply_vec) = reply_vec else {
                    continue;
                };

                let mut list = FuturesUnordered::new();
                for reply in reply_vec {
                    list.push(async {
                        match reply {
                            ReplyType::Tap(buf) => {
                                let vec: Vec<IoSlice> =
                                    buf.iter().map(|x| IoSlice::new(x)).collect();
                                let _ = tun.send_vectored(&vec).await;
                            }
                            ReplyType::Wire(reply) => {
                                let _ = node_device.tx.send(OutgoingMessage::Vectored(reply)).await;
                            }
                        };
                    });
                }

                while let Some(()) = list.next().await {}
            }
        }
        .in_current_span(),
    );

    node.generate(node_device.clone());

    loop {
        let buf = uninit_array![u8; 1500];
        let mut buf = unsafe { std::mem::transmute::<_, [u8; 1500]>(buf) };
        let n = tun.recv(&mut buf).await?;
        let Ok(Some(messages)) = node.tap_traffic(Data::new(node.get_mac(), &buf[..n]).into())
        else {
            continue;
        };

        let mut list = FuturesUnordered::new();
        for message in messages {
            list.push(async {
                match message {
                    ReplyType::Tap(buf) => {
                        let vec: Vec<IoSlice> = buf.iter().map(|x| IoSlice::new(x)).collect();
                        let _ = tun.send_vectored(&vec).await;
                    }
                    ReplyType::Wire(message) => {
                        let _ = node_device
                            .tx
                            .send(OutgoingMessage::Vectored(message))
                            .await;
                    }
                };
            });
        }
        while let Some(()) = list.next().await {}
    }
}

pub async fn create_with_vdev(args: Args, tun: Arc<Tun>, node_device: Arc<Device>) -> Result<()> {
    let mac_address = node_device.mac_address;
    match args.node_params.node_type {
        NodeType::Rsu => {
            create_with_vdev_with_node(tun, node_device, Rsu::new(args, mac_address)?.into()).await
        }
        NodeType::Obu => {
            create_with_vdev_with_node(tun, node_device, Obu::new(args, mac_address)?.into()).await
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
