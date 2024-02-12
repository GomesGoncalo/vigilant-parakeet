use anyhow::{Context, Result};
use std::sync::Arc;
use tokio_tun::Tun;
use uuid::Uuid;

mod dev;
use dev::Device;

pub mod args;
use args::Args;

use crate::dev::OutgoingMessage;

pub async fn create(args: Args) -> Result<()> {
    tracing::info!(args = ?args, "created with args");

    let uuid = Arc::new(match args.uuid {
        Some(uuid) => uuid,
        None => {
            let uuid = Uuid::new_v4();
            tracing::info!(uuid = %uuid, "set uuid");
            uuid
        }
    });

    let tun = Arc::new(
        Tun::builder()
            .name(&args.tap_name.unwrap_or("".to_string()))
            .tap(true)
            .packet_info(false)
            .up()
            .try_build()
            .unwrap(),
    );

    let name = tun.name().to_owned();
    tracing::info!(name = name, "Created tap device");

    let (_device, mut receiver, sender) = Device::new(&args.bind).context("created the device")?;

    let tunc = tun.clone();
    let uuidc = uuid.clone();
    tokio::task::spawn(async move {
        let own_id = uuidc.to_bytes_le();
        let _ = *receiver.borrow_and_update();
        loop {
            if receiver.changed().await.is_err() {
                break;
            }

            let pkt = receiver.borrow_and_update().clone();
            if pkt.len() < 14 {
                continue;
            }

            let ethertype = u16::from_be_bytes(pkt[12..14].try_into().unwrap());
            if ethertype != 0x3030 {
                continue;
            }

            if own_id == pkt[14..14 + 16] {
                continue;
            }

            tracing::trace!(
                packet = ?&pkt[14 + 16..],
                "received from the physical device"
            );

            let _ = tunc.send_all(&pkt[14 + 16..]).await;
        }
    });

    loop {
        let mut buf = [0u8; 1500];
        let n = tun.recv(&mut buf).await.unwrap();
        tracing::debug!(
            n = n, packet = ?buf[..n],
            "received from the virtual device"
        );
        let mut messages = Vec::new();
        let payload = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x30, 0x30];
        messages.push(payload.to_vec());
        messages.push(uuid.to_bytes_le().to_vec());
        messages.push(buf[..n].to_vec());
        let _ = sender.send(OutgoingMessage::Vectored(messages)).await;
    }
}
