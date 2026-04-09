#!/usr/bin/env bash
# collect_metrics.sh — poll the simulator /metrics endpoint and write results to a JSON file.
#
# Usage:
#   ./scripts/collect_metrics.sh <metrics_url> <output_file> [interval_ms] [duration_s]
#
# Arguments:
#   metrics_url   Full URL of the /metrics endpoint (e.g. http://localhost:3030/metrics)
#   output_file   Path for the output JSON lines file
#   interval_ms   Polling interval in milliseconds (default: 100)
#   duration_s    Total collection duration in seconds (default: 60)
#
# Output format:
#   One JSON object per line, each containing a "timestamp_ms" field (milliseconds
#   since collection start) and the full parsed metrics payload from the simulator.
#
# Example:
#   ./scripts/collect_metrics.sh http://localhost:3030/metrics results.json 100 60

set -euo pipefail

METRICS_URL="${1:?Usage: $0 <metrics_url> <output_file> [interval_ms] [duration_s]}"
OUTPUT_FILE="${2:?Usage: $0 <metrics_url> <output_file> [interval_ms] [duration_s]}"
INTERVAL_MS="${3:-100}"
DURATION_S="${4:-60}"

INTERVAL_S=$(echo "scale=3; $INTERVAL_MS / 1000" | bc)
END_TIME=$(( $(date +%s) + DURATION_S ))
START_MS=$(date +%s%3N)

echo "Collecting metrics from $METRICS_URL"
echo "  Output:   $OUTPUT_FILE"
echo "  Interval: ${INTERVAL_MS} ms"
echo "  Duration: ${DURATION_S} s"

> "$OUTPUT_FILE"

while [ "$(date +%s)" -lt "$END_TIME" ]; do
    NOW_MS=$(date +%s%3N)
    ELAPSED_MS=$(( NOW_MS - START_MS ))

    PAYLOAD=$(curl --silent --max-time 1 "$METRICS_URL" 2>/dev/null || echo "null")

    if [ "$PAYLOAD" != "null" ] && [ -n "$PAYLOAD" ]; then
        printf '{"timestamp_ms":%d,"metrics":%s}\n' "$ELAPSED_MS" "$PAYLOAD" >> "$OUTPUT_FILE"
    fi

    sleep "$INTERVAL_S"
done

LINES=$(wc -l < "$OUTPUT_FILE")
echo "Collection complete: $LINES samples written to $OUTPUT_FILE"
