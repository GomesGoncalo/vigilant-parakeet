#!/usr/bin/env bash
set -euo pipefail
# Run cargo fmt (check or fix) and cargo clippy (unless skipped).

usage() {
  cat <<EOF
Usage: $0 [--fix] [--no-clippy]

--fix       Run `cargo fmt` to apply formatting. Otherwise runs `cargo fmt -- --check`.
--no-clippy Skip running cargo clippy.
EOF
}

FIX=0
NO_CLIPPY=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --fix) FIX=1; shift;;
    --no-clippy) NO_CLIPPY=1; shift;;
    -h|--help) usage; exit 0;;
    *) echo "Unknown arg: $1"; usage; exit 2;;
  esac
done

if [[ $FIX -eq 1 ]]; then
  echo "Running: cargo fmt"
  cargo fmt
else
  echo "Running: cargo fmt -- --check"
  cargo fmt -- --check
fi

if [[ $NO_CLIPPY -eq 0 ]]; then
  echo "Running: cargo clippy --workspace --all-targets -- -D warnings"
  cargo clippy --workspace --all-targets -- -D warnings
else
  echo "Skipping clippy as requested."
fi

echo "Linting and formatting done."
