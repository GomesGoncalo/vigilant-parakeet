//! Example demonstrating buffer pool usage for efficient packet construction
//!
//! This example shows how to use the buffer pool to minimize allocations
//! when constructing and serializing network packets.

use node_lib::buffer_pool::{get_buffer, return_buffer};
use node_lib::messages::control::heartbeat::Heartbeat;
use node_lib::messages::control::Control;
use node_lib::messages::message::Message;
use node_lib::messages::packet_type::PacketType;
use std::time::Duration;

fn main() {
    println!("=== Buffer Pool Usage Example ===\n");

    // Example 1: Using buffer pool for packet serialization
    example_buffer_pool_basic();

    // Example 2: Flat vs nested serialization comparison
    example_flat_vs_nested();

    // Example 3: Buffer reuse pattern
    example_buffer_reuse();
}

fn example_buffer_pool_basic() {
    println!("Example 1: Basic buffer pool usage\n");

    // Get a buffer from the pool with size hint
    let mut buf = get_buffer(100);
    println!("  - Acquired buffer with capacity: {}", buf.capacity());

    // Use the buffer for packet construction
    buf.extend_from_slice(b"Hello, network!");
    println!("  - Buffer contents: {:?}", std::str::from_utf8(&buf));

    // Return buffer to pool for reuse
    return_buffer(buf);
    println!("  - Buffer returned to pool\n");
}

fn example_flat_vs_nested() {
    println!("Example 2: Flat vs nested serialization\n");

    // Create a heartbeat message
    let from = [0x02; 6].into();
    let to = [0x03; 6].into();
    let heartbeat = Heartbeat::new(Duration::from_secs(1), 42, [0x04; 6].into());
    let packet = PacketType::Control(Control::Heartbeat(heartbeat));
    let message = Message::new(from, to, packet);

    // Modern way: flat Vec<u8> - single allocation
    let flat: Vec<u8> = (&message).into();
    println!("\n  Flat format:");
    println!("    - Allocations: 1");
    println!("    - Total bytes: {}", flat.len());
    println!("    - Memory overhead: ~24 bytes (single Vec header)");

    println!("\n  This demonstrates the flat serialization approach used in production.");
    println!("  The old nested Vec<Vec<u8>> format has been removed.\n");
}

fn example_buffer_reuse() {
    println!("Example 3: Buffer reuse pattern\n");

    // Simulate processing multiple packets
    for i in 0u32..5 {
        let mut buf = get_buffer(64);

        // Construct packet (simplified)
        buf.extend_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]);
        buf.extend_from_slice(&i.to_be_bytes());

        println!("  Packet {}: {} bytes at {:p}", i, buf.len(), buf.as_ptr());

        // Return to pool
        return_buffer(buf);
    }

    println!("\n  Note: Buffer addresses may repeat, showing reuse from pool\n");
}
