#!/usr/bin/env bash
set -euo pipefail

dry_run=false
backup=false
ip_cidr=""
default_latency=""
default_loss=""
all_flag=false
args=()
alloc_db=""
start_offset=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run) dry_run=true; shift ;;
    --backup) backup=true; shift ;;
  --ip-cidr) ip_cidr="$2"; shift 2 ;;
  --default-latency) default_latency="$2"; shift 2 ;;
  --default-loss) default_loss="$2"; shift 2 ;;
  --all) all_flag=true; shift ;;
  --alloc-db) alloc_db="$2"; shift 2 ;;
  --start-offset) start_offset="$2"; shift 2 ;;
    --) shift; while [[ $# -gt 0 ]]; do args+=("$1"); shift; done; break ;;
    *) args+=("$1"); shift ;;
  esac
done

if [[ ${#args[@]} -eq 0 ]]; then
  echo "Usage: $0 [--dry-run] [--backup] <file...>" >&2
  exit 2
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
BIN_RELEASE="$ROOT_DIR/scripts_tools/target/release/scripts_tools"

# resolve paths
abs_args=()
for p in "${args[@]}"; do
  abs=$(readlink -f -- "$p" 2>/dev/null)
  abs_args+=("$abs")
done

if [[ -x "$BIN_RELEASE" ]]; then
  "$BIN_RELEASE" autofixconfigs --dry-run=$dry_run --backup=$backup ${ip_cidr:+--ip-cidr "$ip_cidr"} ${default_latency:+--default-latency "$default_latency"} ${default_loss:+--default-loss "$default_loss"} ${all_flag:+--all} ${alloc_db:+--alloc-db "$alloc_db"} ${start_offset:+--start-offset "$start_offset"} "${abs_args[@]}"
else
  (cd "$ROOT_DIR/scripts_tools" && cargo run --quiet -- autofixconfigs --dry-run=$dry_run --backup=$backup ${ip_cidr:+--ip-cidr "$ip_cidr"} ${default_latency:+--default-latency "$default_latency"} ${default_loss:+--default-loss "$default_loss"} ${all_flag:+--all} ${alloc_db:+--alloc-db "$alloc_db"} ${start_offset:+--start-offset "$start_offset"} "${abs_args[@]}")
fi
