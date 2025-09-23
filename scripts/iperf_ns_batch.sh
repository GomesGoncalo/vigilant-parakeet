#!/usr/bin/env bash
set -euo pipefail

# Run iperf3 tests for multiple namespace pairs and collect results.
# Input CSV format (header optional): src_ns,dst_ns[,time_seconds[,repeat]]
# Output: /tmp/iperf_ns_batch_RESULTS.csv with columns: src,dst,time,repeat,bandwidth_mbits,raw_log

OUT_CSV="/tmp/iperf_ns_batch_RESULTS.csv"
TMP_DIR="/tmp/iperf_ns_batch"
mkdir -p "$TMP_DIR"

usage() {
  cat <<EOF
Usage: $(basename "$0") pairs.csv

pairs.csv format: src_ns,dst_ns[,time_seconds[,repeat]]
Example:
  ns1,ns2,10,3
  ns3,ns4
EOF
}

if [[ ${#@} -lt 1 ]]; then
  usage
  exit 1
fi

INPUT="$1"

echo "src,dst,time,repeat,bandwidth_mbits,raw_log" >"$OUT_CSV"

while IFS=, read -r SRC DST TIME REPEAT; do
  # skip empty/comment lines
  [[ -z "${SRC// /}" ]] && continue
  [[ $SRC == \#* ]] && continue

  TIME=${TIME:-10}
  REPEAT=${REPEAT:-1}

  for ((i = 1; i <= REPEAT; i++)); do
    LOG="$TMP_DIR/${SRC}_${DST}_${TIME}s_run${i}.log"
    echo "Running iperf3: $SRC -> $DST (${TIME}s) (run $i/$REPEAT)"
    # start server in dst
    sudo ip netns exec "$DST" iperf3 -s -p 5201 >"$LOG" 2>&1 &
    SERVER_PID=$!
    sleep 0.3
    # detect dst ip inside namespace
    DST_IP=$(sudo ip netns exec "$DST" ip -4 addr show scope global 2>/dev/null | awk '/inet/ {print $2}' | head -n1 || true)
    if [[ -z "$DST_IP" ]]; then
      DST_IP=$(sudo ip netns exec "$DST" ip -4 addr show 2>/dev/null | awk '/inet/ {print $2}' | head -n1 || true)
    fi
    DST_IP=${DST_IP%%/*}
    DST_IP=${DST_IP:-127.0.0.1}

    # run client with JSON output for reliable parsing
    set +e
    JSON_OUT="$TMP_DIR/${SRC}_${DST}_${TIME}s_run${i}.json"
    sudo ip netns exec "$SRC" iperf3 -c "$DST_IP" -p 5201 -t "$TIME" --json >"$JSON_OUT" 2>"$LOG"
    CLIENT_EXIT=$?
    set -e

    # parse JSON for bits_per_second (try sum stream then end summary)
    BAND=0
    BAND=$(./target/debug/scripts_tools parseband "${JSON_OUT}" 2>/dev/null || echo 0)
    BAND=${BAND:-0}

    echo "$SRC,$DST,$TIME,$i,$BAND,$LOG" >>"$OUT_CSV"

    # cleanup server
    if kill -0 "$SERVER_PID" 2>/dev/null; then
      kill "$SERVER_PID" || true
      wait "$SERVER_PID" 2>/dev/null || true
    fi
  done
done <"$INPUT"

echo "Results written to $OUT_CSV"

# Also write JSON output for easier downstream parsing and a summary CSV
JSON_OUT="/tmp/iperf_ns_batch_RESULTS.json"
SUMMARY_CSV="/tmp/iperf_ns_batch_SUMMARY.csv"
./target/debug/scripts_tools buildsummary "$OUT_CSV" "$JSON_OUT" "$SUMMARY_CSV"
