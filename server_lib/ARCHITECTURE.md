# Server Library Architecture

## Overview

The `server_lib` crate provides a simple UDP server implementation for receiving traffic from RSU nodes via standard networking. Unlike OBU/RSU nodes that use the custom vehicular routing protocol, Server nodes operate using normal UDP sockets.

## Components

### Server (`src/server.rs`)

The core `Server` struct that handles UDP packet reception:

```rust
pub struct Server {
    ip: Ipv4Addr,
    port: u16,
    socket: Arc<Mutex<Option<Arc<UdpSocket>>>>,
}
```

**Key methods:**
- `new(ip, port)` - Creates a new Server instance
- `start()` - Binds to the UDP socket and spawns a background task to receive packets
- `ip()` - Returns the configured IP address
- `port()` - Returns the configured port

### Args (`src/args.rs`)

Command-line argument parsing using `clap`:

```rust
pub struct ServerArgs {
    pub ip: Ipv4Addr,
    pub server_params: ServerParameters,
}

pub struct ServerParameters {
    pub port: u16,
}
```

### Builder (`src/builder.rs`)

Fluent builder pattern for constructing Server instances:

```rust
ServerBuilder::new(ip)
    .with_port(8080)
    .build()
```

### Library Interface (`src/lib.rs`)

Public API for creating and managing Server instances:

```rust
pub async fn create(args: ServerArgs) -> Result<Arc<Server>>
```

## Usage Patterns

### As a Library

```rust
use server_lib::{ServerArgs, ServerParameters, create};
use std::net::Ipv4Addr;

let args = ServerArgs {
    ip: Ipv4Addr::new(192, 168, 1, 1),
    server_params: ServerParameters { port: 8080 },
};

let server = create(args).await?;
// Server is now listening and receiving packets
```

### In the Simulator

The simulator integrates Server nodes by:

1. Parsing Server node configurations from YAML
2. Creating Server instances using `ServerBuilder`
3. Starting the server after namespace creation
4. Server runs independently, receiving UDP traffic via normal networking

### As a Standalone Binary

The `node` binary supports running as a Server:

```bash
node server --ip 192.168.1.100 --port 8080
```

## Design Principles

### Simplicity

Server nodes are intentionally simple - they only receive UDP packets and log them. This makes them easy to understand, test, and extend.

### Independence

Unlike OBU/RSU nodes, Server nodes:
- Don't implement the `Node` trait
- Don't participate in routing protocols
- Don't require Device/Tun abstractions
- Use standard networking primitives

### Composability

The library is designed to be used in multiple contexts:
- Simulator integration
- Standalone binary
- Testing scenarios
- Custom applications

## Testing

Unit tests cover:
- Server creation and configuration
- UDP socket binding and listening
- Packet reception
- Builder pattern functionality

Run tests with:
```bash
cargo test -p server_lib
```

## Future Enhancements

Potential improvements:
- Packet statistics and metrics
- Response capability (bidirectional communication)
- Multiple listening addresses/ports
- TCP support
- Custom packet handlers/callbacks
- Integration with monitoring systems
