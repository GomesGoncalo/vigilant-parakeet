#!/usr/bin/env bash
set -euo pipefail

# Generate all OBU->RSU pairs from simulator and run iperf_ns_batch on them.
# Usage: sudo scripts/run_all_obu_rsu.sh [pairs_out.csv] [time] [repeat]

PAIRS_FILE=${1:-/tmp/obu_rsu_pairs.csv}
TIME=${2:-10}
REPEAT=${3:-1}

echo "Generating OBU->RSU pairs to $PAIRS_FILE (time=$TIME repeat=$REPEAT)"
./scripts/generate_obu_rsu_pairs.sh "$PAIRS_FILE" "$TIME" "$REPEAT"

echo "Running batch iperf on $PAIRS_FILE"
sudo ./scripts/iperf_ns_batch.sh "$PAIRS_FILE"

echo "Done. See /tmp/iperf_ns_batch_* for results and summary."
