use anyhow::{Context, Error, Result};
use std::sync::Arc;
use tokio_tun::Tun;
use tracing::Instrument;
use uninit::uninit_array;
use uuid::Uuid;

pub mod dev;
use dev::Device;

pub mod args;
use args::Args;

use crate::dev::OutgoingMessage;

pub async fn create_with_vdev(args: Args, tun: Arc<Tun>, node: Arc<Device>) -> Result<()> {
    tracing::info!(?args, "setting up node");

    let uuid = Arc::new(if let Some(uuid) = args.uuid {
        uuid
    } else {
        let uuid = Uuid::new_v4();
        uuid
    });

    let span = tracing::error_span!("uuid", ?uuid);

    let tunc = tun.clone();
    let uuidc = uuid.clone();
    let nodec = node.clone();
    tokio::task::spawn(
        async move {
            let node = nodec;
            let mut rx = node.rx.clone();
            rx.borrow_and_update();
            loop {
                if rx.changed().await.is_err() {
                    break Ok::<(), Error>(());
                }

                let pkt = rx.borrow_and_update().clone();
                if pkt.len() < 14 {
                    continue;
                }

                let ethertype = u16::from_be_bytes(pkt[12..14].try_into()?);
                if ethertype != 0x3030 {
                    continue;
                }

                let pkt_id = &pkt[14..14 + 16];
                let source_uuid =
                    Uuid::from_bytes_le(pkt_id.try_into().expect("slice with incorrect length"))
                        .into();
                if uuidc == source_uuid {
                    continue;
                }

                tracing::trace!(?source_uuid, "received traffic to decapsulate");
                let _ = tunc.send_all(&pkt[14 + 16..]).await;
            }
        }
        .instrument(span),
    );

    loop {
        let buf = uninit_array![u8; 1500];
        let mut buf = unsafe { std::mem::transmute::<_, [u8; 1500]>(buf) };
        let n = tun.recv(&mut buf).await?;
        let messages = vec![
            vec![255; 6],
            node.mac_address.bytes().to_vec(),
            [0x30, 0x30].to_vec(),
            uuid.to_bytes_le().to_vec(),
            buf[..n].to_vec(),
        ];
        let _ = node.tx.send(OutgoingMessage::Vectored(messages)).await;
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
