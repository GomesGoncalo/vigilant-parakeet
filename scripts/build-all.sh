#!/usr/bin/env bash
set -euo pipefail
# Build the workspace or a specific package with optional features and release mode.

usage() {
  cat <<EOF
Usage: $0 [--package NAME] [--features "f1 f2"] [--release]

Examples:
  $0 --release --features "webview"
  $0 --package simulator
EOF
}

PACKAGE=""
FEATURES=""
RELEASE=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --package) PACKAGE="$2"; shift 2;;
    --features) FEATURES="$2"; shift 2;;
    --release) RELEASE=1; shift;;
    -h|--help) usage; exit 0;;
    *) echo "Unknown arg: $1"; usage; exit 2;;
  esac
done

CMD=(cargo build)
if [[ $RELEASE -eq 1 ]]; then
  CMD+=(--release)
fi
if [[ -n "$FEATURES" ]]; then
  CMD+=(--features "$FEATURES")
fi
if [[ -n "$PACKAGE" ]]; then
  CMD+=(--package "$PACKAGE")
fi

echo "Running: ${CMD[*]}"
eval "${CMD[*]}"

echo "Build finished."
