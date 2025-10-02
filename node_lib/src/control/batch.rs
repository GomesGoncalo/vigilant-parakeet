// Batch packet processing module
//
// This module provides efficient batch processing of packets to reduce syscall overhead
// and improve throughput. It can be enabled via the "batch_processing" feature flag.

use crate::control::node::ReplyType;
use anyhow::Result;
use common::batch::{RecvBatch, SendBatch, MAX_BATCH_SIZE};
use common::device::Device;
use common::tun::Tun;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

/// Configuration for batch processing
#[derive(Debug, Clone)]
pub struct BatchConfig {
    /// Maximum number of packets to batch
    pub max_batch_size: usize,
    /// Maximum time to wait for batch to fill before sending
    pub max_wait_ms: u64,
    /// Enable adaptive batching based on load
    pub adaptive: bool,
}

impl Default for BatchConfig {
    fn default() -> Self {
        Self {
            max_batch_size: 16,
            max_wait_ms: 1, // 1ms max latency added
            adaptive: true,
        }
    }
}

/// Batch packet receiver
///
/// Attempts to receive multiple packets in a tight loop up to the batch size
/// or timeout, whichever comes first.
pub async fn recv_batch_wire(dev: &Arc<Device>, config: &BatchConfig) -> Result<RecvBatch> {
    let mut batch = RecvBatch::new(config.max_batch_size);
    let deadline = Duration::from_millis(config.max_wait_ms);

    // Try to receive first packet (blocking)
    let mut buf = [0u8; 1500];
    let n = dev.recv(&mut buf).await?;
    batch.push(&buf[..n])?;

    // Try to receive more packets without blocking (opportunistic batching)
    for _ in 1..config.max_batch_size {
        match timeout(deadline, dev.recv(&mut buf)).await {
            Ok(Ok(n)) => {
                if batch.push(&buf[..n]).is_err() {
                    break;
                }
            }
            Ok(Err(_)) | Err(_) => break, // Error or timeout, send what we have
        }
    }

    Ok(batch)
}

/// Batch packet sender
///
/// Groups reply packets by type and sends them using vectored I/O
pub async fn send_batch(
    replies: Vec<ReplyType>,
    tun: &Arc<Tun>,
    dev: &Arc<Device>,
) -> Result<(usize, usize)> {
    let mut wire_batch = SendBatch::new();
    let mut tap_batch = SendBatch::new();

    // Separate packets by destination
    for reply in replies {
        match reply {
            ReplyType::WireFlat(buf) => wire_batch.push(buf),
            ReplyType::TapFlat(buf) => tap_batch.push(buf),
        }
    }

    // Send batches concurrently
    let wire_sent = if !wire_batch.is_empty() {
        let slices = wire_batch.io_slices();
        dev.send_vectored(&slices).await.unwrap_or(0)
    } else {
        0
    };

    let tap_sent = if !tap_batch.is_empty() {
        let slices = tap_batch.io_slices();
        tun.send_vectored(&slices).await.unwrap_or(0)
    } else {
        0
    };

    Ok((wire_sent, tap_sent))
}

/// Adaptive batch size calculator
///
/// Adjusts batch size based on observed packet rate to balance latency and throughput
pub struct AdaptiveBatchSize {
    current_size: usize,
    min_size: usize,
    max_size: usize,
    packets_per_batch_avg: f64,
    alpha: f64, // exponential moving average factor
}

impl AdaptiveBatchSize {
    pub fn new(min_size: usize, max_size: usize) -> Self {
        Self {
            current_size: min_size,
            min_size,
            max_size: max_size.min(MAX_BATCH_SIZE),
            packets_per_batch_avg: min_size as f64,
            alpha: 0.1,
        }
    }

    pub fn current(&self) -> usize {
        self.current_size
    }

    /// Update based on observed batch fill rate
    pub fn update(&mut self, actual_batch_size: usize) {
        // Update exponential moving average
        self.packets_per_batch_avg = self.alpha * (actual_batch_size as f64)
            + (1.0 - self.alpha) * self.packets_per_batch_avg;

        // If we're consistently filling batches, increase size
        if self.packets_per_batch_avg > (self.current_size as f64 * 0.8) {
            self.current_size = (self.current_size * 2).min(self.max_size);
        }
        // If batches are mostly empty, decrease size
        else if self.packets_per_batch_avg < (self.current_size as f64 * 0.3) {
            self.current_size = (self.current_size / 2).max(self.min_size);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_config_default() {
        let config = BatchConfig::default();
        assert_eq!(config.max_batch_size, 16);
        assert_eq!(config.max_wait_ms, 1);
        assert!(config.adaptive);
    }

    #[test]
    fn adaptive_batch_size_increases_on_full_batches() {
        let mut adaptive = AdaptiveBatchSize::new(4, 32);
        assert_eq!(adaptive.current(), 4);

        // Simulate consistently full batches
        for _ in 0..10 {
            adaptive.update(4); // 100% fill rate
        }

        // Should increase batch size
        assert!(adaptive.current() > 4);
    }

    #[test]
    fn adaptive_batch_size_decreases_on_empty_batches() {
        let mut adaptive = AdaptiveBatchSize::new(4, 32);
        adaptive.current_size = 16; // Start at larger size

        // Simulate mostly empty batches
        for _ in 0..10 {
            adaptive.update(2); // 12.5% fill rate (2/16)
        }

        // Should decrease batch size
        assert!(adaptive.current() < 16);
    }

    #[test]
    fn adaptive_batch_size_respects_bounds() {
        let mut adaptive = AdaptiveBatchSize::new(4, 16);

        // Try to increase beyond max
        for _ in 0..20 {
            adaptive.update(adaptive.current());
        }
        assert!(adaptive.current() <= 16);

        // Try to decrease below min
        adaptive.current_size = 8;
        for _ in 0..20 {
            adaptive.update(1);
        }
        assert!(adaptive.current() >= 4);
    }
}
