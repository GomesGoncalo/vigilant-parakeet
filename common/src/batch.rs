// Batch processing for packet I/O
//
// This module provides efficient batch operations for sending and receiving multiple packets
// in a single system call, reducing syscall overhead and improving throughput.

use std::io::{self, IoSlice};

/// Maximum number of packets to batch in a single operation
pub const MAX_BATCH_SIZE: usize = 32;

/// Maximum packet size for Ethernet frames
pub const MAX_PACKET_SIZE: usize = 1500;

/// A batch of received packets
#[derive(Debug)]
pub struct RecvBatch {
    /// Packet buffers
    buffers: Vec<[u8; MAX_PACKET_SIZE]>,
    /// Number of bytes received in each packet
    lengths: Vec<usize>,
    /// Number of valid packets in this batch
    count: usize,
}

impl RecvBatch {
    /// Create a new receive batch with the specified capacity
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.min(MAX_BATCH_SIZE);
        Self {
            buffers: vec![[0u8; MAX_PACKET_SIZE]; capacity],
            lengths: vec![0; capacity],
            count: 0,
        }
    }

    /// Get the number of packets in this batch
    #[inline]
    pub fn len(&self) -> usize {
        self.count
    }

    /// Check if the batch is empty
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Get an iterator over the packets in this batch
    pub fn iter(&self) -> impl Iterator<Item = &[u8]> {
        (0..self.count).map(move |i| &self.buffers[i][..self.lengths[i]])
    }

    /// Clear the batch
    #[inline]
    pub fn clear(&mut self) {
        self.count = 0;
    }

    /// Add a packet to the batch
    #[inline]
    pub fn push(&mut self, data: &[u8]) -> io::Result<()> {
        if self.count >= self.buffers.len() {
            return Err(io::Error::other("batch full"));
        }
        let len = data.len().min(MAX_PACKET_SIZE);
        self.buffers[self.count][..len].copy_from_slice(&data[..len]);
        self.lengths[self.count] = len;
        self.count += 1;
        Ok(())
    }

    /// Get capacity of the batch
    #[inline]
    pub fn capacity(&self) -> usize {
        self.buffers.len()
    }
}

/// A batch of packets to send
#[derive(Debug)]
pub struct SendBatch {
    /// Packet data
    packets: Vec<Vec<u8>>,
}

impl SendBatch {
    /// Create a new send batch
    pub fn new() -> Self {
        Self {
            packets: Vec::with_capacity(MAX_BATCH_SIZE),
        }
    }

    /// Create a new send batch with specific capacity
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            packets: Vec::with_capacity(capacity.min(MAX_BATCH_SIZE)),
        }
    }

    /// Add a packet to the batch
    #[inline]
    pub fn push(&mut self, packet: Vec<u8>) {
        if self.packets.len() < MAX_BATCH_SIZE {
            self.packets.push(packet);
        }
    }

    /// Get the number of packets in this batch
    #[inline]
    pub fn len(&self) -> usize {
        self.packets.len()
    }

    /// Check if the batch is empty
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.packets.is_empty()
    }

    /// Check if the batch is full
    #[inline]
    pub fn is_full(&self) -> bool {
        self.packets.len() >= MAX_BATCH_SIZE
    }

    /// Clear the batch
    #[inline]
    pub fn clear(&mut self) {
        self.packets.clear();
    }

    /// Get an iterator over packet IoSlices for vectored I/O
    pub fn io_slices(&self) -> Vec<IoSlice<'_>> {
        self.packets.iter().map(|p| IoSlice::new(p)).collect()
    }

    /// Consume the batch and return the packets
    pub fn into_packets(self) -> Vec<Vec<u8>> {
        self.packets
    }
}

impl Default for SendBatch {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recv_batch_new() {
        let batch = RecvBatch::new(16);
        assert_eq!(batch.len(), 0);
        assert!(batch.is_empty());
        assert_eq!(batch.capacity(), 16);
    }

    #[test]
    fn recv_batch_push() {
        let mut batch = RecvBatch::new(4);
        assert!(batch.push(&[1, 2, 3, 4]).is_ok());
        assert_eq!(batch.len(), 1);
        assert!(!batch.is_empty());

        assert!(batch.push(&[5, 6]).is_ok());
        assert_eq!(batch.len(), 2);
    }

    #[test]
    fn recv_batch_iter() {
        let mut batch = RecvBatch::new(4);
        batch.push(&[1, 2, 3]).unwrap();
        batch.push(&[4, 5]).unwrap();

        let packets: Vec<_> = batch.iter().collect();
        assert_eq!(packets.len(), 2);
        assert_eq!(packets[0], &[1, 2, 3]);
        assert_eq!(packets[1], &[4, 5]);
    }

    #[test]
    fn recv_batch_clear() {
        let mut batch = RecvBatch::new(4);
        batch.push(&[1, 2, 3]).unwrap();
        assert_eq!(batch.len(), 1);

        batch.clear();
        assert_eq!(batch.len(), 0);
        assert!(batch.is_empty());
    }

    #[test]
    fn send_batch_new() {
        let batch = SendBatch::new();
        assert_eq!(batch.len(), 0);
        assert!(batch.is_empty());
        assert!(!batch.is_full());
    }

    #[test]
    fn send_batch_push() {
        let mut batch = SendBatch::new();
        batch.push(vec![1, 2, 3]);
        assert_eq!(batch.len(), 1);
        assert!(!batch.is_empty());
        assert!(!batch.is_full());
    }

    #[test]
    fn send_batch_full() {
        let mut batch = SendBatch::new();
        for i in 0..MAX_BATCH_SIZE {
            batch.push(vec![i as u8]);
        }
        assert!(batch.is_full());
        assert_eq!(batch.len(), MAX_BATCH_SIZE);
    }

    #[test]
    fn send_batch_io_slices() {
        let mut batch = SendBatch::new();
        batch.push(vec![1, 2, 3]);
        batch.push(vec![4, 5]);

        let slices = batch.io_slices();
        assert_eq!(slices.len(), 2);
    }

    #[test]
    fn send_batch_into_packets() {
        let mut batch = SendBatch::new();
        batch.push(vec![1, 2, 3]);
        batch.push(vec![4, 5]);

        let packets = batch.into_packets();
        assert_eq!(packets.len(), 2);
        assert_eq!(packets[0], vec![1, 2, 3]);
        assert_eq!(packets[1], vec![4, 5]);
    }
}
