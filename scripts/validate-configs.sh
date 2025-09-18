#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 ]]; then
  echo "Usage: $0 <file1> [file2 ...]" >&2
  exit 2
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
BIN_RELEASE="$ROOT_DIR/scripts_tools/target/release/scripts_tools"

abs_args=()
for p in "$@"; do
  abs=$(readlink -f -- "$p" 2>/dev/null || realpath -- "$p")
  abs_args+=("$abs")
done

if [[ -x "$BIN_RELEASE" ]]; then
  "$BIN_RELEASE" validateconfigs "${abs_args[@]}"
else
  (cd "$ROOT_DIR/scripts_tools" && cargo run --quiet -- validateconfigs "${abs_args[@]}")
fi
