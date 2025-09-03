use crate::messages::message::Message;
use anyhow::{bail, Result};
use common::device::Device;
use common::tun::Tun;
use futures::{future::join_all, Future};
use itertools::Itertools;
use std::{io::IoSlice, sync::Arc};

#[derive(Debug)]
pub enum ReplyType {
    Wire(Vec<Vec<u8>>),
    Tap(Vec<Vec<u8>>),
    // Zero-copy scatter-gather variants. Prefer these for performance.
    WireParts(Vec<BufPart>),
    TapParts(Vec<BufPart>),
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
                    ReplyType::TapParts(parts) => {
                        // For debug only: flatten parts into a Vec<u8>
                        let flat: Vec<u8> = flatten_parts(parts);
                        // Represent as hex string for tap debug as well
                        let s = bytes_to_hex(&flat);
                        Some(DebugReplyType::Wire(format!("tap parts: {s}")))
                    }
                    ReplyType::Wire(x) => {
                        let x = x.iter().flat_map(|x| x.iter()).copied().collect_vec();
                        let Ok(message) = Message::try_from(&x[..]) else {
                            return None;
                        };
                        Some(DebugReplyType::Wire(format!("{message:?}")))
                    }
                    ReplyType::WireParts(parts) => {
                        let flat: Vec<u8> = flatten_parts(parts);
                        let Ok(message) = Message::try_from(&flat[..]) else {
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

/// A zero-copy part of a scatter-gather buffer to transmit.
#[derive(Debug, Clone)]
pub enum BufPart {
    Owned(Vec<u8>),
    ArcSlice { data: Arc<[u8]>, offset: usize, len: usize },
}

fn flatten_parts(parts: &[BufPart]) -> Vec<u8> {
    let mut out = Vec::new();
    for p in parts {
        match p {
            BufPart::Owned(v) => out.extend_from_slice(v),
            BufPart::ArcSlice { data, offset, len } => {
                let start = *offset;
                let end = start + *len;
                out.extend_from_slice(&data[start..end]);
            }
        }
    }
    out
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
                ReplyType::TapParts(parts) => {
                    // Build IoSlices that borrow from either owned Vecs or Arc slices.
                    // Keep local owned Vecs and cloned Arcs alive until await completes.
                    let mut backing_vecs: Vec<&[u8]> = Vec::with_capacity(parts.len());
                    for p in parts {
                        match p {
                            BufPart::Owned(v) => backing_vecs.push(v.as_slice()),
                            BufPart::ArcSlice { data, offset, len } => {
                                let start = *offset;
                                let end = start + *len;
                                backing_vecs.push(&data[start..end]);
                            }
                        }
                    }
                    let ios: Vec<IoSlice> = backing_vecs.iter().map(|s| IoSlice::new(s)).collect();
                    let _ = tun
                        .send_vectored(&ios)
                        .await
                        .inspect_err(|e| tracing::error!(?e, "error sending to tap(parts)"));
                }
                ReplyType::Wire(reply) => {
                    let vec: Vec<IoSlice> = reply.iter().map(|x| IoSlice::new(x)).collect();
                    let _ = dev
                        .send_vectored(&vec)
                        .await
                        .inspect_err(|e| tracing::error!(?e, "error sending to dev"));
                }
                ReplyType::WireParts(parts) => {
                    let mut backing_vecs: Vec<&[u8]> = Vec::with_capacity(parts.len());
                    for p in parts {
                        match p {
                            BufPart::Owned(v) => backing_vecs.push(v.as_slice()),
                            BufPart::ArcSlice { data, offset, len } => {
                                let start = *offset;
                                let end = start + *len;
                                backing_vecs.push(&data[start..end]);
                            }
                        }
                    }
                    let ios: Vec<IoSlice> = backing_vecs.iter().map(|s| IoSlice::new(s)).collect();
                    let _ = dev
                        .send_vectored(&ios)
                        .await
                        .inspect_err(|e| tracing::error!(?e, "error sending to dev(parts)"));
                }
            };
        })
        .collect_vec();

    join_all(future_vec).await;
    Ok(())
}

