# Server Implementation Usage Guide

This document describes how to use the centralized server functionality for RSU traffic decryption.

## Overview

The server implementation moves encrypted traffic decryption from individual RSUs to a centralized server. This enables:

- **Multi-RSU coordination**: Server handles traffic distribution between multiple RSUs
- **OBU handover scenarios**: OBUs can seamlessly move between RSU coverage areas
- **Centralized traffic decryption**: Single point for decryption while routing remains decentralized

## Architecture

```
OBU1 ──(encrypted)──> RSU1 ──(UDP)──> Server ──(encrypted)──> RSU2 ──(decrypted)──> OBU2
OBU2 ──(encrypted)──> RSU2              │
                                        └──(encrypted)──> RSU1
```

1. OBUs send encrypted upstream traffic to RSUs
2. RSUs forward encrypted data to server via UDP
3. Server decrypts traffic and determines distribution
4. Server sends encrypted data back to appropriate RSUs
5. RSUs decrypt and forward traffic to destination OBUs

## Configuration

Configure RSUs with `server_address` (mandatory):

```yaml
# rsu-config.yaml
node_type: Rsu
hello_history: 10
hello_periodicity: 5000
ip: 10.0.1.1
enable_encryption: true
server_address: "127.0.0.1:8080"  # Forward to centralized server
cached_candidates: 3
```

## Running the Simulator

### With Centralized Server

```bash
# Build the simulator
cargo build -p simulator --release --features webview

# Start simulator with server on port 8080
sudo RUST_LOG="node=debug" ./target/release/simulator \
  --config-file examples/simulator-with-server.yaml \
  --server-address 127.0.0.1:8080 \
  --pretty
```

## Example Configurations

The `examples/` directory contains sample configurations:

- `simulator-with-server.yaml` - Multi-RSU setup with centralized server
- `rsu1-server.yaml`, `rsu2-server.yaml` - RSU configs with server address
- `obu1.yaml`, `obu2.yaml` - OBU configurations

## Testing

Run the server integration tests:

```bash
cargo test --test integration_server
```

Validate full test suite:

```bash
cargo test --workspace
```

## Configuration Priority

Server address must be specified in the RSU config file: `server_address: "127.0.0.1:8080"`

## Monitoring

When server is enabled:

- Server logs show traffic decryption and distribution
- RSU logs show "Received server response" messages
- HTTP API on port 3030 provides node status and statistics

## Troubleshooting

**Server not starting**: Check if port is available and you have network permissions

**RSUs not connecting**: Verify server address in config and network connectivity

**Traffic not flowing**: Check encryption settings match between OBUs and RSUs