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
- HTTP API for runtime stats and for changing channel parameters
- Browser visualization (in `visualization/`) to monitor traffic and change
  parameters interactively (optional `webview` feature)

## Project structure

- `common/` — shared data types and utilities used across crates.
- `node_lib/` — core library implementing node behaviour (routing, control).
- `node/` — binary crate that runs a single node (for real hardware or testing).
- `simulator/` — binary crate that orchestrates multiple nodes in network
  namespaces and exposes an HTTP control API.
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

Build the simulator with the optional webview feature (enables UI integration):

```sh
cargo build --bin simulator --release --features simulator/webview
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
sudo RUST_LOG="node=debug" ./target/release/simulator --config-file simulator.yaml --pretty
```

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

Run tests for the `node_lib` crate only (useful when iterating on routing):

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

---

If you'd like, I can:

- add a short example that wires a full `simulator.yaml` with 3 nodes and a
  sample run script, or
- add module-level `///` Rust doc comments to `node_lib` and `common` to improve
  API documentation.
