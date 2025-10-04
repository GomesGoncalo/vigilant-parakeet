//! Network channel simulation with configurable latency, jitter, and packet loss
//!
//! This module provides a Channel abstraction that simulates network conditions between nodes,
//! including latency delays, jitter variation, and probabilistic packet loss.

use anyhow::Result;
use common::channel_parameters::ChannelParameters;
use common::tun::Tun;
use mac_address::MacAddress;
use node_lib::PACKET_BUFFER_SIZE;
use rand::Rng;
use std::{
    sync::{Arc, RwLock},
    time::Duration,
};
use tokio::sync::mpsc;
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

/// Simulated network channel between two nodes
///
/// Provides configurable network conditions:
/// - **Latency**: Base delay for packet transmission
/// - **Jitter**: Random variation in latency (±jitter)
/// - **Loss**: Probabilistic packet drop rate (0.0 to 1.0)
///
/// The channel filters packets by MAC address and applies the configured
/// network conditions before forwarding to the destination interface.
pub struct Channel {
    tx: mpsc::UnboundedSender<Packet>,
    #[cfg_attr(not(feature = "webview"), allow(dead_code))]
    param_notify_tx: mpsc::UnboundedSender<()>,
    parameters: RwLock<ChannelParameters>,
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
        let _ = self.param_notify_tx.send(());
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
        from: &String,
        to: &String,
    ) -> Arc<Self> {
        // Use unbounded channels for zero-copy fast path
        let (tx, mut rx) = mpsc::unbounded_channel();
        let (param_notify_tx, mut param_notify_rx) = mpsc::unbounded_channel();

        tracing::info!(from, to, ?parameters, "Created channel");
        let this = Arc::new(Self {
            tx,
            param_notify_tx,
            parameters: parameters.into(),
            mac,
            tun,
            from: from.clone(),
            to: to.clone(),
        });
        let thisc = this.clone();

        // Spawn task to process packets from channel with latency simulation
        tokio::spawn(async move {
            loop {
                let Some(packet) = rx.recv().await else {
                    // Channel closed, exit task
                    break;
                };

                loop {
                    // Calculate latency with jitter in a separate scope to drop the lock
                    let latency = {
                        let params = thisc
                            .parameters
                            .read()
                            .expect("channel parameters lock poisoned");

                        // Apply base latency + random jitter
                        let mut latency = params.latency;
                        if !params.jitter.is_zero() {
                            let mut rng = rand::rng();
                            // Generate random jitter in range [-jitter, +jitter]
                            let jitter_ms = params.jitter.as_millis() as i64;
                            let random_jitter = rng.random_range(-jitter_ms..=jitter_ms);
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
                        let _ = thisc.tun.send_all(&packet.packet[..packet.size]).await;
                        break;
                    } else {
                        tokio::select! {
                            _ = tokio_timerfd::sleep(duration) => {
                                let _ = thisc.tun.send_all(&packet.packet[..packet.size]).await;
                                break;
                            },
                            _ = param_notify_rx.recv() => {
                                // Parameters changed, recalculate duration
                            },
                        }
                    }
                }
            }
        });
        this
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
    pub async fn send(&self, packet: [u8; PACKET_BUFFER_SIZE], size: usize) -> Result<(), ChannelError> {
        self.should_send(&packet[..size])?;

        // Send packet through unbounded channel - no blocking on fast path
        // This should never fail with unbounded channel unless receiver is dropped
        let _ = self.tx.send(Packet {
            packet,
            size,
            instant: Instant::now(),
        });

        Ok(())
    }

    /// Check if packet should be sent based on MAC filter and loss rate
    fn should_send(&self, buf: &[u8]) -> Result<(), ChannelError> {
        let bcast = vec![255; 6];
        let unicast = self.mac.bytes();
        if buf[0..6] != bcast && buf[0..6] != unicast {
            return Err(ChannelError::Filtered);
        }

        let loss = self
            .parameters
            .read()
            .expect("channel parameters lock poisoned")
            .loss;
        if loss > 0.0 {
            let mut rng = rand::rng();
            if rng.random::<f64>() < loss {
                return Err(ChannelError::Dropped);
            }
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
