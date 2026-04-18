use bytes::Bytes;
use common::device::Device;
use libp2p::core::transport::{DialOpts, ListenerId, TransportError, TransportEvent};
use libp2p::{Multiaddr, Transport};
use mac_address::MacAddress;
use std::{
    collections::{HashMap, VecDeque},
    future::Ready,
    io,
    pin::Pin,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc, Mutex,
    },
    task::{Context, Poll, Waker},
};
use tokio::sync::mpsc;

use crate::{
    connection::L2Connection,
    framing,
    multiaddr_ext::{mac_to_multiaddr, multiaddr_to_mac},
};

const CHANNEL_CAP: usize = 64;
const DEVICE_BUF: usize = 9000;

type PendingEvents = Arc<
    Mutex<
        VecDeque<TransportEvent<Ready<Result<L2Connection, L2TransportError>>, L2TransportError>>,
    >,
>;

#[derive(Debug, thiserror::Error)]
pub enum L2TransportError {
    #[error("unsupported multiaddr: {0}")]
    MultiaddrNotSupported(Multiaddr),
    #[error("io error: {0}")]
    Io(#[from] io::Error),
}

/// Notification sent from the demux task to the Transport when a new inbound
/// connection arrives (unknown conn_id seen on the wire).
struct InboundConn {
    conn: L2Connection,
    remote_mac: MacAddress,
}

/// State shared between the Transport and the background demux task.
struct Shared {
    /// Map from conn_id to the inbound channel for that connection.
    conn_map: HashMap<u32, mpsc::Sender<Bytes>>,
    /// Next conn_id to assign for inbound connections created by the demux task.
    next_inbound_id: u32,
    /// Waker to wake the Transport::poll when new events arrive.
    waker: Option<Waker>,
}

/// A libp2p `Transport` that sends/receives framed data over a raw L2 `Device`.
///
/// Each logical connection is identified by a `conn_id` embedded in every
/// frame.  The dialer allocates conn_ids (via `next_outbound_id`); the
/// listener assigns its own IDs to frames with unknown IDs.
///
/// Frame wire format (prepended to every payload):
/// ```text
/// MAGIC[2] | conn_id[4 BE] | length[2 BE] | payload[length]
/// ```
pub struct L2Transport {
    device: Arc<Device>,
    local_mac: MacAddress,
    shared: Arc<Mutex<Shared>>,
    next_outbound_id: Arc<AtomicU32>,
    /// Events queued by the demux task, drained by `poll`.
    pending: PendingEvents,
    listener_id: Option<ListenerId>,
}

impl L2Transport {
    /// Create a new transport and spawn the background demux task.
    pub fn new(device: Arc<Device>, local_mac: MacAddress) -> Self {
        let shared = Arc::new(Mutex::new(Shared {
            conn_map: HashMap::new(),
            next_inbound_id: u32::MAX / 2, // inbound IDs start in upper half
            waker: None,
        }));
        let pending = Arc::new(Mutex::new(VecDeque::new()));
        let next_outbound_id = Arc::new(AtomicU32::new(0));

        let transport = Self {
            device: device.clone(),
            local_mac,
            shared: shared.clone(),
            next_outbound_id,
            pending: pending.clone(),
            listener_id: None,
        };

        // Spawn the demux task.
        tokio::spawn(demux_loop(device, local_mac, shared, pending));

        transport
    }
}

/// Background task: reads raw frames from the Device and routes them to the
/// appropriate `L2Connection` channel, or creates a new inbound connection.
async fn demux_loop(
    device: Arc<Device>,
    local_mac: MacAddress,
    shared: Arc<Mutex<Shared>>,
    pending: PendingEvents,
) {
    let mut buf = vec![0u8; DEVICE_BUF];
    loop {
        let n = match device.recv(&mut buf).await {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!(error = %e, "L2Transport demux recv error");
                continue;
            }
        };

        let data = &buf[..n];
        let Some((conn_id, payload, _)) = framing::decode_frame(data) else {
            // Not one of our frames (e.g. existing VANET protocol traffic).
            continue;
        };

        let payload = Bytes::copy_from_slice(payload);

        // Try to route to an existing connection.
        let sender = shared.lock().unwrap().conn_map.get(&conn_id).cloned();

        if let Some(tx) = sender {
            let _ = tx.send(payload).await;
            continue;
        }

        // Unknown conn_id → new inbound connection.
        let (tx, rx) = mpsc::channel(CHANNEL_CAP);
        let inbound_id = {
            let mut s = shared.lock().unwrap();
            let id = s.next_inbound_id;
            s.next_inbound_id = s.next_inbound_id.wrapping_add(1);
            s.conn_map.insert(id, tx.clone());
            id
        };

        // Deliver the first payload.
        let _ = tx.send(payload).await;

        // We don't know the remote MAC from the frame alone (the VANET header
        // is stripped; only the L2Transport frame is visible here).  Use a
        // zero MAC as placeholder — callers use PeerId for identity anyway.
        let remote_mac: MacAddress = [0u8; 6].into();

        let conn = L2Connection::new(local_mac, remote_mac, inbound_id, rx, device.clone());
        let inbound = InboundConn { conn, remote_mac };

        // Queue a TransportEvent::Incoming for poll() to emit.
        let event = TransportEvent::Incoming {
            listener_id: ListenerId::next(),
            upgrade: std::future::ready(Ok(inbound.conn)),
            local_addr: mac_to_multiaddr(local_mac),
            send_back_addr: mac_to_multiaddr(inbound.remote_mac),
        };

        {
            let mut p = pending.lock().unwrap();
            p.push_back(event);
        }

        // Wake the Transport::poll waker if registered.
        if let Some(waker) = shared.lock().unwrap().waker.take() {
            waker.wake();
        }
    }
}

