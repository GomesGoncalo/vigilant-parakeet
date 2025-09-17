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

# After iperf batch completes, augment the generated JSON with latency measurements.
# This calls `scripts/measure-latency.sh` for each unique src->dst pair (10 pings,
# 0.2s interval) and inserts an averaged `latency_ms` into each run entry in
# `/tmp/iperf_ns_batch_RESULTS.json`. Also appends a top-level `latencies` list
# with summaries per (src,dst,time). If `measure-latency.sh` is missing or
# non-executable, this step is skipped.

IPERF_JSON="/tmp/iperf_ns_batch_RESULTS.json"
LATENCY_TMP_DIR="/tmp/measure_latency"
mkdir -p "$LATENCY_TMP_DIR"

if [[ -f "$IPERF_JSON" && -x ./scripts/measure-latency.sh ]]; then
  echo "Measuring latency for pairs listed in $IPERF_JSON"
  # We'll call measure-latency.sh with count=10 and interval=0.2 and parse the average
  python3 - <<PYMERGE
import json,subprocess,sys,os

ipf = '$IPERF_JSON'
tmpd = '$LATENCY_TMP_DIR'
with open(ipf,'r') as f:
	j = json.load(f)

runs = j.get('runs', [])

def measure_avg(src,dst):
	# call the measure-latency.sh script; if dst looks like a namespace, pass as-is
	cmd = ['./scripts/measure-latency.sh','-s',src,'-d',dst,'-c','10','-i','0.2']
	try:
		out = subprocess.check_output(cmd, stderr=subprocess.STDOUT, text=True)
	except subprocess.CalledProcessError as e:
		out = e.output
	# parse for lines like 'rtt min/avg/max/mdev = 0.042/0.043/0.045/0.001 ms' (from ping)
	for line in out.splitlines()[::-1]:
		if 'rtt min/avg' in line or 'round-trip min/avg' in line:
			parts = line.split('=')[-1].strip().split()[0]
			vals = parts.split('/')
			if len(vals) >= 2:
				try:
					return float(vals[1])
				except:
					return None
	# Fallback: try to find 'avg' in other formats
	for line in out.splitlines()[::-1]:
		if 'avg' in line and '/' in line:
			try:
				return float(line.split('/')[1])
			except:
				pass
	return None

# Measure latency per unique pair (src,dst,time) and attach to runs
latencies = {}
for r in runs:
	key = (r.get('src'), r.get('dst'), str(r.get('time')))
	if key in latencies:
		continue
	src, dst = r.get('src'), r.get('dst')
	print('Measuring', src, '->', dst)
	avg = measure_avg(src,dst)
	latencies[key] = avg

# attach latency_ms field to runs where available
for r in runs:
	key = (r.get('src'), r.get('dst'), str(r.get('time')))
	avg = latencies.get(key)
	if avg is not None:
		r['latency_ms'] = round(avg,3)

# merge latency into the existing summary entries (if present)
summary = j.get('summary', [])
for s in summary:
	key = (s.get('src'), s.get('dst'), str(s.get('time')))
	avg = latencies.get(key)
	if avg is not None:
		s['latency_ms'] = round(avg,3)
	else:
		s['latency_ms'] = None

# write back the possibly-updated summary
j['summary'] = summary

with open(ipf,'w') as f:
	json.dump(j,f,indent=2)

print('Latency measurements merged into', ipf)
PYMERGE
else
  echo "Either $IPERF_JSON does not exist or scripts/measure-latency.sh is not executable; skipping latency measurements."
fi

echo "Done. See /tmp/iperf_ns_batch_* for results and summary."
