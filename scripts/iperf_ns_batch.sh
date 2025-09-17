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

  for ((i=1;i<=REPEAT;i++)); do
    LOG="$TMP_DIR/${SRC}_${DST}_${TIME}s_run${i}.log"
    echo "Running iperf3: $SRC -> $DST (${TIME}s) (run $i/$REPEAT)"
    # start server in dst
    ip netns exec "$DST" iperf3 -s -p 5201 >"$LOG" 2>&1 &
    SERVER_PID=$!
    sleep 0.3
    # detect dst ip inside namespace
    DST_IP=$(ip netns exec "$DST" ip -4 addr show scope global 2>/dev/null | awk '/inet/ {print $2}' | head -n1 || true)
    if [[ -z "$DST_IP" ]]; then
      DST_IP=$(ip netns exec "$DST" ip -4 addr show 2>/dev/null | awk '/inet/ {print $2}' | head -n1 || true)
    fi
    DST_IP=${DST_IP%%/*}
    DST_IP=${DST_IP:-127.0.0.1}

  # run client with JSON output for reliable parsing
  set +e
  JSON_OUT="$TMP_DIR/${SRC}_${DST}_${TIME}s_run${i}.json"
  ip netns exec "$SRC" iperf3 -c "$DST_IP" -p 5201 -t "$TIME" --json >"$JSON_OUT" 2>"$LOG"
  CLIENT_EXIT=$?
  set -e

  # parse JSON for bits_per_second (try sum stream then end summary)
  BAND=0
  if command -v python3 >/dev/null 2>&1; then
    BAND=$(python3 - <<PYJSON
import json,sys
try:
  j=json.load(open('${JSON_OUT}'))
  # Try to find receiver sum (client side reports end.sum_received)
  b=None
  if 'end' in j and 'sum_received' in j['end'] and 'bits_per_second' in j['end']['sum_received']:
    b=j['end']['sum_received']['bits_per_second']
  elif 'end' in j and 'sum_sent' in j['end'] and 'bits_per_second' in j['end']['sum_sent']:
    b=j['end']['sum_sent']['bits_per_second']
  elif 'intervals' in j and len(j['intervals'])>0 and 'sum' in j['intervals'][-1] and 'bits_per_second' in j['intervals'][-1]['sum']:
    b=j['intervals'][-1]['sum']['bits_per_second']
  if b is None:
    print('0')
  else:
    # convert to Mbits/s
    print(round(b/1e6,3))
except Exception as e:
  print('0')
PYJSON
)
  else
    # fallback to old parsing if python3 unavailable
    BAND=$(awk '/SUM|receiver|sender/ {b=$0} END{print b}' "$LOG" | awk '{print $(NF-1)" "$(NF)}' | sed 's/ //g' | sed 's/Mbits\/sec/ /g' | awk '{print $1}' )
  fi
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
if command -v python3 >/dev/null 2>&1; then
  python3 - <<PYCSVJSON
import csv, json, statistics
from collections import defaultdict
rows=[]
with open('$OUT_CSV','r') as f:
  r=csv.DictReader(f)
  for row in r:
    # convert numeric fields
    for k in ('time','repeat'):
      if row.get(k) is not None and row[k] != '':
        try:
          row[k]=int(row[k])
        except:
          pass
    if row.get('bandwidth_mbits') is not None and row['bandwidth_mbits'] != '':
      try:
        row['bandwidth_mbits']=float(row['bandwidth_mbits'])
      except:
        row['bandwidth_mbits']=0.0
    else:
      row['bandwidth_mbits']=0.0
    rows.append(row)

# build summaries grouped by (src,dst,time)
groups=defaultdict(list)
for r in rows:
  key=(r['src'], r['dst'], r['time'])
  groups[key].append(r['bandwidth_mbits'])

summary=[]
for (src,dst,time), vals in groups.items():
  mean=statistics.mean(vals) if len(vals)>0 else 0.0
  stddev=statistics.stdev(vals) if len(vals)>1 else 0.0
  summary.append({
    'src': src,
    'dst': dst,
    'time': time,
    'samples': len(vals),
    'mean_mbits': round(mean,3),
    'stddev_mbits': round(stddev,3),
    'min_mbits': round(min(vals),3) if vals else 0.0,
    'max_mbits': round(max(vals),3) if vals else 0.0,
  })

# write JSON with runs and summary
with open('$JSON_OUT','w') as jf:
  json.dump({'runs': rows, 'summary': summary}, jf, indent=2)

# write summary CSV
with open('$SUMMARY_CSV','w',newline='') as sf:
  w=csv.DictWriter(sf, fieldnames=['src','dst','time','samples','mean_mbits','stddev_mbits','min_mbits','max_mbits'])
  w.writeheader()
  for s in summary:
    w.writerow(s)

print('JSON results written to $JSON_OUT')
print('Summary CSV written to $SUMMARY_CSV')
PYCSVJSON
else
  echo "python3 not found; skipping JSON and summary output."
fi
