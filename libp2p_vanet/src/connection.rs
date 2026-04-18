use bytes::{Bytes, BytesMut};
use common::device::Device;
use mac_address::MacAddress;
use std::{
    future::Future,
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    sync::mpsc,
};

use crate::framing;

/// A logical connection between two MAC-addressed nodes over a shared L2 Device.
///
/// Reads are fed from an MPSC channel populated by `L2Transport`'s demux task.
/// Writes are framed and sent through the shared `Device`.
pub struct L2Connection {
    pub local_mac: MacAddress,
    pub remote_mac: MacAddress,
    pub conn_id: u32,
    inbound_rx: mpsc::Receiver<Bytes>,
    device: Arc<Device>,
    read_buf: BytesMut,
    pending_write: Option<Pin<Box<dyn Future<Output = io::Result<()>> + Send>>>,
}

impl L2Connection {
    pub fn new(
        local_mac: MacAddress,
        remote_mac: MacAddress,
        conn_id: u32,
        inbound_rx: mpsc::Receiver<Bytes>,
        device: Arc<Device>,
    ) -> Self {
        Self {
            local_mac,
            remote_mac,
            conn_id,
            inbound_rx,
            device,
            read_buf: BytesMut::new(),
            pending_write: None,
        }
    }
}

impl AsyncRead for L2Connection {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if !self.read_buf.is_empty() {
            let to_copy = buf.remaining().min(self.read_buf.len());
            let data = self.read_buf.split_to(to_copy);
            buf.put_slice(&data);
            return Poll::Ready(Ok(()));
        }
        match self.inbound_rx.poll_recv(cx) {
            Poll::Ready(Some(bytes)) => {
                let to_copy = buf.remaining().min(bytes.len());
                buf.put_slice(&bytes[..to_copy]);
                if to_copy < bytes.len() {
                    self.read_buf.extend_from_slice(&bytes[to_copy..]);
                }
                Poll::Ready(Ok(()))
            }
            Poll::Ready(None) => Poll::Ready(Ok(())),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl AsyncWrite for L2Connection {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        // Poll any in-flight write to completion before accepting new data.
        if let Some(fut) = self.pending_write.as_mut() {
            match fut.as_mut().poll(cx) {
                Poll::Ready(Ok(())) => {
                    self.pending_write = None;
                }
                Poll::Ready(Err(e)) => {
                    self.pending_write = None;
                    return Poll::Ready(Err(e));
                }
                Poll::Pending => return Poll::Pending,
            }
        }

        let frame = framing::encode_frame(self.conn_id, buf);
        let device = self.device.clone();
        let n = buf.len();
        let mut fut: Pin<Box<dyn Future<Output = io::Result<()>> + Send>> =
            Box::pin(async move { device.send_all(&frame).await });

        match fut.as_mut().poll(cx) {
            Poll::Ready(Ok(())) => Poll::Ready(Ok(n)),
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => {
                self.pending_write = Some(fut);
                Poll::Pending
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