pub async fn wire_traffic<Fut>(
    dev: &Arc<Device>,
    callable: impl FnOnce(Arc<[u8]>, usize) -> Fut,
) -> Result<Option<Vec<ReplyType>>>
where
    Fut: Future<Output = Result<Option<Vec<ReplyType>>>>,
{
    // Allocate heap buffer to allow zero-copy referencing downstream
    let mut v = vec![0u8; 1500];
    let n = dev.recv(&mut v).await?;
    v.truncate(n);
    let arc: Arc<[u8]> = v.into_boxed_slice().into();
    // Also emit a debug so test output can capture the raw bytes when tracing
    tracing::trace!(n = n, raw = %bytes_to_hex(&arc[..]), "wire_traffic recv");
    callable(arc, n).await
}

pub async fn tap_traffic<Fut>(
    dev: &Arc<Tun>,
    callable: impl FnOnce(Arc<[u8]>, usize) -> Fut,
) -> Result<Option<Vec<ReplyType>>>
where
    Fut: Future<Output = Result<Option<Vec<ReplyType>>>>,
{
    let mut v = vec![0u8; 1500];
    let n = dev.recv(&mut v).await?;
    v.truncate(n);
    let arc: Arc<[u8]> = v.into_boxed_slice().into();
    callable(arc, n).await
}

#[cfg(test)]
mod tests {
    use super::{get_msgs, BufPart, ReplyType};
    use crate::control::node::DebugReplyType;
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
        // ReplyType::Wire with random bytes that won't parse to Message -> filtered out
        let replies = vec![ReplyType::Wire(vec![vec![0u8; 3]])];
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

