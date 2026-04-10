#!/usr/bin/env python3
"""
collect_metrics.py

Polls the simulator and the OBU network namespace for routing metrics every interval
and emits CSV lines suitable for plotting (timestamp,hops,rssi_dbm,route_latency_ms,ping_rtt_ms).

Usage examples:
  ./scripts/collect_metrics.py --obu obu1 --interval 0.5 --out /tmp/metrics.csv
  ./scripts/collect_metrics.py --obu obu1 | tee /tmp/metrics.csv

Notes:
- Requires access to the simulator HTTP API (default http://localhost:3030).
- Runs ping inside the sim namespace using sudo; ensure your user can sudo
  or run the script as a user with appropriate privileges.
"""

import argparse
import json
import os
import subprocess
import sys
import time
from datetime import datetime
from urllib.request import urlopen, Request
from urllib.error import URLError, HTTPError
import re

PING_TIMEOUT = 3

PING_TIME_RE = re.compile(r"time=([0-9]+\.?[0-9]*)\s*ms")


def fetch_node_info(base_url):
    url = base_url.rstrip('/') + '/node_info'
    req = Request(url, headers={"User-Agent": "collect-metrics/1.0"})
    try:
        with urlopen(req, timeout=2) as resp:
            return json.load(resp)
    except (URLError, HTTPError, ValueError) as e:
        return None


def run_ping(netns_prefix, obu, target_ip, user_env):
    # Build the command the same way the UI uses it
    # sudo ip netns exec sim_ns_<obu> runuser -l $USER -c "ping 10.0.0.1 -3 -D -W 2 -c 1"
    netns = f"{netns_prefix}{obu}"
    cmd = f'sudo ip netns exec {netns} runuser -l {user_env} -c "ping {target_ip} -3 -D -W 2 -c 1"'
    try:
        completed = subprocess.run(cmd, shell=True, capture_output=True, text=True, timeout=PING_TIMEOUT)
        out = completed.stdout + "\n" + completed.stderr
        m = PING_TIME_RE.search(out)
        if m:
            return float(m.group(1))
        else:
            return None
    except subprocess.TimeoutExpired:
        return None


def csv_header(out_f):
    out_f.write('iso_ts,unixts,obu,hops,rssi_dbm,route_latency_ms,ping_rtt_ms\n')
    out_f.flush()


def main():
    parser = argparse.ArgumentParser(description='Collect routing metrics for one OBU and ping RTT.')
    parser.add_argument('--obu', required=True, help='OBU node name (as shown in /nodes)')
    parser.add_argument('--base-url', default='http://localhost:3030', help='Simulator base URL')
    parser.add_argument('--interval', type=float, default=0.5, help='Polling interval in seconds')
    parser.add_argument('--out', default='-', help='Output CSV file (default stdout)')
    parser.add_argument('--target-ip', default='10.0.0.1', help='Ping target IP (default 10.0.0.1)')
    parser.add_argument('--netns-prefix', default='sim_ns_', help='Network namespace prefix for OBUs')
    args = parser.parse_args()

    user_env = os.getenv('USER', 'root')

    # Prompt for sudo credentials up-front to avoid the password prompt appearing
    # interleaved with CSV output. If sudo -v fails, warn but continue (the
    # caller may be root or prefer to run the script with privileges).
    try:
        rv = subprocess.run(["sudo", "-v"], check=False)
        if rv.returncode == 0:
            print('Sudo credentials cached; continuing', file=sys.stderr)
        else:
            print('Warning: sudo -v failed (you may be prompted on first ping call)', file=sys.stderr)
    except Exception as e:
        print(f'Warning: failed to run sudo -v: {e}', file=sys.stderr)

    if args.out == '-':
        out_f = sys.stdout
    else:
        out_f = open(args.out, 'w', buffering=1)

    csv_header(out_f)

    try:
        while True:
            ts = time.time()
            iso = datetime.utcfromtimestamp(ts).isoformat() + 'Z'

            node_info = fetch_node_info(args.base_url)
            hops = ''
            rssi = ''
            route_latency_ms = ''
            if node_info and isinstance(node_info, dict):
                info = node_info.get(args.obu)
                if info and isinstance(info, dict):
                    upstream = info.get('upstream')
                    if upstream:
                        hops = str(upstream.get('hops', ''))
                        rssi_val = upstream.get('rssi_dbm')
                        rssi = '' if rssi_val is None else f"{rssi_val}"
                        lat = upstream.get('latency_us')
                        if lat is not None:
                            # convert µs -> ms
                            route_latency_ms = f"{(lat/1000.0):.3f}"

            ping_rtt = run_ping(args.netns_prefix, args.obu, args.target_ip, user_env)
            ping_rtt_str = '' if ping_rtt is None else f"{ping_rtt:.3f}"

            line = f'{iso},{ts:.3f},{args.obu},{hops},{rssi},{route_latency_ms},{ping_rtt_str}\n'
            out_f.write(line)
            out_f.flush()

            # Sleep until next tick; account for work time so interval is steady
            elapsed = time.time() - ts
            to_sleep = args.interval - elapsed
            if to_sleep > 0:
                time.sleep(to_sleep)
    except KeyboardInterrupt:
        print('\nInterrupted, exiting', file=sys.stderr)
    finally:
        if args.out != '-':
            out_f.close()


if __name__ == '__main__':
    main()
