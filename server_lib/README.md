# server_lib

A simple UDP server library for receiving traffic from RSU nodes in the vigilant-parakeet vehicular network simulation.

## Overview

`server_lib` provides a UDP server implementation that can receive traffic via standard networking, unlike OBU/RSU nodes which use the custom vehicular routing protocol. This is useful for simulating scenarios where vehicular nodes communicate with external cloud servers or backend systems.

## Features

- Simple UDP packet reception
- Configurable IP address and port
- Async/await using tokio
- Comprehensive logging
- Builder pattern for flexible configuration
- Full test coverage

## Usage

### As a Library

Add to your `Cargo.toml`:

```toml
[dependencies]
server_lib = { path = "../server_lib" }
tokio = { version = "*", features = ["full"] }
```

Example:

```rust
use server_lib::{ServerArgs, ServerParameters, create};
use std::net::Ipv4Addr;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Create server from args
    let args = ServerArgs {
        ip: Ipv4Addr::new(192, 168, 1, 100),
        server_params: ServerParameters { port: 8080 },
    };
    
    let server = create(args).await?;
    
    // Server is now listening and receiving packets
    // Wait for shutdown signal
    tokio::signal::ctrl_c().await?;
    Ok(())
}
```

### Using the Builder

```rust
use server_lib::ServerBuilder;
use std::net::Ipv4Addr;

let server = ServerBuilder::new(Ipv4Addr::new(127, 0, 0, 1))
    .with_port(9000)
    .build()?;

server.start().await?;
```

### As a Standalone Binary

Use the `node` binary:

```bash
# Run server on localhost:8080
node server --ip 127.0.0.1

# Run server on specific IP and port
node server --ip 192.168.1.100 --port 9000
```

### In the Simulator

Create a server node configuration file:

```yaml
# n_server1.yaml
node_type: Server
ip: 192.168.100.1
port: 8080
```

Add to simulator configuration:

```yaml
nodes:
  server1:
    config_path: examples/n_server1.yaml
  rsu1:
    config_path: examples/n_rsu1.yaml

topology:
  # Server nodes are NOT included in topology
```

## Architecture

- `server.rs` - Core Server implementation with UDP socket handling
- `args.rs` - CLI argument definitions using clap
- `builder.rs` - Builder pattern for flexible Server creation
- `lib.rs` - Public API and convenience functions

See [ARCHITECTURE.md](ARCHITECTURE.md) for detailed design documentation.

## Testing

Run tests:

```bash
cargo test -p server_lib
```

Tests cover:
- Server creation and configuration
- UDP socket binding and listening
- Packet reception
- Builder pattern functionality
- Args parsing

## Logging

The server uses `tracing` for structured logging:

- `info` - Server startup and lifecycle events
- `debug` - Received packet information (source, length)
- `trace` - Packet content preview (first 64 bytes)

Example:

```bash
RUST_LOG=debug node server --ip 127.0.0.1
```

## Differences from OBU/RSU Nodes

Server nodes:
- Do NOT implement the `Node` trait
- Do NOT participate in the routing protocol
- Do NOT require Device/Tun abstractions
- Use standard UDP sockets
- Are receive-only (currently)

This makes them simpler and more suitable for representing external backend systems.

## API Reference

### `Server`

Main server struct that handles UDP packet reception.

**Methods:**
- `new(ip, port)` - Create a new Server
- `start()` - Start listening for UDP packets
- `ip()` - Get the configured IP address
- `port()` - Get the configured port

### `ServerBuilder`

Fluent builder for creating Server instances.

**Methods:**
- `new(ip)` - Create builder with IP address
- `from_args(args)` - Create builder from ServerArgs
- `with_port(port)` - Set the port
- `build()` - Build the Server instance

### `create(args)`

Convenience function that creates and starts a server.

**Parameters:**
- `args: ServerArgs` - Server configuration

**Returns:**
- `Result<Arc<Server>>` - Started server instance

## Examples

See the `examples/` directory in the project root for complete examples.

## License

MIT
