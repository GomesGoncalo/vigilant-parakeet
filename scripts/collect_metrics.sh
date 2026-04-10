#!/usr/bin/env bash
# Polls the simulator /metrics endpoint and appends JSON lines to an output file.
#
# Usage:
#   ./scripts/collect_metrics.sh <api_url> <output_file> [interval_ms]
#
# Arguments:
#   api_url      Base URL of the simulator HTTP API (e.g. http://localhost:3030/metrics)
#   output_file  Path to output file; each poll appends one JSON line with a timestamp
#   interval_ms  Polling interval in milliseconds (default: 500)
#
# Example:
#   ./scripts/collect_metrics.sh http://localhost:3030/metrics results.jsonl
#   ./scripts/collect_metrics.sh http://localhost:3030/metrics nakagami_results.json 100

set -euo pipefail

if [[ $# -lt 2 ]]; then
    echo "Usage: $0 <api_url> <output_file> [interval_ms]" >&2
    exit 1
fi

API_URL="$1"
OUTPUT_FILE="$2"
INTERVAL_MS="${3:-500}"
INTERVAL_SEC="$(echo "scale=3; $INTERVAL_MS / 1000" | bc)"

echo "Polling $API_URL every ${INTERVAL_MS}ms → $OUTPUT_FILE"
echo "Press Ctrl+C to stop."

while true; do
    ts="$(date -u +%s%3N)"
    response="$(curl -s --max-time 1 "$API_URL" 2>/dev/null || echo '{}')"
    echo "{\"ts_ms\":$ts,$( echo "$response" | sed 's/^{//' )}" >> "$OUTPUT_FILE"
    sleep "$INTERVAL_SEC"
done
