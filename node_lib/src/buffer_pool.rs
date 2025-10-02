//! Buffer pool for efficient packet buffer reuse
//!
//! This module provides a simple buffer pool to reduce allocations for common packet sizes.
//! Buffers are pre-allocated and reused across packet serialization operations.

use bytes::BytesMut;
use std::sync::Mutex;

/// Common packet size categories for pooling
const SMALL_PACKET_SIZE: usize = 256;
const MEDIUM_PACKET_SIZE: usize = 512;
const LARGE_PACKET_SIZE: usize = 1500;

/// Pool capacity for each size category
const POOL_CAPACITY: usize = 32;

// Thread-local buffer pool to avoid contention
thread_local! {
    static SMALL_POOL: Mutex<Vec<BytesMut>> = Mutex::new(Vec::with_capacity(POOL_CAPACITY));
    static MEDIUM_POOL: Mutex<Vec<BytesMut>> = Mutex::new(Vec::with_capacity(POOL_CAPACITY));
    static LARGE_POOL: Mutex<Vec<BytesMut>> = Mutex::new(Vec::with_capacity(POOL_CAPACITY));
}

/// Get a buffer from the pool or allocate a new one
///
/// # Arguments
/// * `size_hint` - Expected size of the packet to be serialized
///
/// # Returns
/// A `BytesMut` buffer with at least the requested capacity
pub fn get_buffer(size_hint: usize) -> BytesMut {
    let (pool, capacity) = match size_hint {
        0..SMALL_PACKET_SIZE => (&SMALL_POOL, SMALL_PACKET_SIZE),
        SMALL_PACKET_SIZE..MEDIUM_PACKET_SIZE => (&MEDIUM_POOL, MEDIUM_PACKET_SIZE),
        _ => (&LARGE_POOL, LARGE_PACKET_SIZE),
    };

    pool.with(|p| {
        let mut pool = p.lock().unwrap();
        pool.pop()
            .unwrap_or_else(|| BytesMut::with_capacity(capacity))
    })
}

/// Return a buffer to the pool for reuse
///
/// # Arguments
/// * `mut buf` - Buffer to return to the pool
///
/// The buffer is cleared and returned to the appropriate pool if there's space.
pub fn return_buffer(mut buf: BytesMut) {
    buf.clear();
    let capacity = buf.capacity();

    let pool = match capacity {
        SMALL_PACKET_SIZE => &SMALL_POOL,
        MEDIUM_PACKET_SIZE => &MEDIUM_POOL,
        LARGE_PACKET_SIZE => &LARGE_POOL,
        _ => return, // Don't pool unusual sizes
    };

    pool.with(|p| {
        let mut pool = p.lock().unwrap();
        if pool.len() < POOL_CAPACITY {
            pool.push(buf);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_buffer_small() {
        let buf = get_buffer(100);
        assert!(buf.capacity() >= SMALL_PACKET_SIZE);
    }

    #[test]
    fn test_get_buffer_medium() {
        let buf = get_buffer(400);
        assert!(buf.capacity() >= MEDIUM_PACKET_SIZE);
    }

    #[test]
    fn test_get_buffer_large() {
        let buf = get_buffer(1000);
        assert!(buf.capacity() >= LARGE_PACKET_SIZE);
    }

    #[test]
    fn test_return_and_reuse() {
        let buf1 = get_buffer(100);
        let ptr1 = buf1.as_ptr();
        return_buffer(buf1);

        let buf2 = get_buffer(100);
        let ptr2 = buf2.as_ptr();

        // Should reuse the same buffer
        assert_eq!(ptr1, ptr2);
    }

    #[test]
    fn test_pool_capacity_limit() {
        // Fill the pool beyond capacity
        for _ in 0..POOL_CAPACITY + 10 {
            let buf = get_buffer(100);
            return_buffer(buf);
        }

        // Verify pool doesn't grow unbounded
        SMALL_POOL.with(|p| {
            let pool = p.lock().unwrap();
            assert!(pool.len() <= POOL_CAPACITY);
        });
    }
}
