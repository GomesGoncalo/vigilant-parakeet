#!/usr/bin/env bash
set -euo pipefail
# Run cargo-tarpaulin to generate coverage. Defaults to lcov output for common, node_lib, rsu_lib and obu_lib crates.

usage() {
  cat <<EOF
Usage: $0 [--out lcov|json] [--packages "p1,p2"] [--timeout N]

Example: $0 --out lcov --packages "common,node_lib,rsu_lib,obu_lib" --timeout 120
EOF
}

OUT="lcov"
PACKAGES="common,node_lib,rsu_lib,obu_lib"
TIMEOUT=120

while [[ $# -gt 0 ]]; do
  case "$1" in
    --out) OUT="$2"; shift 2;;
    --packages) PACKAGES="$2"; shift 2;;
    --timeout) TIMEOUT="$2"; shift 2;;
    -h|--help) usage; exit 0;;
    *) echo "Unknown arg: $1"; usage; exit 2;;
  esac
done

if ! command -v cargo-tarpaulin >/dev/null 2>&1; then
  echo "cargo-tarpaulin not found. Installing..."
  cargo install cargo-tarpaulin --locked
fi

IFS=',' read -r -a PKG_ARR <<< "$PACKAGES"
CMD=(cargo tarpaulin)
for p in "${PKG_ARR[@]}"; do
  CMD+=( -p "$p" )
done
CMD+=(--out "$OUT" --timeout "$TIMEOUT")

echo "Running: ${CMD[*]}"
eval "${CMD[*]}"

echo "Coverage finished. Output in ./tarpaulin.":
