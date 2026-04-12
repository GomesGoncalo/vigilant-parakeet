//! Network channel simulation with configurable latency, jitter, and packet loss
//!
//! This module provides a Channel abstraction that simulates network conditions between nodes,
//! including latency delays, jitter variation, and probabilistic packet loss.

use anyhow::Result;
use common::channel_parameters::ChannelParameters;
use common::tun::Tun;
use mac_address::MacAddress;
use node_lib::PACKET_BUFFER_SIZE;
use std::{
    sync::{Arc, RwLock},
    time::Duration,
};
use tokio::sync::{mpsc, Notify};
use tokio::time::Instant;

/// Channel send error types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelError {
    /// Packet was filtered due to MAC address mismatch (normal operation)
    Filtered,
    /// Packet was dropped due to simulated packet loss
    Dropped,
}

#[cfg(any(test, feature = "webview"))]
use anyhow::Context;
#[cfg(any(test, feature = "webview"))]
use std::{collections::HashMap, str::FromStr};

/// Packet with timing information for latency simulation
struct Packet {
    packet: [u8; PACKET_BUFFER_SIZE],
    size: usize,
    instant: Instant,
}

/// Maximum number of packets buffered per channel before congestion-dropping.
/// Prevents unbounded memory growth when latency is high relative to send rate.
/// Kept at 32 (one tokio mpsc block) to minimise block allocations: each Packet
/// is ~9 KB so 256 depth would require 8 × 288 KB blocks ≈ 2.3 MB per channel.
const CHANNEL_QUEUE_DEPTH: usize = 32;

/// Simulated network channel between two nodes
///
/// Provides configurable network conditions:
/// - **Latency**: Base delay for packet transmission
/// - **Jitter**: Random variation in latency (±jitter)
/// - **Loss**: Probabilistic packet drop rate (0.0 to 1.0)
///
/// The channel filters packets by MAC address and applies the configured
/// network conditions before forwarding to the destination interface.
///
/// ## Task lifetime
///
/// `Channel::new` spawns one background task.  The task holds independent
/// `Arc` clones of only the fields it needs (`parameters`, `tun`,
/// `param_notify`) — it does **not** hold `Arc<Channel>`.  This means the
/// task exits as soon as the last external `Arc<Channel>` is dropped: when
/// that happens `tx` is freed, `rx.recv()` returns `None`, and the task
/// terminates cleanly.  Without this design, dynamically-removed channels
/// (e.g. from range-based pruning) would leak one zombie task per removal.
pub struct Channel {
    tx: mpsc::Sender<Packet>,
    /// Coalescing notifier for parameter changes. Uses `Notify` (not a channel)
    /// so multiple rapid updates from the fading/mobility task are collapsed into
    /// one wakeup — preventing unbounded memory growth in the notification buffer.
    #[cfg_attr(
        not(any(test, feature = "webview", feature = "mobility")),
        allow(dead_code)
    )]
    param_notify: Arc<Notify>,
    /// Shared with the background forwarding task so parameters can be updated
    /// without keeping the full `Arc<Channel>` alive inside the task.
    parameters: Arc<RwLock<ChannelParameters>>,
    mac: MacAddress,
    tun: Arc<Tun>,
    /// Source node name
    from: String,
    /// Destination node name
    to: String,
}

impl Channel {
    /// Get current channel parameters
    pub fn params(&self) -> ChannelParameters {
        *self
            .parameters
            .read()
            .expect("channel parameters lock poisoned")
    }

    /// Get source node name
    pub fn from(&self) -> &str {
        &self.from
    }

    /// Get destination node name
    pub fn to(&self) -> &str {
        &self.to
    }

    /// Update only the loss field without changing latency or jitter.
    /// Used by the Nakagami-m fading task to update loss continuously.
    #[cfg(feature = "mobility")]
    #[allow(dead_code)]
    pub fn set_loss(&self, loss: f64) {
        let mut inner = self
            .parameters
            .write()
            .expect("channel parameters lock poisoned");
        inner.loss = loss.clamp(0.0, 1.0);
        self.param_notify.notify_one();
    }

    /// Update loss and latency atomically.
    /// Used by the Nakagami-m fading task so routing prefers nearby RSUs.
    #[cfg(feature = "mobility")]
    pub fn set_fading_params(&self, loss: f64, latency_ms: u64) {
        let mut inner = self
            .parameters
            .write()
            .expect("channel parameters lock poisoned");
        inner.loss = loss.clamp(0.0, 1.0);
        inner.latency = std::time::Duration::from_millis(latency_ms);
        self.param_notify.notify_one();
    }