        let wire: Vec<Vec<u8>> = (&message).into();
        let replies = vec![ReplyType::Wire(wire)];
        let res: Result<Option<Vec<ReplyType>>> = Ok(Some(replies));
        let dbg = get_msgs(&res).expect("ok some").expect("some");
        // should contain one Wire debug entry
        assert_eq!(dbg.len(), 1);
        match &dbg[0] {
            DebugReplyType::Wire(s) => {
                assert!(s.contains("Message"));
            }
            _ => panic!("expected Wire debug string"),
        }
    }

    #[test]
    fn get_msgs_with_tap_parts_debug_string() {
        // Build a TapParts reply and ensure get_msgs returns a debug string entry
        let parts = vec![
            BufPart::Owned(b"aa".to_vec()),
            BufPart::Owned(b"bb".to_vec()),
        ];
        let replies = vec![ReplyType::TapParts(parts)];
        let res: Result<Option<Vec<ReplyType>>> = Ok(Some(replies));
        let dbg = get_msgs(&res).expect("ok some").expect("some");
        assert_eq!(dbg.len(), 1);
        match &dbg[0] {
            DebugReplyType::Wire(s) => assert!(s.contains("tap parts")),
            _ => panic!("expected Wire debug for tap parts"),
        }
    }

    #[tokio::test]
    async fn wire_traffic_invokes_closure_with_arc() {
        use common::device::{Device, DeviceIo};
        use std::os::unix::io::FromRawFd;
        use tokio::io::unix::AsyncFd;

        // create pipe: reader for Device::recv, writer to feed data
        let mut fds = [0; 2];
        unsafe { libc::pipe(fds.as_mut_ptr()) };
        let reader_fd = fds[0];
        let writer_fd = fds[1];

        // set reader non-blocking
        unsafe {
            let flags = libc::fcntl(reader_fd, libc::F_GETFL);
            if flags >= 0 { let _ = libc::fcntl(reader_fd, libc::F_SETFL, flags | libc::O_NONBLOCK); }
        }

        let dev = std::sync::Arc::new(Device::from_asyncfd_for_bench(
            [2u8,2,2,2,2,2].into(),
            AsyncFd::new(unsafe { DeviceIo::from_raw_fd(reader_fd) }).unwrap(),
        ));

        // write payload
        let payload = b"wire-traffic";
        let w = unsafe { libc::write(writer_fd, payload.as_ptr().cast(), payload.len()) };
        assert_eq!(w, payload.len() as isize);

        // capture arc in closure
        let res = super::wire_traffic(&dev, |arc, n| async move {
            assert_eq!(n, payload.len());
            assert_eq!(&arc[..], payload);
            Ok::<_, anyhow::Error>(None)
        })
        .await
        .expect("ok");
        assert!(res.is_none());

        unsafe { libc::close(writer_fd) };
    }

    #[tokio::test]
    async fn tap_traffic_invokes_closure_with_arc() {
        use common::tun::{test_tun::TokioTun, Tun};

        let (a, b) = TokioTun::new_pair();
        let tun = std::sync::Arc::new(Tun::new_shim(a));

        // send from peer
        let payload = b"tap-traffic".to_vec();
        let expected = payload.clone();
        let sender = tokio::spawn(async move {
            b.send_all(&payload).await.expect("peer send");
        });

        let res = super::tap_traffic(&tun, |arc, n| async move {
            assert_eq!(n, expected.len());
            assert_eq!(&arc[..], &expected);
            Ok::<_, anyhow::Error>(None)
        })
        .await
        .expect("ok");
        assert!(res.is_none());

        sender.await.expect("join");
    }

    #[tokio::test]
    async fn handle_messages_tap_parts_sends() {
        use common::tun::{test_tun::TokioTun, Tun};

        let (a, b) = TokioTun::new_pair();
        let tun_a = std::sync::Arc::new(Tun::new_shim(a));
        let tun_b = b; // receiver side

        // Prepare TapParts buffers
        let parts = vec![
            BufPart::Owned(b"tap ".to_vec()),
            BufPart::Owned(b"parts".to_vec()),
        ];
        let replies = vec![ReplyType::TapParts(parts)];

        // Dummy device (unused in this test)
        use common::device::{Device, DeviceIo};
        use tokio::io::unix::AsyncFd;
        use std::os::unix::io::FromRawFd;
        let mut fds = [0; 2];
        unsafe { libc::pipe(fds.as_mut_ptr()) };
        let writer_fd = fds[1];
        // writer end for device (won't be used by TapParts)
        let device = std::sync::Arc::new(Device::from_asyncfd_for_bench(
            [1u8,2,3,4,5,6].into(),
            AsyncFd::new(unsafe { DeviceIo::from_raw_fd(writer_fd) }).unwrap(),
        ));

        super::handle_messages(replies, &tun_a, &device)
            .await
            .expect("handle_messages ok");

        // Receive on peer and validate payload
        let mut buf = vec![0u8; 64];
        let n = tun_b.recv(&mut buf).await.expect("recv");
        assert_eq!(&buf[..n], b"tap parts");

        // close unused reader end
        unsafe { libc::close(fds[0]) };
    }

    #[tokio::test]
    async fn handle_messages_wire_parts_sends() {
        use common::device::{Device, DeviceIo};
        use tokio::io::unix::AsyncFd;
        use std::os::unix::io::FromRawFd;

        // Pipe for device: reader will verify payload, writer used by Device
        let mut fds = [0; 2];
        unsafe { libc::pipe(fds.as_mut_ptr()) };
        let reader_fd = fds[0];
        let writer_fd = fds[1];

        // Set writer non-blocking for AsyncFd
        unsafe {
            let flags = libc::fcntl(writer_fd, libc::F_GETFL);
            if flags >= 0 { let _ = libc::fcntl(writer_fd, libc::F_SETFL, flags | libc::O_NONBLOCK); }
        }

        let device = std::sync::Arc::new(Device::from_asyncfd_for_bench(
            [6u8,5,4,3,2,1].into(),
            AsyncFd::new(unsafe { DeviceIo::from_raw_fd(writer_fd) }).unwrap(),
        ));

        // Tun is unused for WireParts but pass a shim
        use common::tun::{test_tun::TokioTun, Tun};
        let (a, _b) = TokioTun::new_pair();
        let tun = std::sync::Arc::new(Tun::new_shim(a));

        // Prepare WireParts buffers
        let parts = vec![
            BufPart::Owned(b"wire ".to_vec()),
            BufPart::Owned(b"parts".to_vec()),
        ];
        let replies = vec![ReplyType::WireParts(parts)];

        super::handle_messages(replies, &tun, &device)
            .await
            .expect("handle_messages ok");

        // Read from reader end and verify
        let mut out = vec![0u8; 64];
        let n = unsafe { libc::read(reader_fd, out.as_mut_ptr().cast(), out.len()) };
        assert!(n > 0);
        let n = n as usize;
        assert_eq!(&out[..n], b"wire parts");

        unsafe { libc::close(reader_fd) };
    }
}