impl Transport for L2Transport {
    type Output = L2Connection;
    type Error = L2TransportError;
    type ListenerUpgrade = Ready<Result<L2Connection, L2TransportError>>;
    type Dial =
        Pin<Box<dyn std::future::Future<Output = Result<L2Connection, L2TransportError>> + Send>>;

    fn listen_on(
        &mut self,
        id: ListenerId,
        addr: Multiaddr,
    ) -> Result<(), TransportError<Self::Error>> {
        if multiaddr_to_mac(&addr).is_none() {
            return Err(TransportError::MultiaddrNotSupported(addr));
        }
        self.listener_id = Some(id);
        // Emit NewAddress so the Swarm registers the listen address.
        self.pending
            .lock()
            .unwrap()
            .push_back(TransportEvent::NewAddress {
                listener_id: id,
                listen_addr: mac_to_multiaddr(self.local_mac),
            });
        Ok(())
    }

    fn remove_listener(&mut self, id: ListenerId) -> bool {
        if self.listener_id == Some(id) {
            self.listener_id = None;
            true
        } else {
            false
        }
    }

    fn dial(
        &mut self,
        addr: Multiaddr,
        _opts: DialOpts,
    ) -> Result<Self::Dial, TransportError<Self::Error>> {
        let remote_mac =
            multiaddr_to_mac(&addr).ok_or(TransportError::MultiaddrNotSupported(addr))?;

        let conn_id = self.next_outbound_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel(CHANNEL_CAP);

        self.shared.lock().unwrap().conn_map.insert(conn_id, tx);

        let conn = L2Connection::new(self.local_mac, remote_mac, conn_id, rx, self.device.clone());

        Ok(Box::pin(async move { Ok(conn) }))
    }

    fn poll(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<TransportEvent<Self::ListenerUpgrade, Self::Error>> {
        let this = self.get_mut();
        if let Some(event) = this.pending.lock().unwrap().pop_front() {
            return Poll::Ready(event);
        }
        // Register waker for when the demux task produces new events.
        this.shared.lock().unwrap().waker = Some(cx.waker().clone());
        Poll::Pending
    }
}
