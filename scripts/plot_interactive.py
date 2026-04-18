#!/usr/bin/env python3
"""
plot_interactive.py

Read metrics CSV (or stdin) and generate an interactive Plotly HTML file with
horizontal scrolling (range slider + pan/zoom).

Usage:
  python3 scripts/plot_interactive.py --in /tmp/metrics.csv --out /tmp/metrics_interactive.html --open
  cat /tmp/metrics.csv | python3 scripts/plot_interactive.py --in - --out /tmp/m.html --open

Columns expected (header): iso_ts,unixts,obu,hops,rssi_dbm,route_latency_ms,ping_rtt_ms
"""
import sys
import csv
import json
import argparse
import webbrowser
from datetime import datetime


def parse_args():
    p = argparse.ArgumentParser(description="Generate interactive Plotly HTML from metrics CSV")
    p.add_argument('--in', dest='infile', default='-', help='Input CSV file (default stdin). Use - for stdin')
    p.add_argument('--out', dest='outfile', default='/tmp/metrics_interactive.html', help='Output HTML file')
    p.add_argument('--open', dest='open_browser', action='store_true', help='Open the resulting HTML in the default browser')
    p.add_argument('--use-webgl', dest='use_webgl', action='store_true', help='Render traces using WebGL (scattergl) for better performance')
    p.add_argument('--downsample', dest='downsample', type=int, default=0, help='Downsample to at most N points (uniform decimation) for high-volume data')
    return p.parse_args()


def read_csv(fobj):
    reader = csv.DictReader(fobj)
    times = []
    hops = []
    rssi = []
    route_latency = []
    ping_rtt = []
    for row in reader:
        ts = row.get('iso_ts') or row.get('time') or row.get('timestamp')
        if not ts:
            continue
        try:
            # Parse ISO8601 (should be UTC ending with Z)
            dt = datetime.fromisoformat(ts.replace('Z', '+00:00'))
            ms = int(dt.timestamp() * 1000)
        except Exception:
            # fallback: use unixts column if present
            try:
                ms = int(float(row.get('unixts', '0')) * 1000)
            except Exception:
                continue
        times.append(ms)
        def maybe_float(k):
            v = row.get(k, '')
            if v is None or v == '' or v.lower() in ('na','nan'):
                return None
            try:
                return float(v)
            except Exception:
                return None
        hops.append(maybe_float('hops'))
        rssi.append(maybe_float('rssi_dbm'))
        route_latency.append(maybe_float('route_latency_ms'))
        ping_rtt.append(maybe_float('ping_rtt_ms'))
    return {
        'times': times,
        'hops': hops,
        'rssi': rssi,
        'route_latency': route_latency,
        'ping_rtt': ping_rtt,
    }


