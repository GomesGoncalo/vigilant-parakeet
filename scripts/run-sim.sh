#!/usr/bin/env bash
# Simple helper to build and run the simulator with sudo (assumes repo root)
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT_DIR"

# Build the simulator release binary
cargo build --bin simulator --release --features "webview,tui,mobility"

# Raise the open-file limit inside sudo — it resets to the system default
# (typically 1024), which is too low for large simulations (each node opens
# namespaces, TUN devices, sockets, and timerfd handles).
sudo bash -c "ulimit -n 65536 && RUST_LOG='info' '$ROOT_DIR/target/release/simulator' --config-file '$CONFIG' --pretty --tui"
