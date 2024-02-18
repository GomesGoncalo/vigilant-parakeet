use anyhow::{Context, Result};
use std::sync::Arc;
use tokio_tun::Tun;
use tracing::Instrument;
use uninit::uninit_array;
use uuid::Uuid;

pub mod dev;
use dev::Device;

pub mod args;
use args::Args;

mod message;
use message::Message;

use crate::{dev::OutgoingMessage, message::PacketType};

pub async fn create_with_vdev(args: Args, tun: Arc<Tun>, node: Arc<Device>) -> Result<()> {
    tracing::info!(?args, "setting up node");

    let uuid = Arc::new(if let Some(uuid) = args.uuid {
        uuid
    } else {
        Uuid::new_v4()
    });

    let span = tracing::error_span!("uuid", ?uuid);

    let tunc = tun.clone();
    let uuidc = uuid.clone();
    let nodec = node.clone();
    tokio::task::spawn(
        async move {
            let node = nodec;
            let mut rx = node.get_channel();
            loop {
                let Some(pkt) = rx.recv().await else {
                    continue;
                };

                let Ok(pkt) = Message::try_from(pkt) else {
                    continue;
                };

                if pkt.uuid == *uuidc {
                    continue;
                }

                let span = tracing::error_span!("remoteuuid", ?pkt.uuid);
                let _guard = span.enter();
                match pkt.next_layer() {
                    Ok(PacketType::Control) => {
                        tracing::error!(?pkt.uuid, "received control");
                    }
                    Ok(PacketType::Data(buf)) => {
                        tracing::trace!(?pkt.uuid, "received traffic to decapsulate");
                        let _ = tunc.send_all(buf).await;
                    }
                    Err(e) => tracing::error!(?e, "invalid"),
                }
            }
        }
        .instrument(span),
    );

    loop {
        let buf = uninit_array![u8; 1500];
        let mut buf = unsafe { std::mem::transmute::<_, [u8; 1500]>(buf) };
        let n = tun.recv(&mut buf).await?;
        let messages = Message::new(
            node.mac_address.bytes(),
            [255; 6],
            &uuid,
            &PacketType::Data(&buf[..n]),
        );
        let _ = node
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