HTML_TEMPLATE = """
<!doctype html>
<html>
<head>
  <meta charset="utf-8" />
  <title>Metrics Interactive Plot</title>
  <script src="https://cdn.plot.ly/plotly-latest.min.js"></script>
  <style> body { font-family: Arial, sans-serif; margin: 8px; } #plot { width: 100%; height: 700px; }</style>
</head>
<body>
<h3>Metrics interactive plot</h3>
<div id="plot"></div>
<script>
const times = %%TIMES%%;
const hops = %%HOPS%%;
const rssi = %%RSSI%%;
const route_latency = %%ROUTE_LATENCY%%;
const ping_rtt = %%PING_RTT%%;

const time_strings = times.map(t => new Date(t).toISOString());

// Compute a helper max value of present numeric latencies/RTTs for placing loss markers
(function(){
  const presentLatency = route_latency.filter(v => v !== null && !isNaN(v));
  const presentPing = ping_rtt.filter(v => v !== null && !isNaN(v));
  const all = presentLatency.concat(presentPing);
  window.__max_val = all.length ? Math.max.apply(null, all) : 1.0;
})();

// Build arrays for lost pings (timestamps + hover texts)
const lost_ping_times = [];
const lost_ping_texts = [];
ping_rtt.forEach((v, i) => {
  if (v === null || (typeof v === 'number' && isNaN(v))) {
    lost_ping_times.push(times[i]);
    lost_ping_texts.push(time_strings[i]);
  }
});

const traces = [
  {
    type: %%TRACE_TYPE%%,
    x: times,
    y: rssi.map(v => v===null?null:v),
    mode: 'lines+markers',
    name: 'RSSI (dBm)',
    yaxis: 'y1',
    marker: {size:6},
    hovertemplate: '%{text}<br>RSSI: %{y} dBm',
    text: time_strings
  },
  
  {
    type: %%TRACE_TYPE%%,
    x: times,
    y: route_latency.map(v => v===null?null:v),
    mode: 'lines+markers',
    name: 'Route latency (ms)',
    yaxis: 'y2',
    marker: {size:6},
    hovertemplate: '%{text}<br>Route latency: %{y} ms',
    text: time_strings
  },
  {
    type: %%TRACE_TYPE%%,
    x: times,
    y: ping_rtt.map(v => v===null?null:v),
    mode: 'lines+markers',
    name: 'Ping RTT (ms)',
    yaxis: 'y2',
    marker: {size:6},
    hovertemplate: '%{text}<br>Ping RTT: %{y} ms',
    text: time_strings
  },
  {
    type: %%TRACE_TYPE%%,
    x: times,
    y: hops.map(v => v===null?null:v),
    mode: 'lines+markers',
    name: 'Hops',
    yaxis: 'y3',
    marker: {size:6},
    hovertemplate: '%{text}<br>Hops: %{y}',
    text: time_strings
  }
];

const layout = {
  dragmode: 'pan',
  showlegend: true,
  xaxis: {
    title: 'Time',
    type: 'date',
    rangeslider: { visible: true },
  },
  yaxis: { title: 'RSSI (dBm)', side: 'left' },
  yaxis2: { title: 'Latency (ms)', overlaying: 'y', side: 'right', position: 0.95 },
  yaxis3: { title: 'Hops', anchor: 'free', overlaying: 'y', side: 'right', position: 0.85 },
  margin: {l:60, r:100, t:40, b:80}
};

Plotly.newPlot('plot', traces, layout, {responsive:true});
</script>
</body>
</html>
"""


def main():
    args = parse_args()
    if args.infile == '-':
        data = read_csv(sys.stdin)
    else:
        with open(args.infile, 'r', newline='') as f:
            data = read_csv(f)
    # Optionally downsample for high-volume data
    if args.downsample and args.downsample > 0 and len(data['times']) > args.downsample:
        n = args.downsample
        total = len(data['times'])
        step = total / float(n)
        idxs = []
        for i in range(n):
            idx = int(i * step)
            if idx >= total:
                idx = total - 1
            idxs.append(idx)
        # ensure unique and keep last
        idxs = sorted(set(idxs))
        if idxs[-1] != total - 1:
            idxs.append(total - 1)
        def pick(arr):
            return [arr[i] for i in idxs]
        data = {
            'times': pick(data['times']),
            'hops': pick(data['hops']),
            'rssi': pick(data['rssi']),
            'route_latency': pick(data['route_latency']),
            'ping_rtt': pick(data['ping_rtt']),
        }

    # Dump JSON arrays using safe placeholder replacement (avoid str.format on template with braces)
    payload = HTML_TEMPLATE
    payload = payload.replace('%%TIMES%%', json.dumps(data['times']))
    payload = payload.replace('%%HOPS%%', json.dumps([None if v is None else v for v in data['hops']]))
    payload = payload.replace('%%RSSI%%', json.dumps([None if v is None else v for v in data['rssi']]))
    payload = payload.replace('%%ROUTE_LATENCY%%', json.dumps([None if v is None else v for v in data['route_latency']]))
    payload = payload.replace('%%PING_RTT%%', json.dumps([None if v is None else v for v in data['ping_rtt']]))
    # TRACE_TYPE replacement
    trace_type = '"scattergl"' if args.use_webgl else '"scatter"'
    payload = payload.replace('%%TRACE_TYPE%%', trace_type)
    with open(args.outfile, 'w', encoding='utf-8') as of:
        of.write(payload)
    print(f'Wrote {args.outfile}', file=sys.stderr)
    if args.open_browser:
        try:
            webbrowser.open('file://' + args.outfile)
        except Exception as e:
            print('Failed to open browser:', e, file=sys.stderr)

if __name__ == '__main__':
    main()
