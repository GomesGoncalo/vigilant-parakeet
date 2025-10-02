use node_lib::messages::message::Message;

use super::routing::Routing;
use anyhow::{bail, Result};
use common::device::Device;
use common::tun::Tun;
use futures::{future::join_all, Future};
use itertools::Itertools;
use std::{io::IoSlice, sync::Arc};
use uninit::uninit_array;

#[derive(Debug)]
pub enum ReplyType {
    Wire(Vec<Vec<u8>>),
    Tap(Vec<Vec<u8>>),
    // New flat variants for improved performance
    WireFlat(Vec<u8>),
    TapFlat(Vec<u8>),
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
                    ReplyType::TapFlat(x) => Some(DebugReplyType::Tap(vec![x.clone()])),
                    ReplyType::WireFlat(x) => {
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

/// Return a compact hex string for a byte slice (e.g. "01 02 aa ...").
pub fn bytes_to_hex(slice: &[u8]) -> String {
    slice
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<Vec<_>>()
        .join(" ")
}

pub async fn handle_messages(
    messages: Vec<ReplyType>,
    tun: &Arc<Tun>,
    dev: &Arc<Device>,
    routing: Option<Arc<std::sync::RwLock<Routing>>>,
) -> Result<()> {
    let future_vec = messages
        .iter()
        .map(|reply| {
            let routing_clone = routing.clone();
            async move {
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
                        // Attempt to send; on error, if routing is present and this
                        // wire payload targets the cached upstream for an OBU,
                        // trigger a failover to the next candidate.
                        let send_res = dev.send_vectored(&vec).await;
                        if let Err(e) = send_res {
                            tracing::error!(?e, "error sending to dev");

                            // Try to parse the wire bytes; if this message's destination
                            // equals the current cached upstream, trigger a failover.
                            if let Some(r) = &routing_clone {
                                let bytes: Vec<u8> = reply.iter().flat_map(|x| x.iter()).copied().collect();
                                if let Ok(parsed) = Message::try_from(&bytes[..]) {
                                    if let Ok(dest) = parsed.to() {
                                        let cached = match r.read() {
                                            Ok(guard) => guard.get_cached_upstream(),
                                            Err(poisoned) => {
                                                tracing::error!("routing lock poisoned during failover check, attempting recovery");
                                                poisoned.into_inner().get_cached_upstream()
                                            }
                                        };
                                        if let Some(cached_mac) = cached {
                                            if cached_mac == dest {
                                                let promoted = match r.write() {
                                                    Ok(guard) => guard.failover_cached_upstream(),
                                                    Err(poisoned) => {
                                                        tracing::error!("routing write lock poisoned during failover, attempting recovery");
                                                        poisoned.into_inner().failover_cached_upstream()
                                                    }
                                                };
                                                tracing::info!(promoted = ?promoted, "promoted cached upstream after send error");
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    // New flat variants - single allocation, zero fragmentation
                    ReplyType::TapFlat(buf) => {
                        let vec = [IoSlice::new(buf)];
                        let _ = tun
                            .send_vectored(&vec)
                            .await
                            .inspect_err(|e| tracing::error!(?e, "error sending to tap"));
                    }
                    ReplyType::WireFlat(buf) => {
                        let vec = [IoSlice::new(buf)];
                        let send_res = dev.send_vectored(&vec).await;
                        if let Err(e) = send_res {
                            tracing::error!(?e, "error sending to dev");

                            // Failover logic for flat buffers
                            if let Some(r) = &routing_clone {
                                if let Ok(parsed) = Message::try_from(&buf[..]) {
                                    if let Ok(dest) = parsed.to() {
                                        let cached = match r.read() {
                                            Ok(guard) => guard.get_cached_upstream(),
                                            Err(poisoned) => {
                                                tracing::error!("routing lock poisoned during failover check, attempting recovery");
                                                poisoned.into_inner().get_cached_upstream()
                                            }
                                        };
                                        if let Some(cached_mac) = cached {
                                            if cached_mac == dest {
                                                let promoted = match r.write() {
                                                    Ok(guard) => guard.failover_cached_upstream(),
                                                    Err(poisoned) => {
                                                        tracing::error!("routing write lock poisoned during failover, attempting recovery");
                                                        poisoned.into_inner().failover_cached_upstream()
                                                    }
                                                };
                                                tracing::info!(promoted = ?promoted, "promoted cached upstream after send error");
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                };
            }
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
    // Also emit a debug so test output can capture the raw bytes when tracing
    tracing::trace!(n = n, raw = %bytes_to_hex(&buf[..n]), "wire_traffic recv");
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

#[cfg(test)]
mod tests {
    use super::{get_msgs, ReplyType};
    use crate::control::node::DebugReplyType;
    use anyhow::Result;
    use node_lib::messages::message::Message;

    #[test]
    fn get_msgs_ok_none() {
        let res: Result<Option<Vec<ReplyType>>> = Ok(None);
        let out = get_msgs(&res).expect("ok none");
        assert!(out.is_none());
    }

    #[test]
    fn get_msgs_ok_some_with_unparsable_wire() {
        // ReplyType::Wire with random bytes that won't parse to Message -> filtered out
        let replies = vec![ReplyType::Wire(vec![vec![0u8; 3]])];
        let res: Result<Option<Vec<ReplyType>>> = Ok(Some(replies));
        let dbg = get_msgs(&res).expect("ok some").expect("some");
        // should filter out unparsable wire entries
        assert!(dbg.is_empty());
    }

    #[test]
    fn get_msgs_ok_some_with_parsable_wire() {
        use mac_address::MacAddress;
        use node_lib::messages::data::Data;
        use node_lib::messages::data::ToUpstream;
        use node_lib::messages::packet_type::PacketType;

        let from: MacAddress = [2u8; 6].into();
        let to: MacAddress = [3u8; 6].into();
        let payload = b"hi";
        let tu = ToUpstream::new(from, payload);
        let data = Data::Upstream(tu);
        let pkt = PacketType::Data(data);
        let message = Message::new(from, to, pkt);

        let wire: Vec<u8> = (&message).into();
        let replies = vec![ReplyType::WireFlat(wire)];
        let res: Result<Option<Vec<ReplyType>>> = Ok(Some(replies));
        let dbg = get_msgs(&res).expect("ok some").expect("some");
        // should contain one WireFlat debug entry
        assert_eq!(dbg.len(), 1);
        match &dbg[0] {
            DebugReplyType::Wire(s) => {
                assert!(s.contains("Message"));
            }
            _ => panic!("expected Wire debug string"),
        }
    }
}
