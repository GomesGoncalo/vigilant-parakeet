#!/usr/bin/env bash
set -euo pipefail

# Query simulator /node_info and generate CSV lines for every OBU -> RSU pair.
# Usage: ./scripts/generate_obu_rsu_pairs.sh [output.csv] [time] [repeat]

OUT=${1:--}   # '-' means stdout
TIME=${2:-10}
REPEAT=${3:-1}

API_URL=${SIM_API_URL:-http://127.0.0.1:3030/node_info}

if ! command -v curl >/dev/null 2>&1; then
  echo "curl is required" >&2; exit 1
fi

if ! command -v python3 >/dev/null 2>&1; then
  echo "python3 is required" >&2; exit 1
fi

JSON=$(curl -sSf "$API_URL")

# Write JSON to a temp file and run a small Python script to generate pairs
TMP_JSON=$(mktemp)
TMP_PY=$(mktemp --suffix=.py)
trap 'rm -f "$TMP_JSON" "$TMP_PY"' EXIT
printf '%s' "$JSON" >"$TMP_JSON"
cat >"$TMP_PY" <<'PY'
import sys, json
json_path=sys.argv[1]
TIME=sys.argv[2]
REPEAT=sys.argv[3]
data=json.load(open(json_path))
obus=[n for n,info in data.items() if info.get('node_type')=='Obu']
rsus=[n for n,info in data.items() if info.get('node_type')=='Rsu']
out=[]
for o in obus:
    for r in rsus:
        out.append(f"sim_ns_{o},sim_ns_{r},{TIME},{REPEAT}")
print('\n'.join(out))
PY

PAIRS=$(python3 "$TMP_PY" "$TMP_JSON" "$TIME" "$REPEAT")

if [[ "$OUT" != "-" ]]; then
  printf '%s\n' "$PAIRS" >"$OUT"
  echo "Wrote $OUT"
else
  printf '%s\n' "$PAIRS"
fi
