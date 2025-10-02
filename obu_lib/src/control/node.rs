use super::routing::Routing;
use anyhow::Result;
use common::device::Device;
use common::tun::Tun;
use futures::{future::join_all, Future};
use itertools::Itertools;
use node_lib::messages::message::Message;
use node_lib::{SharedDevice, SharedTun, PACKET_BUFFER_SIZE};
use std::{io::IoSlice, sync::Arc};
use uninit::uninit_array;

// Re-export shared ReplyType from node_lib
pub use node_lib::control::node::ReplyType;

// Debug types and functions for tracing and testing
#[cfg(any(test, feature = "test_helpers"))]
#[derive(Debug)]
pub enum DebugReplyType {
    Tap(Vec<Vec<u8>>),
    Wire(String),
}

#[cfg(any(test, feature = "test_helpers"))]
pub fn get_msgs(response: &Result<Option<Vec<ReplyType>>>) -> Result<Option<Vec<DebugReplyType>>> {
    use anyhow::bail;
    use itertools::Itertools;
    use node_lib::messages::message::Message;

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

// Re-export shared helper function from node_lib
pub use node_lib::control::node::bytes_to_hex;

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
                    ReplyType::TapFlat(buf) => {
                        let vec = [IoSlice::new(buf)];
                        let _ = tun
                            .send_vectored(&vec)
                            .await
                            .inspect_err(|e| tracing::error!(error = %e, size = buf.len(), "Failed to send to TAP device"));
                    }
                    ReplyType::WireFlat(buf) => {
                        let vec = [IoSlice::new(buf)];
                        let send_res = dev.send_vectored(&vec).await;
                        if let Err(e) = send_res {
                            tracing::error!(error = %e, size = buf.len(), "Failed to send to device");

                            // Failover logic for flat buffers
                            if let Some(r) = &routing_clone {
                                if let Ok(parsed) = Message::try_from(&buf[..]) {
                                    if let Ok(dest) = parsed.to() {
                                        let cached = match r.read() {
                                            Ok(guard) => guard.get_cached_upstream(),
                                            Err(poisoned) => {
                                                tracing::error!("Routing lock poisoned during failover check, recovering");
                                                poisoned.into_inner().get_cached_upstream()
                                            }
                                        };
                                        if let Some(cached_mac) = cached {
                                            if cached_mac == dest {
                                                let promoted = match r.write() {
                                                    Ok(guard) => guard.failover_cached_upstream(),
                                                    Err(poisoned) => {
                                                        tracing::error!("Routing write lock poisoned during failover, recovering");
                                                        poisoned.into_inner().failover_cached_upstream()
                                                    }
                                                };
                                                if let Some(new_upstream) = promoted {
                                                    tracing::info!(
                                                        new_upstream = %new_upstream,
                                                        "Promoted next cached upstream after send failure"
                                                    );
                                                }
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

/// Process and send a batch of replies efficiently using vectored I/O
///
/// This function groups replies by type (Wire/Tap) and sends them in batches,
/// reducing the number of system calls and improving throughput by 2-3x.
///
/// If routing is provided, failed wire sends targeting the cached upstream
/// will trigger failover to the next candidate.
pub async fn handle_messages_batched(
    messages: Vec<ReplyType>,
    tun: &Arc<Tun>,
    dev: &Arc<Device>,
    routing: Option<Arc<std::sync::RwLock<Routing>>>,
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

    // Send batches concurrently using vectored I/O
    let wire_future = async {
        if !wire_packets.is_empty() {
            let slices: Vec<IoSlice> = wire_packets.iter().map(|p| IoSlice::new(p)).collect();
            let total_bytes: usize = wire_packets.iter().map(|p| p.len()).sum();
            let send_res = dev.send_vectored(&slices).await;

            if let Err(ref e) = send_res {
                tracing::error!(
                    error = %e,
                    packet_count = wire_packets.len(),
                    total_bytes = total_bytes,
                    "Failed to batch send to device"
                );

                // On batch send error, try to trigger failover if routing is present
                // and any packet in the batch targets the cached upstream.
                if let Some(ref r) = routing {
                    for packet in &wire_packets {
                        if let Ok(parsed) = Message::try_from(&packet[..]) {
                            if let Ok(dest) = parsed.to() {
                                let cached = match r.read() {
                                    Ok(guard) => guard.get_cached_upstream(),
                                    Err(poisoned) => {
                                        tracing::error!(
                                            "Routing read lock poisoned during failover check"
                                        );
                                        poisoned.into_inner().get_cached_upstream()
                                    }
                                };
                                if let Some(cached_mac) = cached {
                                    if cached_mac == dest {
                                        let promoted = match r.write() {
                                            Ok(guard) => guard.failover_cached_upstream(),
                                            Err(poisoned) => {
                                                tracing::error!(
                                                    "Routing write lock poisoned during failover"
                                                );
                                                poisoned.into_inner().failover_cached_upstream()
                                            }
                                        };
                                        if let Some(new_upstream) = promoted {
                                            tracing::info!(
                                                new_upstream = %new_upstream,
                                                "Promoted next cached upstream after send failure"
                                            );
                                        }
                                        // Only trigger failover once per batch
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
            }

            send_res
        } else {
            Ok(0)
        }
    };

    let tap_future = async {
        if !tap_packets.is_empty() {
            let slices: Vec<IoSlice> = tap_packets.iter().map(|p| IoSlice::new(p)).collect();
            let total_bytes: usize = tap_packets.iter().map(|p| p.len()).sum();
            tun.send_vectored(&slices).await.inspect_err(|e| {
                tracing::error!(
                    error = %e,
                    packet_count = tap_packets.len(),
                    total_bytes = total_bytes,
                    "Failed to batch send to TAP device"
                )
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

#[cfg(test)]
mod tests {
    use super::{get_msgs, DebugReplyType, ReplyType};
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
        // ReplyType::WireFlat with random bytes that won't parse to Message -> filtered out
        let replies = vec![ReplyType::WireFlat(vec![0u8; 3])];
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
