# vigilant-parakeet

<!-- Badges -->
<p>
  <a href="https://www.rust-lang.org/"><img alt="Made with Rust" src="https://img.shields.io/badge/Made%20with-Rust-dea584.svg"></a>
  <img alt="Platform" src="https://img.shields.io/badge/platform-Linux-blue.svg">
  <a href="https://tokio.rs/"><img alt="Runtime: Tokio" src="https://img.shields.io/badge/runtime-Tokio-6a5acd.svg"></a>
  <a href="./Cargo.toml"><img alt="Cargo workspace" src="https://img.shields.io/badge/Cargo-workspace-orange.svg"></a>
  <a href="https://github.com/GomesGoncalo/vigilant-parakeet/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/GomesGoncalo/vigilant-parakeet/actions/workflows/ci.yml/badge.svg?branch=main"></a>
  <a href="https://GomesGoncalo.github.io/vigilant-parakeet/"><img alt="Coverage" src="https://img.shields.io/endpoint?url=https://GomesGoncalo.github.io/vigilant-parakeet/badges/coverage.json"></a>
  <a href="#license"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-green.svg"></a>
</p>

<!-- The Codecov badge will show coverage once CODECOV_TOKEN is configured in repo secrets. -->

Simulate and visualize vehicular network nodes (OBU/RSU) and their routing.

<!-- toc -->

## Quick summary

`vigilant-parakeet` implements a simulator and visualization for experimenting
with network conditions (latency, loss) while running multiple instances of the
node logic used by RSUs/OBUs. It was originally created to help reproduce
behaviour from: https://www.researchgate.net/publication/286923369_L3_Security_in_Vehicular_Networks

This repository is a Rust Cargo workspace with several crates. The main
deliverables are the `simulator` binary (runs many nodes in separate network
namespaces) and the `visualization` UI (browser-based dashboard).

## Checklist (what I'll cover in this README)

- Quick Start (build & run)
- Features and intended use
- Project structure (what each crate does)
- Configuration examples (simulator + node)
- Useful commands (stats, change channels, network tests)
- Contributing & development tips

## Features

