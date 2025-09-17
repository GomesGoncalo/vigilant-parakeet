#!/usr/bin/env bash
set -euo pipefail

# Measure latency (ping) from a source namespace to a destination IP or namespace.
# Usage: sudo scripts/measure-latency.sh -s src_ns -d dst_ip_or_ns [-c count] [-i interval]

COUNT=10
INTERVAL=0.2

usage() {
  cat <<EOF
Usage: $(basename "$0") -s SRC_NS -d DST [-c COUNT] [-i INTERVAL]
DST may be an IP address or a namespace name (in which case the script will try to pick an IPv4 from that namespace).
Example: sudo scripts/measure-latency.sh -s ns1 -d ns2 -c 100 -i 0.1
EOF
}

while [[ "$#" -gt 0 ]]; do
  case "$1" in
    -s|--src) SRC="$2"; shift 2;;
    -d|--dst) DST="$2"; shift 2;;
    -c|--count) COUNT="$2"; shift 2;;
    -i|--interval) INTERVAL="$2"; shift 2;;
    -h|--help) usage; exit 0;;
    *) echo "Unknown: $1"; usage; exit 1;;
  esac
done

if [[ -z "${SRC:-}" || -z "${DST:-}" ]]; then
  usage; exit 1
fi

if ! command -v ip >/dev/null 2>&1; then
  echo "ip command not found" >&2; exit 1
fi

if ! command -v ping >/dev/null 2>&1; then
  echo "ping not found" >&2; exit 1
fi

# Determine DST_IP if DST looks like a namespace
if ip netns list | awk '{print $1}' | grep -qw "$DST"; then
  DST_IP=$(ip netns exec "$DST" ip -4 addr show scope global 2>/dev/null | awk '/inet/ {print $2}' | head -n1 || true)
  if [[ -z "$DST_IP" ]]; then
    DST_IP=$(ip netns exec "$DST" ip -4 addr show 2>/dev/null | awk '/inet/ {print $2}' | head -n1 || true)
  fi
  DST_IP=${DST_IP%%/*}
else
  DST_IP="$DST"
fi

if [[ -z "$DST_IP" ]]; then
  echo "Could not determine destination IP for $DST" >&2
  exit 1
fi

echo "Pinging $DST_IP from namespace $SRC ($COUNT pings, interval $INTERVAL)"

# Run ping from SRC namespace
ip netns exec "$SRC" ping -c "$COUNT" -i "$INTERVAL" "$DST_IP"
