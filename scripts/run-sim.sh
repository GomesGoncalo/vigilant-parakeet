#!/usr/bin/env bash
# Simple helper to build and run the simulator with sudo (assumes repo root)
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT_DIR"

# Build the simulator release binary
cargo build --bin simulator --release --features "webview,tui,mobility"

sudo RUST_LOG="info" "$ROOT_DIR/target/release/simulator" --config-file "$CONFIG" --pretty --tui