- Run many instances of `node_lib` inside isolated Linux network namespaces
- Simulate per-link latency and packet loss
- **External tap interfaces for RSU nodes** - Connect RSUs to external servers via 172.x.x.x network (see [docs/EXTERNAL_TAP_INTERFACE.md](docs/EXTERNAL_TAP_INTERFACE.md))
- **DH message signing** — Ed25519 digital signatures on key exchange messages authenticate the DH handshake (see [DH Signatures](#dh-message-signing) below)
- HTTP API for runtime stats and for changing channel parameters
- Browser visualization (in `visualization/`) to monitor traffic and change
  parameters interactively via the simulator HTTP API (webview).

## Project structure

- `common/` — shared data types and utilities used across crates.
- `node_lib/` — shared building blocks (messages, crypto, metrics, routing utils).
- `obu_lib/` — OBU node implementation and CLI args.
- `rsu_lib/` — RSU node implementation and CLI args.
- `node/` — binary crate exposing subcommands `node obu` and `node rsu`.
- `simulator/` — orchestrates multiple nodes in network namespaces and exposes an HTTP control API.
- `visualization/` — front-end app to inspect network topology and live stats.

See top-level `Cargo.toml` for the workspace and crate manifests.

## Prerequisites

- Rust toolchain (stable). Install from https://rustup.rs
- `cargo` (comes with rustup)
- On Linux: `sudo` privileges are required to create network namespaces and
  virtual interfaces used by the simulator.
- Optional: `trunk` (for the front-end) and `npm`/`node` if you extend the UI.

## Quick Start

Clone the repo and build everything (from the repository root):

```sh
git clone <repo-url>
cd vigilant-parakeet
cargo build --workspace
```

Build release artifacts (all crates):

```sh
cargo build --workspace --release
```

Build the simulator (webview, TUI, and stats enabled by default):

```sh
# Build simulator (includes web API, TUI and metrics by default)
cargo build -p simulator --release
```

Build the node binary only:

```sh
cargo build --bin node --release
```

## Configuration

The simulator is configured using a YAML file that specifies nodes and the
topology (per-link latency/loss). Each node points to its own node config file.

Example `simulator.yaml`:

```yaml
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

Example node config for an RSU (`n1.yaml`):

```yaml
node_type: Rsu
hello_history: 10
hello_periodicity: 5000
ip: 10.0.0.1
# Optional: External tap interface for server connectivity
external_tap_ip: 172.16.0.1
external_tap_name: ext_rsu1
```

Example node config for an OBU (`n2.yaml`):

```yaml
node_type: Obu
hello_history: 10
ip: 10.0.0.2
```

## Running

Start the simulator (from the repo root). The simulator creates network
namespaces and virtual interfaces, so run with `sudo`:

```sh
# Standard mode with console logging
sudo RUST_LOG="node=debug" ./target/release/simulator --config-file simulator.yaml --pretty

# With TUI dashboard (requires --features tui at build time)
sudo RUST_LOG="node=debug" ./target/release/simulator --config-file simulator.yaml --tui

# With TUI + Webview API (requires both features)
sudo RUST_LOG="node=debug" ./target/release/simulator --config-file simulator.yaml --tui
```

**TUI Dashboard Features:**
- **Metrics Tab**: Real-time packet statistics, performance metrics, and live graphs
  - Packets sent/dropped/delayed with totals
  - Drop rate and average latency
  - Throughput and uptime tracking
  - 4 live graphs with 60-second history
- **Logs Tab**: Captured simulation logs with color-coded levels (ERROR, WARN, INFO, DEBUG, TRACE)
- **Controls**: 
  - **Q/Esc/Ctrl+C** - Quit TUI
  - **R** - Reset metrics
  - **Tab** or **1/2** - Switch between tabs
  - **↑/↓/PgUp/PgDn** - Scroll logs
  - **Home** - Go to top of logs

**Webview API** (if built with `--features webview`):
- HTTP metrics endpoint: `curl http://localhost:3030/metrics`
- Returns JSON with all simulation metrics
- **Works with TUI**: When both features are enabled, webview runs alongside TUI

### Example: 3-node setup

Below is a small example (`simulator.yaml`) that wires three nodes (n1, n2, n3)
with asymmetric latencies and small loss on one link. Save it at the repository
root and create matching `n1.yaml`, `n2.yaml`, `n3.yaml` files (examples below).

```yaml
nodes:
  n1:
    config_path: n1.yaml
  n2:
    config_path: n2.yaml
  n3:
    config_path: n3.yaml
topology:
  n1:
    n2:
      latency: 10
      loss: 0.0
    n3:
      latency: 50
      loss: 0.01
  n2:
    n1:
      latency: 10
      loss: 0.0
    n3:
      latency: 20
      loss: 0.0
  n3:
    n1:
      latency: 50
      loss: 0.01
    n2:
      latency: 20
      loss: 0.0
```

Example node config for `n1.yaml` (RSU):

```yaml
node_type: Rsu
hello_history: 10
hello_periodicity: 5000
ip: 10.0.0.1
```

Example node config for `n2.yaml` and `n3.yaml` (OBU):

```yaml
node_type: Obu
hello_history: 10
ip: 10.0.0.2 # change to .3 for n3
```

You can run the included helper script to build and launch the simulator. The
script will look for `examples/simulator.yaml` by default; the repository also
contains ready-to-run example node configs in `examples/`.

```sh
./scripts/run-sim.sh
```

When running, the simulator exposes an HTTP API (default port `3030`) that
provides runtime stats and control endpoints.

Get per-node traffic statistics:

```sh
curl http://127.0.0.1:3030/stats | jq
```

Change channel parameters (latency/loss) between two nodes:

```sh
curl -X POST -H "Content-Type: application/json" \
  -d '{"latency":"100","loss":"0.0"}' \
  http://localhost:3030/channel/n1/n2/
```

Network tests inside namespaces (examples):

```sh
# start iperf server inside namespace n1
sudo ip netns exec sim_ns_n1 runuser -l $USER -c "iperf -s -i 1"

# run iperf client from namespace n2 to n1
sudo ip netns exec sim_ns_n2 runuser -l $USER -c "iperf -c 10.0.0.1 -i 1 -t 10"

# ping example
sudo ip netns exec sim_ns_n2 runuser -l $USER -c "ping 10.0.0.1"
```

## Visualization

The `visualization/` folder contains a front-end app. To run it locally (if
you have `trunk` installed):

```sh
cd visualization
trunk serve
# open http://127.0.0.1:8080/
```

The UI can display topology, per-link parameters, and live traffic charts.

## DH Message Signing

`vigilant-parakeet` supports **Ed25519 digital signatures** on Diffie-Hellman key
exchange messages, providing authentication of the DH handshake. Without signatures,
a node cannot verify that a `KeyExchangeInit` or `KeyExchangeReply` was sent by a
legitimate peer rather than an attacker injecting fake DH public keys.

### How it works

When `enable_dh_signatures: true` is configured:

1. Each node (OBU or Server) generates a **random Ed25519 identity keypair** at startup
   (or loads a stable keypair from a configured seed — see PKI mode below).
2. Every outgoing `KeyExchangeInit` or `KeyExchangeReply` is **signed** over its
   42-byte base payload (`key_id | dh_public_key | sender_mac`).
3. The 32-byte Ed25519 verifying key and 64-byte signature are appended to the
   message, extending the wire format from 42 bytes to **138 bytes**.
4. The receiver **verifies** the signature before accepting the key exchange. Messages
   with missing or invalid signatures are dropped with a warning log.
5. Intermediate relay nodes (OBUs forwarding KE messages up or down the tree)
   **preserve** signatures unchanged.

#### Trust models

| Mode | What it provides | What it does not prevent |
|------|-----------------|--------------------------|
| **TOFU** (default) | Blocks key-substitution MitM on subsequent contacts | First-contact impersonation (attacker with own keypair) |
| **PKI** (allowlist) | Also blocks first-contact impersonation | Nothing — full authentication |

The default mode is TOFU. Enable PKI mode by pre-registering OBU public keys on the
server (see below).

### Enabling DH signatures on an OBU node — TOFU mode (`n2.yaml`)

```yaml
node_type: Obu
hello_history: 10
ip: 10.0.0.2
enable_encryption: true       # required — signatures only apply to DH messages
enable_dh_signatures: true    # sign outgoing KE messages, verify incoming ones
```

At startup the node logs its signing public key:

```
INFO signing_pubkey=aabbcc... DH signing enabled — register this public key in the server's dh_signing_allowlist to enforce PKI authentication
```

### Enabling DH signatures on a Server node — TOFU mode (`server.yaml`)

```yaml
node_type: Server
virtual_ip: 10.0.0.50
cloud_ip: 172.16.0.50
port: 8080
enable_encryption: true       # required
enable_dh_signatures: true    # sign KE replies, verify incoming KE inits
```

The server logs its signing public key at startup so you can register it on OBUs:

```
INFO signing_pubkey=e5f6a7b8... DH signing enabled on server
```

To get a stable, repeatable public key across restarts, configure a fixed seed (see PKI mode).

### PKI mode — pre-registering OBU identities

To close the first-contact impersonation gap, give each OBU a **stable keypair** (via
a persistent seed) and register the corresponding public key on the server.

**Step 1 — generate a keypair for each node** using the built-in keygen command:

```sh
keygen
# Ed25519 signing keypair for DH authentication
#
# Seed (signing_key_seed in node YAML — keep secret):
#   a1b2c3d4...64hexchars
#
# Verifying key (for dh_signing_allowlist on server, or server_signing_pubkey on OBU):
#   e5f6a7b8...64hexchars
```

`keygen` uses a cryptographically secure RNG (`OsRng`). The seed is secret — treat
it like a private key. The verifying key is what you distribute to peers.

**Step 2 — pin the VANET MAC address** of each OBU so it matches the allowlist entry.
The allowlist is keyed by VANET MAC; since the simulator assigns MACs randomly at
startup you must fix them in config (`n2.yaml`):

```yaml
node_type: Obu
hello_history: 10
ip: 10.0.0.2
enable_encryption: true
enable_dh_signatures: true
vanet_mac: "AA:BB:CC:DD:EE:FF"              # fixed MAC for the VANET interface
signing_key_seed: "a1b2c3d4...64hexchars"   # stable 32-byte seed, hex-encoded
server_signing_pubkey: "e5f6a7b8...64hexchars"  # server's verifying key (optional)
```

`vanet_mac` is applied via `ip link set <iface> address <mac> up` after the TAP is
created, so it requires the same privileges as the rest of the simulator (root /
`CAP_NET_ADMIN`).

**Step 3 — give the server a stable identity** (`server.yaml`):

Run `keygen` once for the server too, then set its seed in config:

```yaml
node_type: Server
virtual_ip: 10.0.0.50
cloud_ip: 172.16.0.50
port: 8080
enable_encryption: true
enable_dh_signatures: true
signing_key_seed: "e5f6a7b8...64hexchars"   # stable seed — keep secret
```

The server derives the same keypair every restart and logs its verifying key at startup.
OBUs can then pin that key via `server_signing_pubkey`.

**Step 4 — register OBU public keys on the server** (`server.yaml`):

```yaml
node_type: Server
virtual_ip: 10.0.0.50
cloud_ip: 172.16.0.50
port: 8080
enable_encryption: true
enable_dh_signatures: true
dh_signing_allowlist:
  "AA:BB:CC:DD:EE:FF": "aabbcc...64hexchars"   # OBU n2's verifying key
  "11:22:33:44:55:66": "112233...64hexchars"   # OBU n3's verifying key
```

When `dh_signing_allowlist` is non-empty, the server rejects any
`KeyExchangeInit` whose `signing_pubkey` does not match the pre-registered key for
that OBU MAC address. OBUs not in the allowlist cannot complete key exchange.

**Optional — pin the server's identity on each OBU** (`n2.yaml`):

```yaml
server_signing_pubkey: "e5f6a7b8...64hexchars"
```

When set, the OBU rejects any `KeyExchangeReply` whose signing key does not match,
preventing a rogue server from completing the exchange even on first contact.

### Mixed deployments

Signatures are **optional per-node**. An OBU with `enable_dh_signatures: true`
will drop any unsigned `KeyExchangeReply` from the server. A Server with
`enable_dh_signatures: true` will drop any unsigned `KeyExchangeInit` from an
OBU. Therefore, for end-to-end signature enforcement both the OBU and the Server
must have `enable_dh_signatures: true`.

Nodes with `enable_dh_signatures: false` (the default) continue to work exactly
as before — unsigned, 42-byte KE messages.

### Wire format summary

| Mode     | Size (bytes) | Contents |
|----------|-------------|----------|
| Unsigned | 42          | `key_id (4) \| dh_pubkey (32) \| sender (6)` |
| Signed   | 138         | `key_id (4) \| dh_pubkey (32) \| sender (6) \| ed25519_verifying_key (32) \| ed25519_signature (64)` |

The signature covers the first 42 bytes (the base payload).

## Development tips

- Run the unit tests and workspace checks:

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt -- --check
```

- Follow Rust idioms and keep `node_lib` focused on logic so `node` and
  `simulator` stay thin wrappers.

## Testing & test helpers

Run the full test suite from the repository root to verify the workspace:

```sh
cargo test --workspace
```

Run tests for the `node_lib` crate only (useful when iterating on routing/utils shared logic):

```sh
cargo test -p node_lib
```

The repository provides shared, reusable test helpers to simplify integration
tests. In particular there is a canonical `Hub` helper you should use instead
of duplicating a local implementation in tests. Import it from the crate root:

```rust
use node_lib::test_helpers::hub::Hub;
// or, from inside tests that already re-export it:
// use crate::tests::hub::Hub;
```

Key helpers to know about:
- `node_lib::test_helpers::hub::Hub` — in-process programmable hub that
  forwards frames between endpoints and can inject per-link latency and
  provide upstream/downstream watch hooks used by integration tests.
- `common::tun::test_tun::TokioTun` and `common::Tun::new_shim` — a TUN shim
  used by tests to inject/observe TAP traffic without creating OS TUN devices.

When adding or updating integration tests, prefer the shared helpers and
export a single `hub` module for tests (the repository already provides
`node_lib/tests/hub.rs` which re-exports the shared helper).

## Contributing

Contributions are welcome. Suggested workflow:

1. Open an issue describing the bug or feature.
2. Create a branch for your work.
3. Submit a pull request with tests and a clear description.

Please run `cargo fmt` and `cargo clippy` before submitting.

## License

This project is licensed under the MIT License. See [LICENSE](./LICENSE) for details.
