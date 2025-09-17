#!/usr/bin/env bash
set -euo pipefail
# CI wrapper: strict, fast checks used in CI pipelines.
# Runs: fmt check, clippy -D warnings, cargo build --workspace --release, cargo test --workspace
# Optional: --no-tests, --no-clippy, --coverage

usage() {
  cat <<EOF
Usage: $0 [--no-tests] [--no-clippy] [--coverage]

Flags:
  --no-tests    Skip running tests (faster, but only for special CI steps)
  --no-clippy   Skip running clippy
  --no-coverage Skip running coverage (may be slow) using scripts/coverage.sh
EOF
}

NO_TESTS=0
NO_CLIPPY=0
NO_COVERAGE=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --no-tests) NO_TESTS=1; shift;;
    --no-clippy) NO_CLIPPY=1; shift;;
    --no-coverage) NO_COVERAGE=1; shift;;
    -h|--help) usage; exit 0;;
    *) echo "Unknown arg: $1"; usage; exit 2;;
  esac
done

echo "1) Formatting check"
cargo fmt -- --check

if [[ $NO_CLIPPY -eq 0 ]]; then
  echo "2) Running clippy (this will fail on warnings)"
  cargo clippy --workspace --all-targets -- -D warnings
else
  echo "2) Skipping clippy as requested."
fi

echo "3) Building workspace (release)"
cargo build --workspace --release

if [[ $NO_TESTS -eq 0 ]]; then
  echo "4) Running tests (workspace)"
  # Run tests; avoid very slow integration by default - projects in this repo are fast per CONTRIBUTING.
  cargo test --workspace
else
  echo "4) Skipping tests as requested."
fi

if [[ $NO_COVERAGE -eq 0 ]]; then
  echo "5) Running coverage (this may take >1m)"
  ./scripts/coverage.sh --out lcov --packages "common,node_lib,rsu_lib,obu_lib" --timeout 180
else
  echo "5) Coverage step skipped."
fi

echo "CI checks passed."
