# GitHub Copilot Instructions for vigilant-parakeet

**ALWAYS follow these instructions first and fallback to additional search and context gathering ONLY if the information here is incomplete or found to be in error.**

Vigilant-parakeet is a Rust workspace that simulates and visualizes vehicular network nodes (OBU/RSU) and their routing protocols. The project consists of multiple crates in a Cargo workspace for building simulation tools and network components.

## Prerequisites and Setup

Install Rust toolchain:
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env
```

Clone and setup:
```bash
git clone <repo-url>
cd vigilant-parakeet
```

Install git hooks (recommended for commit message format compliance):
```bash
./scripts/install-hooks.sh
```

## Building the Project

**CRITICAL TIMING**: NEVER CANCEL builds or long-running commands. Set timeout to 120+ minutes for safety.

Build entire workspace (takes ~1 minute 40 seconds):
```bash
cargo build --workspace
```

Build release version (takes ~1 minute 30 seconds):
```bash
cargo build --workspace --release
```

Build specific components:
```bash
# Simulator with webview feature (takes ~1 minute 40 seconds)
cargo build -p simulator --release --features webview

# Single node binary only
cargo build --bin node --release

# With stats/metrics feature enabled
cargo build --features stats
```

**NEVER CANCEL**: All builds may take 90+ seconds. Wait for completion.

## Testing

Run full test suite (takes ~22 seconds):
```bash
cargo test --workspace
```

Run specific crate tests:
```bash
# Node library tests only (fastest iteration)
cargo test -p node_lib

# With stats feature enabled (adds 2 additional metric tests)
cargo test -p node_lib --features stats

# Specific test with logs
RUST_LOG=trace cargo test -p node_lib -- tests::integration_two_hop -- --nocapture
```

**Test timing**: Full test suite completes in ~22 seconds. NEVER CANCEL.

## Code Quality and Linting

Format code (takes ~1 second):
```bash
cargo fmt --all --check
```

Apply formatting:
```bash
cargo fmt
```

Run clippy linting (takes ~43 seconds):
```bash
cargo clippy --workspace --all-targets -- -D warnings
```

**CRITICAL**: Always run both `cargo fmt` and `cargo clippy` before committing. CI will fail if these checks don't pass.

## Code Coverage

Generate coverage report (takes ~1 minute 20 seconds):
```bash
# Install tarpaulin if needed (takes ~3 minutes 20 seconds first time)
cargo install cargo-tarpaulin --locked

# Run coverage (NEVER CANCEL - takes 80+ seconds)
cargo tarpaulin -p common -p node_lib --out Lcov --features test_helpers --timeout 120
```

**NEVER CANCEL**: Coverage generation takes 80+ seconds. Set timeout to 180+ seconds.

## Running the Application

### Simulator (Multi-node simulation)

**REQUIRES SUDO**: The simulator creates network namespaces and requires root privileges.

Create example configuration files:
```yaml
# simulator-example.yaml
nodes:
  n1:
    config_path: n1.yaml
  n2:
    config_path: n2.yaml
topology:
  n1:
    n2:
      latency: 0
      loss: 0
  n2:
    n1:
      latency: 0
      loss: 0
```

```yaml
# n1.yaml (RSU node)
node_type: Rsu
hello_history: 10
hello_periodicity: 5000
ip: 10.0.0.1
```

```yaml
# n2.yaml (OBU node)
node_type: Obu
hello_history: 10
ip: 10.0.0.2
```

Run simulator:
```bash
# Build first
cargo build -p simulator --release --features webview

# Run with sudo (requires Linux with network namespace support)
sudo RUST_LOG="node=debug" ./target/release/simulator --config-file simulator-example.yaml --pretty
```

The simulator starts an HTTP API on port 3030 for runtime stats and control.

### Single Node

Run individual node:
```bash
./target/release/node --help
# See help output for required parameters (bind interface, node type, etc.)
```

## Validation Scenarios

**ALWAYS test these scenarios after making changes:**

1. **Build and test validation**:
   ```bash
   cargo build --workspace
   cargo test --workspace
   cargo clippy --workspace --all-targets -- -D warnings
   cargo fmt --all --check
   ```

2. **Feature build validation**:
   ```bash
   cargo test -p node_lib --features stats
   cargo build -p simulator --release --features webview
   ```

3. **Coverage validation**:
   ```bash
   cargo tarpaulin -p common -p node_lib --out Lcov --features test_helpers --timeout 120
   ```

## Project Structure

- `common/` — Shared data types and utilities (Device, Tun abstractions)
- `node_lib/` — Core node logic (routing, control plane, wire protocols)
- `node/` — Single node binary for real hardware or testing
- `simulator/` — Multi-node simulation with network namespaces and HTTP API
- `visualization/` — Browser-based visualization frontend (Yew/WASM)

## Key Development Workflows

**Feature development**:
1. Make changes to relevant crate
2. Run `cargo test -p <crate>` for quick iteration
3. Run full test suite: `cargo test --workspace`
4. Run linting: `cargo clippy --workspace --all-targets -- -D warnings`
5. Format: `cargo fmt`

**Stats/metrics development**:
```bash
# Always test with stats feature enabled
cargo test -p node_lib --features stats
cargo build --features stats
```

**Debugging with logs**:
```bash
RUST_LOG=trace cargo test -p node_lib -- <test_name> -- --nocapture
```

## Critical Timing Expectations

- **Build workspace**: 90-120 seconds - NEVER CANCEL, set timeout 180+ seconds
- **Test suite**: 22 seconds - NEVER CANCEL, set timeout 60+ seconds  
- **Clippy linting**: 43 seconds - NEVER CANCEL, set timeout 90+ seconds
- **Coverage**: 80 seconds - NEVER CANCEL, set timeout 180+ seconds
- **Release build**: 90 seconds - NEVER CANCEL, set timeout 180+ seconds

## Additional Tools (Optional)

For visualization development:
```bash
# Install wasm tools (optional)
rustup target add wasm32-unknown-unknown
cargo install wasm-pack trunk
```

Test visualization:
```bash
cd visualization
cargo test  # Host tests
wasm-pack test --headless --firefox  # WASM tests
trunk build --release  # Build frontend
```

## Configuration Notes

- The project uses `.cargo/config.toml` with `rustflags = ["--cfg", "tokio_unstable"]`
- Git hooks enforce Conventional Commits format for commit messages
- Network namespace creation requires sudo on Linux
- HTTP API runs on port 3030 when simulator is active

## Architecture Overview

**Workspace crates:**
- `common/`: Tun and Device abstractions with test shims
- `node_lib/`: Core routing logic (OBU/RSU control planes, wire formats)  
- `node/`: Single node binary
- `simulator/`: Multi-node orchestration with netns
- `visualization/`: Browser UI

**Data flow:**
- RSU emits periodic Heartbeat control messages
- OBUs/RSUs forward and reply, building routing tables
- Routing prefers lower observed latency, falls back to hop count
- Test helpers provide in-memory device pairs for unit tests

**ALWAYS refer to these instructions first. Only search for additional context if commands fail or behavior doesn't match these documented expectations.**