    /// Update channel parameters dynamically
    ///
    /// Accepts a map of parameter names to string values:
    /// - `latency`: Latency in milliseconds
    /// - `loss`: Packet loss rate (0.0 to 1.0)
    /// - `jitter`: Jitter in milliseconds (optional, defaults to 0)
    ///
    /// Changes take effect immediately for new packets.
    #[cfg(any(test, feature = "webview"))]
    pub fn set_params(&self, params: HashMap<String, String>) -> Result<()> {
        let result = ChannelParameters {
            latency: Duration::from_millis(
                (params.get("latency").context("could not get latency")?).parse::<u64>()?,
            ),
            loss: f64::from_str(params.get("loss").context("could not get loss")?)?,
            jitter: Duration::from_millis(
                params
                    .get("jitter")
                    .unwrap_or(&"0".to_string())
                    .parse::<u64>()?,
            ),
        };

        let mut inner_params = self
            .parameters
            .write()
            .expect("channel parameters lock poisoned");
        *inner_params = result;
        self.param_notify.notify_one();
        Ok(())
    }

    /// Create a new channel with specified parameters
    ///
    /// # Arguments
    ///
    /// * `parameters` - Network conditions (latency, loss, jitter)
    /// * `mac` - MAC address filter for this channel
    /// * `tun` - Destination network interface
    /// * `from` - Source node name (for logging)
    /// * `to` - Destination node name (for logging)
    ///
    /// The channel spawns a background task that processes packets with
    /// the configured latency and forwards them to the destination interface.
    pub fn new(
        parameters: ChannelParameters,
        mac: MacAddress,
        tun: Arc<Tun>,
        from: String,
        to: String,
    ) -> Arc<Self> {
        // Bounded packet queue: prevents unbounded memory growth when latency > 0.
        // Packets beyond CHANNEL_QUEUE_DEPTH are congestion-dropped (realistic behaviour).
        let (tx, mut rx): (mpsc::Sender<Packet>, mpsc::Receiver<Packet>) =
            mpsc::channel(CHANNEL_QUEUE_DEPTH);
        // Notify instead of a channel: multiple rapid param changes (e.g. fading task)
        // collapse into one pending wakeup, so the buffer never grows unboundedly.
        let param_notify = Arc::new(Notify::new());
        let notify_for_task = param_notify.clone();

        // Wrap parameters in Arc so the task can share it without holding Arc<Channel>.
        // IMPORTANT: the task must NOT hold Arc<Channel> — if it did, dropping the
        // last external Arc<Channel> would leave tx alive (inside the Arc<Channel>
        // kept by the task), so rx.recv() would never return None and the task would
        // leak permanently.  With independent Arc clones the task only keeps the fields
        // it actually uses; once all external owners drop Arc<Channel>, tx is freed,
        // rx.recv() returns None, and the task exits cleanly.
        let parameters: Arc<RwLock<ChannelParameters>> = Arc::new(parameters.into());
        let params_for_task = parameters.clone();
        let tun_for_task = tun.clone();
        let from_task = from.clone();
        let to_task = to.clone();

        tracing::debug!(target: "sim_channel", from = %from, to = %to, "Created channel");

        // Spawn task to process packets from channel with latency simulation.
        tokio::spawn(async move {
            loop {
                let Some(packet) = rx.recv().await else {
                    // tx was dropped (Arc<Channel> freed) — exit cleanly.
                    break;
                };

                tracing::trace!(target: "sim_channel", from = %from_task, to = %to_task, size = packet.size, "Dequeued packet for delivery");

                loop {
                    // Calculate latency with jitter in a separate scope to drop the lock.
                    let latency = {
                        let params = params_for_task
                            .read()
                            .expect("channel parameters lock poisoned");

                        // Apply base latency + random jitter
                        let mut latency = params.latency;
                        if !params.jitter.is_zero() {
                            // Generate random jitter in range [-jitter, +jitter]
                            let jitter_ms = params.jitter.as_millis() as i64;
                            let random_normalized = rand::random::<f64>(); // [0.0, 1.0)
                            let random_jitter =
                                ((random_normalized * 2.0 - 1.0) * jitter_ms as f64) as i64;
                            if random_jitter >= 0 {
                                latency += Duration::from_millis(random_jitter as u64);
                            } else {
                                // Subtract jitter, but don't go negative
                                let abs_jitter = (-random_jitter) as u64;
                                if latency > Duration::from_millis(abs_jitter) {
                                    latency -= Duration::from_millis(abs_jitter);
                                } else {
                                    latency = Duration::ZERO;
                                }
                            }
                        }
                        latency
                    }; // Lock is released here

                    let duration = (packet.instant + latency).duration_since(Instant::now());

                    if duration.is_zero() {
                        tracing::trace!(target: "sim_channel", from = %from_task, to = %to_task, size = packet.size, "Delivering packet immediately");
                        let _ = tun_for_task.send_all(&packet.packet[..packet.size]).await;
                        break;
                    } else {
                        tokio::select! {
                            _ = tokio_timerfd::sleep(duration) => {
                                tracing::trace!(target: "sim_channel", from = %from_task, to = %to_task, size = packet.size, latency = ?latency, "Delivering packet after latency");
                                let _ = tun_for_task.send_all(&packet.packet[..packet.size]).await;
                                break;
                            },
                            _ = notify_for_task.notified() => {
                                // Parameters changed, recalculate duration.
                            },
                        }
                    }
                }
            }
        });

        Arc::new(Self {
            tx,
            param_notify,
            parameters,
            mac,
            tun,
            from,
            to,
        })
    }

