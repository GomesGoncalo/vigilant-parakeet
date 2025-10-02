use anyhow::Result;
use common::device::Device;
use common::tun::Tun;
use futures::{future::join_all, Future};
use itertools::Itertools;
use std::{io::IoSlice, sync::Arc};
use uninit::uninit_array;

use crate::{SharedDevice, SharedTun, PACKET_BUFFER_SIZE};

#[derive(Debug)]
pub enum ReplyType {
    /// Wire traffic (to device) - flat serialization
    WireFlat(Vec<u8>),
    /// TAP traffic (to tun) - flat serialization
    TapFlat(Vec<u8>),
}

// Debug types and functions for tracing and testing
#[cfg(any(test, feature = "test_helpers"))]
#[derive(Debug)]
#[allow(dead_code)]
pub enum DebugReplyType {
    Tap(Vec<Vec<u8>>),
    Wire(String),
}

#[cfg(any(test, feature = "test_helpers"))]
pub fn get_msgs(response: &Result<Option<Vec<ReplyType>>>) -> Result<Option<Vec<DebugReplyType>>> {
    use crate::messages::message::Message;
    use anyhow::bail;
    use itertools::Itertools;

    match response {
        Ok(Some(response)) => Ok(Some(
            response
                .iter()
                .filter_map(|x| match x {
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
    tun: &SharedTun,
    dev: &SharedDevice,
    _routing: Option<Arc<std::sync::RwLock<dyn std::any::Any + Send + Sync>>>,
) -> Result<()> {
    let future_vec = messages
        .iter()
        .map(|reply| async move {
            match reply {
                ReplyType::TapFlat(buf) => {
                    let vec = [IoSlice::new(buf)];
                    let _ = tun
                        .send_vectored(&vec)
                        .await
                        .inspect_err(|e| tracing::error!(?e, "error sending to tap"));
                }
                ReplyType::WireFlat(buf) => {
                    let vec = [IoSlice::new(buf)];
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

fn buffer() -> [u8; PACKET_BUFFER_SIZE] {
    let buf = uninit_array![u8; PACKET_BUFFER_SIZE];
    unsafe { std::mem::transmute::<_, [u8; PACKET_BUFFER_SIZE]>(buf) }
}

pub async fn wire_traffic<Fut>(
    dev: &SharedDevice,
    callable: impl FnOnce([u8; PACKET_BUFFER_SIZE], usize) -> Fut,
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
    dev: &SharedTun,
    callable: impl FnOnce([u8; PACKET_BUFFER_SIZE], usize) -> Fut,
) -> Result<Option<Vec<ReplyType>>>
where
    Fut: Future<Output = Result<Option<Vec<ReplyType>>>>,
{
    let mut buf = buffer();
    let n = dev.recv(&mut buf).await?;
    callable(buf, n).await
}

/// Batch send multiple packets to device with vectored I/O
///
/// This function batches multiple packets and sends them in a single system call
/// using writev, reducing syscall overhead and improving throughput.
pub async fn batch_send_wire(dev: &Arc<Device>, packets: &[Vec<u8>]) -> Result<usize> {
    if packets.is_empty() {
        return Ok(0);
    }

    let slices: Vec<IoSlice> = packets.iter().map(|p| IoSlice::new(p)).collect();
    let total = dev.send_vectored(&slices).await?;
    Ok(total)
}

/// Batch send multiple packets to TAP device with vectored I/O
pub async fn batch_send_tap(tun: &Arc<Tun>, packets: &[Vec<u8>]) -> Result<usize> {
    if packets.is_empty() {
        return Ok(0);
    }

    let slices: Vec<IoSlice> = packets.iter().map(|p| IoSlice::new(p)).collect();
    let total = tun.send_vectored(&slices).await?;
    Ok(total)
}

/// Process and send a batch of replies efficiently
///
/// This function groups replies by type (Wire/Tap) and sends them in batches,
/// reducing the number of system calls and improving throughput.
pub async fn handle_messages_batched(
    messages: Vec<ReplyType>,
    tun: &Arc<Tun>,
    dev: &Arc<Device>,
) -> Result<()> {
    // Separate wire and tap packets
    let mut wire_packets = Vec::new();
    let mut tap_packets = Vec::new();

    for reply in messages {
        match reply {
            ReplyType::WireFlat(buf) => wire_packets.push(buf),
            ReplyType::TapFlat(buf) => tap_packets.push(buf),
        }
    }

    // Send batches concurrently
    let wire_future = async {
        if !wire_packets.is_empty() {
            batch_send_wire(dev, &wire_packets).await.inspect_err(|e| {
                tracing::error!(
                    ?e,
                    count = wire_packets.len(),
                    "error batch sending to wire"
                )
            })
        } else {
            Ok(0)
        }
    };

    let tap_future = async {
        if !tap_packets.is_empty() {
            batch_send_tap(tun, &tap_packets).await.inspect_err(|e| {
                tracing::error!(?e, count = tap_packets.len(), "error batch sending to tap")
            })
        } else {
            Ok(0)
        }
    };

    let (wire_result, tap_result) = tokio::join!(wire_future, tap_future);

    wire_result?;
    tap_result?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{get_msgs, DebugReplyType, ReplyType};
    use crate::messages::message::Message;
    use anyhow::Result;

    #[test]
    fn get_msgs_ok_none() {
        let res: Result<Option<Vec<ReplyType>>> = Ok(None);
        let out = get_msgs(&res).expect("ok none");
        assert!(out.is_none());
    }

    #[test]
    fn get_msgs_ok_some_with_unparsable_wire() {
        // ReplyType::WireFlat with random bytes that won't parse to Message -> filtered out
        let replies = vec![ReplyType::WireFlat(vec![0u8; 3])];
        let res: Result<Option<Vec<ReplyType>>> = Ok(Some(replies));
        let dbg = get_msgs(&res).expect("ok some").expect("some");
        // should filter out unparsable wire entries
        assert!(dbg.is_empty());
    }

    #[test]
    fn get_msgs_ok_some_with_parsable_wire() {
        use crate::messages::data::Data;
        use crate::messages::data::ToUpstream;
        use crate::messages::packet_type::PacketType;
        use mac_address::MacAddress;

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
            _ => panic!("expected WireFlat debug string"),
        }
    }
}
