#!/usr/bin/env bash
# Simple helper to build and run the simulator with sudo (assumes repo root)
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT_DIR"

# Build the simulator release binary
cargo build --bin simulator --release

# Prefer examples/simulator.yaml if present
if [ -f "examples/simulator.yaml" ]; then
  CONFIG="examples/simulator.yaml"
elif [ -f "simulator.yaml" ]; then
  CONFIG="simulator.yaml"
else
  echo "Config file 'examples/simulator.yaml' or 'simulator.yaml' not found. Create one based on README examples." >&2
  exit 1
fi

# Run simulator (sudo required for netns)
sudo RUST_LOG="node=debug" "$ROOT_DIR/target/release/simulator" --config-file "$CONFIG" --pretty