    /// Send a packet through this channel
    ///
    /// The packet is checked against the MAC address filter and loss rate
    /// before being queued for transmission with the configured latency.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Packet MAC address doesn't match (not broadcast or this channel's MAC) - returns ChannelError::Filtered
    /// - Packet is randomly dropped due to configured loss rate - returns ChannelError::Dropped
    /// - Channel send fails (should not happen with unbounded channel)
    pub async fn send(
        &self,
        packet: [u8; PACKET_BUFFER_SIZE],
        size: usize,
    ) -> Result<(), ChannelError> {
        match self.should_send(&packet[..size]) {
            Ok(()) => {
                tracing::trace!(target: "sim_channel", from = %self.from, to = %self.to, size, "Packet accepted for send");
            },
            Err(ChannelError::Filtered) => {
                // Print the first 32 bytes of the filtered packet as hex for debugging
                let hex: String = packet[..size.min(32)]
                    .iter()
                    .map(|b| format!("{:02x}", b))
                    .collect::<Vec<_>>()
                    .join(" ");
                tracing::debug!(
                    target: "sim_channel",
                    from = %self.from,
                    to = %self.to,
                    size,
                    packet_hex = %hex,
                    "Packet filtered by MAC"
                );
                return Err(ChannelError::Filtered);
            },
            Err(ChannelError::Dropped) => {
                tracing::debug!(target: "sim_channel", from = %self.from, to = %self.to, size, "Packet dropped by loss simulation");
                return Err(ChannelError::Dropped);
            },
        }

        // Try to enqueue; if the queue is full, treat as a congestion drop.
        if self
            .tx
            .try_send(Packet {
                packet,
                size,
                instant: Instant::now(),
            })
            .is_err()
        {
            tracing::warn!(target: "sim_channel", from = %self.from, to = %self.to, size, "Packet dropped due to congestion (queue full)");
            return Err(ChannelError::Dropped);
        }

        Ok(())
    }

    /// Check if packet should be sent based on MAC filter and loss rate.
    ///
    /// When `self.mac` is all-zeros (`[0u8; 6]`) MAC filtering is disabled —
    /// this is used for point-to-point cloud links where every frame must pass
    /// through regardless of destination MAC (e.g. ARP, IP unicast).
    fn should_send(&self, buf: &[u8]) -> Result<(), ChannelError> {
        let zero_mac = MacAddress::new([0u8; 6]);
        if self.mac != zero_mac {
            let bcast = [255u8; 6];
            let unicast = self.mac.bytes();
            if buf.len() < 6 {
                tracing::warn!(target: "sim_channel", from = %self.from, to = %self.to, "Packet too short for MAC filtering");
                return Err(ChannelError::Filtered);
            }
            if buf[0..6] != bcast && buf[0..6] != unicast {
                return Err(ChannelError::Filtered);
            }
        }

        let loss = self
            .parameters
            .read()
            .expect("channel parameters lock poisoned")
            .loss;
        if loss > 0.0 && rand::random::<f64>() < loss {
            return Err(ChannelError::Dropped);
        }

        Ok(())
    }

