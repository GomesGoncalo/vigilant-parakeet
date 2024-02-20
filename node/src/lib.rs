use anyhow::{Context, Result};
use std::{io::IoSlice, sync::Arc};
use tokio_tun::Tun;
use uninit::uninit_array;

pub mod dev;
use dev::{Device, OutgoingMessage};

pub mod control;
use control::{args::Args, node::ReplyType, Node};

mod messages;
use messages::{Message, PacketType};

pub async fn create_with_vdev(args: Args, tun: Arc<Tun>, node_device: Arc<Device>) -> Result<()> {
    tracing::info!(?args, "setting up node");

    let node = Arc::new(Node::new(args, &node_device.mac_address));
    let node_devicec = node_device.clone();
    let tunc = tun.clone();
    let nodec = node.clone();
    tokio::task::spawn(async move {
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

            let Ok(reply) = node.handle_msg(&pkt) else {
                continue;
            };

            match reply {
                Some(ReplyType::Tap(buf)) => {
                    let vec: Vec<IoSlice> = buf.iter().map(|x| IoSlice::new(x)).collect();
                    let _ = tun.send_vectored(&vec).await;
                }
                Some(ReplyType::Wire(reply)) => {
                    let _ = node_device
                        .tx
                        .send(OutgoingMessage::Vectored(reply.into()))
                        .await;
                }
                None => (),
            };
        }
    });

    node.generate(node_device.clone());

    loop {
        let buf = uninit_array![u8; 1500];
        let mut buf = unsafe { std::mem::transmute::<_, [u8; 1500]>(buf) };
        let n = tun.recv(&mut buf).await?;
        let messages = Message::new(
            node_device.mac_address.bytes(),
            [255; 6],
            &PacketType::Data(&buf[..n]),
        );
        let _ = node_device
            .tx
            .send(OutgoingMessage::Vectored(messages.into()))
            .await;
    }
}

pub async fn create(args: Args) -> Result<()> {
    let tun = Arc::new(if args.ip.is_some() {
        Tun::builder()
            .name(args.tap_name.as_ref().unwrap_or(&String::default()))
            .tap(true)
            .packet_info(false)
            .up()
            .address(args.ip.context("no ip")?)
            .try_build()?
    } else {
        Tun::builder()
            .name(args.tap_name.as_ref().unwrap_or(&String::default()))
            .tap(true)
            .packet_info(false)
            .up()
            .try_build()?
    });

    let dev = Device::new(&args.bind).context("created the device")?;

    create_with_vdev(args, tun, dev.into()).await
}
