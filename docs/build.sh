#!/usr/bin/env bash
# thesis/build.sh — build the thesis PDF
#
# Usage:
#   ./thesis/build.sh          # one-shot build → thesis/output/main.pdf
#   ./thesis/build.sh --watch  # watch mode (rebuild on file change)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUTPUT_DIR="$SCRIPT_DIR/output"

mkdir -p "$OUTPUT_DIR"

if [[ "${1:-}" == "--watch" ]]; then
  echo "Watching for changes…"
  exec typst watch "$SCRIPT_DIR/main.typ" "$OUTPUT_DIR/main.pdf"
else
  echo "Building thesis…"
  typst compile "$SCRIPT_DIR/main.typ" "$OUTPUT_DIR/main.pdf"
  echo "Output: $OUTPUT_DIR/main.pdf"
fi
