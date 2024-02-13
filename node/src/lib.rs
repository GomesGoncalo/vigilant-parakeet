use anyhow::{Context, Error, Result};
use std::sync::Arc;
use tokio::sync::{mpsc::Sender, watch::Receiver};
use tokio_tun::Tun;
use uninit::uninit_array;
use uuid::Uuid;

pub mod dev;
use dev::Device;

pub mod args;
use args::Args;

use crate::dev::OutgoingMessage;

pub async fn create_with_vdev(
    args: Args,
    tun: Arc<Tun>,
    tuple: (Device, Receiver<Arc<Vec<u8>>>, Sender<OutgoingMessage>),
) -> Result<()> {
    tracing::info!(args = ?args, "created with args");

    let uuid = Arc::new(match args.uuid {
        Some(uuid) => uuid,
        None => {
            let uuid = Uuid::new_v4();
            tracing::info!(uuid = %uuid, "set uuid");
            uuid
        }
    });

    let (device, mut receiver, sender) = tuple;

    let name = tun.name().to_owned();
    tracing::info!(name = name, "Created tap device");

    let tunc = tun.clone();
    let uuidc = uuid.clone();
    tokio::task::spawn(async move {
        let own_id = uuidc.to_bytes_le();
        receiver.borrow_and_update();
        loop {
            if receiver.changed().await.is_err() {
                break Ok::<(), Error>(());
            }

            let pkt = receiver.borrow_and_update().clone();
            if pkt.len() < 14 {
                continue;
            }

            let ethertype = u16::from_be_bytes(pkt[12..14].try_into()?);
            if ethertype != 0x3030 {
                continue;
            }

            if own_id == pkt[14..14 + 16] {
                continue;
            }

            let _ = tunc.send_all(&pkt[14 + 16..]).await;
        }
    });

    loop {
        let buf = uninit_array![u8; 1500];
        let mut buf = buf
            .iter()
            .take(1500)
            .map(|mu| unsafe { mu.assume_init() })
            .collect::<Vec<_>>();
        let n = tun.recv(&mut buf).await?;
        let mut messages = Vec::new();
        messages.push(vec![255; 6]);
        messages.push(device.mac_address.bytes().to_vec());
        messages.push([0x30, 0x30].to_vec());
        messages.push(uuid.to_bytes_le().to_vec());
        messages.push(buf[..n].to_vec());
        let _ = sender.send(OutgoingMessage::Vectored(messages)).await;
    }
}

pub async fn create(args: Args) -> Result<()> {
    let tun = Arc::new(if args.ip.is_some() {
        Tun::builder()
            .name(&args.tap_name.as_ref().unwrap_or(&"".to_string()))
            .tap(true)
            .packet_info(false)
            .up()
            .address(args.ip.context("no ip")?)
            .try_build()?
    } else {
        Tun::builder()
            .name(&args.tap_name.as_ref().unwrap_or(&"".to_string()))
            .tap(true)
            .packet_info(false)
            .up()
            .try_build()?
    });

    let tuple = Device::new(&args.bind).context("created the device")?;

    create_with_vdev(args, tun, tuple).await
}
