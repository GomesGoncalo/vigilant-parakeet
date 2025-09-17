#!/usr/bin/env bash
set -euo pipefail
# Run cargo tests across the workspace or a package, optionally with features or a test filter.

usage() {
  cat <<EOF
Usage: $0 [--package NAME] [--features "f1 f2"] [--filter TESTNAME]

Examples:
  $0 --package node_lib
  $0 --features stats --filter integration_two_hop
EOF
}

PACKAGE=""
FEATURES=""
FILTER=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --package) PACKAGE="$2"; shift 2;;
    --features) FEATURES="$2"; shift 2;;
    --filter) FILTER="$2"; shift 2;;
    -h|--help) usage; exit 0;;
    *) echo "Unknown arg: $1"; usage; exit 2;;
  esac
done

CMD=(cargo test)
if [[ -n "$FEATURES" ]]; then
  CMD+=(--features "$FEATURES")
fi
if [[ -n "$PACKAGE" ]]; then
  CMD+=(--package "$PACKAGE")
fi
if [[ -n "$FILTER" ]]; then
  CMD+=("$FILTER")
fi

echo "Running: ${CMD[*]}"
eval "${CMD[*]}"

echo "Tests finished."
