#!/usr/bin/env bash
set -euo pipefail

# Generate all OBU->RSU pairs from simulator and run iperf_ns_batch on them.
# Usage: sudo scripts/run_all_obu_rsu.sh [pairs_out.csv] [time] [repeat]

PAIRS_FILE=${1:-/tmp/obu_rsu_pairs.csv}
TIME=${2:-10}
REPEAT=${3:-1}

cargo build -p scripts_tools --release --quiet

echo "Generating OBU->RSU pairs to $PAIRS_FILE (time=$TIME repeat=$REPEAT)"
./scripts/generate_obu_rsu_pairs.sh "$PAIRS_FILE" "$TIME" "$REPEAT"

echo "Running batch iperf on $PAIRS_FILE"
./scripts/iperf_ns_batch.sh "$PAIRS_FILE"

IPERF_JSON="/tmp/iperf_ns_batch_RESULTS.json"
LATENCY_TMP_DIR="/tmp/measure_latency"
mkdir -p "$LATENCY_TMP_DIR"

if [[ -f "$IPERF_JSON" ]]; then
  echo "Measuring latency for pairs listed in $IPERF_JSON"
  ./target/release/scripts_tools mergelatency "$IPERF_JSON"
else
  echo "$IPERF_JSON does not exist; skipping latency measurements."
fi

echo "Done. See /tmp/iperf_ns_batch_* for results and summary."
