#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$ROOT_DIR/scripts_tools/target/release/scripts_tools"
DB_PATH="$ROOT_DIR/.alloc_db.json"

echo "Running autofix dry-run for all examples..."
if [ -x "$BIN" ]; then
  "$BIN" autofixconfigs --dry-run --all --ip-cidr 10.0.0.0/24 --alloc-db "$DB_PATH"
else
  (cd "$ROOT_DIR/scripts_tools" && cargo run --quiet -- autofixconfigs --dry-run --all --ip-cidr 10.0.0.0/24 --alloc-db "$DB_PATH")
fi

read -p "Apply these changes? [y/N] " ans
if [[ "$ans" =~ ^[Yy]$ ]]; then
  echo "Applying fixes..."
  if [ -x "$BIN" ]; then
    "$BIN" autofixconfigs --backup --all --ip-cidr 10.0.0.0/24 --alloc-db "$DB_PATH"
  else
    (cd "$ROOT_DIR/scripts_tools" && cargo run --quiet -- autofixconfigs --backup --all --ip-cidr 10.0.0.0/24 --alloc-db "$DB_PATH")
  fi
  echo "Applied and saved alloc DB to $DB_PATH"
else
  echo "Aborted; no changes applied."
fi
