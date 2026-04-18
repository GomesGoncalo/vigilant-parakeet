#!/usr/bin/env python3
"""
plot_metrics.py

Read the CSV emitted by collect_metrics.py and produce a PNG showing:
 - RSSI (dBm)
 - Route latency (ms)
 - Ping RTT (ms)
 - Hops (step plot)

Usage:
  ./scripts/plot_metrics.py --in /tmp/metrics.csv --out /tmp/metrics.png

The script uses only the Python standard library + matplotlib. Install matplotlib
if missing: pip install matplotlib

"""

import argparse
import csv
import sys
from datetime import datetime
import math

try:
    import matplotlib
    matplotlib.use('Agg')
    import matplotlib.pyplot as plt
    import matplotlib.dates as mdates
except Exception as e:
    print("matplotlib is required: pip install matplotlib", file=sys.stderr)
    raise


def parse_iso(s):
    if s is None or s == '':
        return None
    # Accept trailing Z or no Z
    if s.endswith('Z'):
        s2 = s[:-1]
    else:
        s2 = s
    try:
        # fromisoformat supports fractional seconds
        return datetime.fromisoformat(s2)
    except Exception:
        # Fallback common format
        for fmt in ("%Y-%m-%dT%H:%M:%S.%f", "%Y-%m-%dT%H:%M:%S"):
            try:
                return datetime.strptime(s2, fmt)
            except Exception:
                pass
    return None


def read_csv(path):
    rows = []
    if path == '-':
        f = sys.stdin
    else:
        f = open(path, 'r')
    with f:
        reader = csv.reader(f)
        header = next(reader, None)
        for r in reader:
            if len(r) < 7:
                continue
            iso_ts = r[0]
            # r[1] unixts ignored
            obu = r[2]
            hops = r[3]
            rssi = r[4]
            route_lat = r[5]
            ping_rtt = r[6]
            rows.append((iso_ts, obu, hops, rssi, route_lat, ping_rtt))
    return rows


def to_numeric(x, typ=float):
    if x is None or x == '':
        return None
    try:
        return typ(x)
    except Exception:
        return None


def plot(rows, out_path, title=None):
    # rows: list of (iso, obu, hops, rssi, route_lat, ping_rtt)
    times = []
    hops = []
    rssi = []
    route_lat = []
    ping_rtt = []

    for iso, obu, h, r, rl, pr in rows:
        dt = parse_iso(iso)
        if dt is None:
            continue
        times.append(dt)
        hops.append(to_numeric(h, int))
        rssi.append(to_numeric(r, float))
        route_lat.append(to_numeric(rl, float))
        ping_rtt.append(to_numeric(pr, float))

    if not times:
        print('no data to plot', file=sys.stderr)
        return 1

    # Setup plot with 3 rows: RSSI, latency (route+ping), hops
    fig, axs = plt.subplots(3, 1, figsize=(12, 9), sharex=True)

    # RSSI (top)
    ax = axs[0]
    ax.plot(times, rssi, marker='o', linestyle='-', color='tab:blue', label='RSSI (dBm)')
    ax.set_ylabel('RSSI (dBm)')
    ax.grid(True, linestyle=':', alpha=0.6)
    ax.legend(loc='best')

    # Latency (middle)
    ax = axs[1]
    if any(x is not None for x in route_lat):
        ax.plot(times, [x if x is not None else math.nan for x in route_lat], marker='o', linestyle='-', color='tab:orange', label='Route latency (ms)')
    if any(x is not None for x in ping_rtt):
        ax.plot(times, [x if x is not None else math.nan for x in ping_rtt], marker='x', linestyle='--', color='tab:green', label='Ping RTT (ms)')
    ax.set_ylabel('Latency (ms)')
    ax.grid(True, linestyle=':', alpha=0.6)
    ax.legend(loc='best')

    # Hops (bottom)
    ax = axs[2]
    # Step plot for hops; replace None with nan
    hops_vals = [float(x) if x is not None else math.nan for x in hops]
    ax.step(times, hops_vals, where='post', color='tab:purple', label='Hops')
    ax.set_ylabel('Hops')
    ax.set_xlabel('Time')
    ax.grid(True, linestyle=':', alpha=0.6)
    ax.legend(loc='best')

    # Format x-axis nicely
    ax = axs[-1]
    ax.xaxis.set_major_formatter(mdates.DateFormatter('%H:%M:%S'))
    fig.autofmt_xdate()

    if title:
        fig.suptitle(title)

    plt.tight_layout(rect=[0, 0.03, 1, 0.95])
    fig.savefig(out_path, dpi=150)
    print(f'Wrote {out_path}')
    return 0


def main():
    parser = argparse.ArgumentParser(description='Plot metrics CSV produced by collect_metrics.py')
    parser.add_argument('--in', dest='infile', required=True, help='Input CSV path (use - for stdin)')
    parser.add_argument('--out', dest='out', default='metrics.png', help='Output PNG path')
    parser.add_argument('--title', default=None, help='Optional title for the plot')
    args = parser.parse_args()

    rows = read_csv(args.infile)
    rc = plot(rows, args.out, title=args.title)
    sys.exit(rc)

if __name__ == '__main__':
    main()
