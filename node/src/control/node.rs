use std::{io::IoSlice, sync::Arc};

use anyhow::Result;
use futures::{stream::FuturesUnordered, Future, StreamExt};
use tokio_tun::Tun;
use uninit::uninit_array;

use crate::dev::Device;

#[derive(Debug)]
pub enum ReplyType {
    Wire(Vec<Vec<u8>>),
    Tap(Vec<Vec<u8>>),
}

pub async fn wire_traffic<Fut>(
    tun: &Arc<Tun>,
    dev: &Arc<Device>,
    callable: impl FnOnce([u8; 1500], usize) -> Fut,
) -> Result<()>
where
    Fut: Future<Output = Result<Option<Vec<ReplyType>>>>,
{
    let pkt = uninit_array![u8; 1500];
    let mut pkt = unsafe { std::mem::transmute::<_, [u8; 1500]>(pkt) };
    let Ok(size) = dev.recv(&mut pkt).await else {
        return Ok(());
    };

    let messages = match callable(pkt, size).await {
        Ok(Some(messages)) => messages,
        Ok(None) => return Ok(()),
        Err(e) => {
            tracing::error!(?e, "error");
            return Ok(());
        }
    };

    let mut list = FuturesUnordered::new();
    for reply in messages {
        list.push(async {
            match reply {
                ReplyType::Tap(buf) => {
                    let vec: Vec<IoSlice> = buf.iter().map(|x| IoSlice::new(x)).collect();
                    let _ = tun.send_vectored(&vec).await;
                }
                ReplyType::Wire(reply) => {
                    let vec: Vec<IoSlice> = reply.iter().map(|x| IoSlice::new(x)).collect();
                    let _ = dev.send_vectored(&vec).await;
                }
            };
        });
    }

    while let Some(()) = list.next().await {}
    Ok(())
}

pub async fn tap_traffic<Fut>(
    tun: &Arc<Tun>,
    dev: &Arc<Device>,
    callable: impl FnOnce([u8; 1500], usize) -> Fut,
) -> Result<()>
where
    Fut: Future<Output = Result<Option<Vec<ReplyType>>>>,
{
    let buf = uninit_array![u8; 1500];
    let mut buf = unsafe { std::mem::transmute::<_, [u8; 1500]>(buf) };
    let n = tun.recv(&mut buf).await?;
    let Ok(Some(messages)) = callable(buf, n).await else {
        return Ok(());
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
                    let vec: Vec<IoSlice> = message.iter().map(|x| IoSlice::new(x)).collect();
                    let _ = dev.send_vectored(&vec).await;
                }
            };
        });
    }
    while let Some(()) = list.next().await {}
    Ok(())
}
