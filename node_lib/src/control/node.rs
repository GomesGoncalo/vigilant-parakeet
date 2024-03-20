use crate::messages::message::Message;
use anyhow::{bail, Result};
use common::device::Device;
use futures::{future::join_all, Future};
use itertools::Itertools;
use std::{io::IoSlice, sync::Arc};
use tokio_tun::Tun;
use uninit::uninit_array;

#[derive(Debug)]
pub enum ReplyType {
    Wire(Vec<Vec<u8>>),
    Tap(Vec<Vec<u8>>),
}

// This code is only used to trace the messages and so it is mostly unused
// We want to suppress the warnings
#[derive(Debug)]
#[allow(dead_code)]
pub enum DebugReplyType {
    Tap(Vec<Vec<u8>>),
    Wire(String),
}

pub fn get_msgs(response: &Result<Option<Vec<ReplyType>>>) -> Result<Option<Vec<DebugReplyType>>> {
    match response {
        Ok(Some(response)) => Ok(Some(
            response
                .iter()
                .filter_map(|x| match x {
                    ReplyType::Tap(x) => Some(DebugReplyType::Tap(x.clone())),
                    ReplyType::Wire(x) => {
                        let x = x.iter().flat_map(|x| x.iter()).copied().collect_vec();
                        let Ok(message) = Message::try_from(&x[..]) else {
                            return None;
                        };
                        Some(DebugReplyType::Wire(format!("{message:?}")))
                    }
                })
                .collect_vec(),
        )),
        Ok(None) => Ok(None),
        Err(e) => bail!("{:?}", e),
    }
}

pub async fn handle_messages(
    messages: Vec<ReplyType>,
    tun: &Arc<Tun>,
    dev: &Arc<Device>,
) -> Result<()> {
    let future_vec = messages
        .iter()
        .map(|reply| async move {
            match reply {
                ReplyType::Tap(buf) => {
                    let vec: Vec<IoSlice> = buf.iter().map(|x| IoSlice::new(x)).collect();
                    let _ = tun
                        .send_vectored(&vec)
                        .await
                        .inspect_err(|e| tracing::error!(?e, "error sending to tap"));
                }
                ReplyType::Wire(reply) => {
                    let vec: Vec<IoSlice> = reply.iter().map(|x| IoSlice::new(x)).collect();
                    let _ = dev
                        .send_vectored(&vec)
                        .await
                        .inspect_err(|e| tracing::error!(?e, "error sending to dev"));
                }
            };
        })
        .collect_vec();

    join_all(future_vec).await;
    Ok(())
}

fn buffer() -> [u8; 1500] {
    let buf = uninit_array![u8; 1500];
    unsafe { std::mem::transmute::<_, [u8; 1500]>(buf) }
}

pub async fn wire_traffic<Fut>(
    dev: &Arc<Device>,
    callable: impl FnOnce([u8; 1500], usize) -> Fut,
) -> Result<Option<Vec<ReplyType>>>
where
    Fut: Future<Output = Result<Option<Vec<ReplyType>>>>,
{
    let mut buf = buffer();
    let n = dev.recv(&mut buf).await?;
    callable(buf, n).await
}

pub async fn tap_traffic<Fut>(
    dev: &Arc<Tun>,
    callable: impl FnOnce([u8; 1500], usize) -> Fut,
) -> Result<Option<Vec<ReplyType>>>
where
    Fut: Future<Output = Result<Option<Vec<ReplyType>>>>,
{
    let mut buf = buffer();
    let n = dev.recv(&mut buf).await?;
    callable(buf, n).await
}