    /// Receive a packet from the channel's interface
    ///
    /// This is a passthrough to the underlying TUN interface recv() method.
    pub async fn recv(&self, buf: &mut [u8]) -> Result<usize, std::io::Error> {
        self.tun.recv(buf).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::channel_parameters::ChannelParameters;
    use mac_address::MacAddress;
    use std::collections::HashMap;
    use std::sync::Arc;

    #[tokio::test]
    async fn channel_set_params_updates_and_allows_send() {
        let (tun_a, _peer) = node_lib::test_helpers::util::mk_shim_pair();
        let tun = Arc::new(tun_a);
        let params = ChannelParameters::from(std::collections::HashMap::new());
        let mac = MacAddress::new([0, 1, 2, 3, 4, 5]);

        let ch = Channel::new(
            params,
            mac,
            tun.clone(),
            &"from".to_string(),
            &"to".to_string(),
        );

        let mut map = HashMap::new();
        map.insert("latency".to_string(), "0".to_string());
        map.insert("loss".to_string(), "0".to_string());

        assert!(ch.set_params(map).is_ok());

        let mut packet = [0u8; PACKET_BUFFER_SIZE];
        packet[0..6].copy_from_slice(&mac.bytes());
        packet[6] = 0x42;

        assert!(ch.send(packet, 7).await.is_ok());
    }

    #[tokio::test]
    async fn channel_send_wrong_mac_fails() {
        let (tun_a, _peer) = node_lib::test_helpers::util::mk_shim_pair();
        let tun = Arc::new(tun_a);
        let params = ChannelParameters::from(std::collections::HashMap::new());
        let mac = MacAddress::new([0, 1, 2, 3, 4, 5]);

        let ch = Channel::new(
            params,
            mac,
            tun.clone(),
            &"from".to_string(),
            &"to".to_string(),
        );

        // packet with a different destination MAC
        let mut packet = [0u8; PACKET_BUFFER_SIZE];
        packet[0..6].copy_from_slice(&[9u8, 9, 9, 9, 9, 9]);
        let res = ch.send(packet, 7).await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn channel_send_forced_loss() {
        let (tun_a, _peer) = node_lib::test_helpers::util::mk_shim_pair();
        let tun = Arc::new(tun_a);
        let params = ChannelParameters::from(HashMap::new());
        let mac = MacAddress::new([0, 1, 2, 3, 4, 5]);

        let ch = Channel::new(
            params,
            mac,
            tun.clone(),
            &"from".to_string(),
            &"to".to_string(),
        );

        // Set params with loss = 1.0 to force packet drop
        let mut map = HashMap::new();
        map.insert("latency".to_string(), "0".to_string());
        map.insert("loss".to_string(), "1.0".to_string());
        assert!(ch.set_params(map).is_ok());

        let mut packet = [0u8; PACKET_BUFFER_SIZE];
        packet[0..6].copy_from_slice(&mac.bytes());
        let res = ch.send(packet, 7).await;
        // Should fail due to 100% packet loss
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn channel_zero_mac_allows_any_destination() {
        let (tun_a, _peer) = node_lib::test_helpers::util::mk_shim_pair();
        let tun = Arc::new(tun_a);
        let params = ChannelParameters::from(HashMap::new());
        // All-zeros MAC = no filtering
        let mac = MacAddress::new([0u8; 6]);

        let ch = Channel::new(
            params,
            mac,
            tun.clone(),
            &"from".to_string(),
            &"to".to_string(),
        );

        // A unicast packet addressed to a completely different MAC should pass through.
        let mut packet = [0u8; PACKET_BUFFER_SIZE];
        packet[0..6].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x01]);
        assert!(ch.send(packet, 7).await.is_ok());
    }

    #[tokio::test]
    async fn channel_generates_reads_after_send() {
        use tokio::time::timeout;

        let (tun_a, peer) = node_lib::test_helpers::util::mk_shim_pair();
        let tun = Arc::new(tun_a);
        let params = ChannelParameters::from(HashMap::new());
        let mac = MacAddress::new([0, 1, 2, 3, 4, 5]);

        let ch = Channel::new(
            params,
            mac,
            tun.clone(),
            &"from".to_string(),
            &"to".to_string(),
        );

        let mut packet = [0u8; PACKET_BUFFER_SIZE];
        packet[0..6].copy_from_slice(&mac.bytes());
        packet[6] = 0x99;

        ch.send(packet, 7).await.unwrap();

        // Read from the peer side
        let result = timeout(Duration::from_millis(100), async {
            let mut buf = vec![0u8; 1500];
            peer.recv(&mut buf).await
        })
        .await;

        assert!(result.is_ok());
        let size = result.unwrap().unwrap();
        assert!(size > 0);
    }
}
