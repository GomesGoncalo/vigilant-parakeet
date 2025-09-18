#!/usr/bin/env bash
set -euo pipefail

# Query simulator /node_info and generate CSV lines for every OBU -> RSU pair.
# Usage: ./scripts/generate_obu_rsu_pairs.sh [output.csv] [time] [repeat]

OUT=${1:--}   # '-' means stdout
TIME=${2:-10}
REPEAT=${3:-1}

API_URL=${SIM_API_URL:-http://127.0.0.1:3030/node_info}

if ! command -v curl >/dev/null 2>&1; then
  echo "curl is required" >&2; exit 1
fi

JSON=$(curl -sSf "$API_URL")

# Write JSON to a temp file and run the Rust helper to generate pairs
TMP_JSON=$(mktemp)
trap 'rm -f "$TMP_JSON"' EXIT
printf '%s' "$JSON" >"$TMP_JSON"

# Prefer already-built binary in scripts_tools target, otherwise use cargo run
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
BIN_RELEASE="$ROOT_DIR/target/release/scripts_tools"
BIN_DEBUG="$ROOT_DIR/target/debug/scripts_tools"
if [[ -x "$BIN_RELEASE" ]]; then
  PAIRS=("$BIN_RELEASE" generatepairs "$TMP_JSON" "$TIME" "$REPEAT")
  PAIRS=$("${PAIRS[@]}")
elif [[ -x "$BIN_DEBUG" ]]; then
  PAIRS=("$BIN_DEBUG" generatepairs "$TMP_JSON" "$TIME" "$REPEAT")
  PAIRS=$("${PAIRS[@]}")
else
  # Build release binary first so sudo runs don't require rustup/toolchain
  (cd "$ROOT_DIR/" && cargo build --release) || { echo "Failed to build scripts_tools release" >&2; exit 1; }
  PAIRS=("$BIN_RELEASE" generatepairs "$TMP_JSON" "$TIME" "$REPEAT")
  PAIRS=$("${PAIRS[@]}")
fi

if [[ "$OUT" != "-" ]]; then
  printf '%s\n' "$PAIRS" >"$OUT"
  echo "Wrote $OUT"
else
  printf '%s\n' "$PAIRS"
fi
