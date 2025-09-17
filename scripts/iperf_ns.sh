#!/usr/bin/env bash
set -euo pipefail

# Interactive script to run iperf3 between network namespaces.
# Prompts for source and destination namespace names, validates them,
# starts an iperf3 server in the destination namespace, runs the client
# from the source namespace for 10 seconds, then cleans up the server.


ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
  cat <<EOF
Usage: $(basename "$0") [options]
Interactive mode: run without -s or -d to choose namespaces interactively (fzf if available).

Options:
  -s, --src <ns>     Source namespace name
  -d, --dst <ns>     Destination namespace name
  -t, --time <secs>  Test duration in seconds (default: 10)
  -p, --port <port>  iperf3 port (default: 5201)
  -h, --help         Show this help

You must run this script as root (or with sudo) because it uses ip netns.
EOF
}

# Default parameters (can be overridden by flags)
TIME=10
PORT=5201

# Parse flags (simple POSIX-compatible)
while [[ "$#" -gt 0 ]]; do
    case "$1" in
        -s|--src)
        SRC_NS="$2"; shift 2;;
        -d|--dst)
        DST_NS="$2"; shift 2;;
        -t|--time)
        TIME="$2"; shift 2;;
        -p|--port)
        PORT="$2"; shift 2;;
        -h|--help)
        usage; exit 0;;
        --)
        shift; break;;
        -*|--*)
        echo "Unknown option: $1" >&2; usage; exit 1;;
        *)
        break;;
    esac
done

# Validate numeric flags
if ! [[ "$TIME" =~ ^[0-9]+$ ]] || [[ "$TIME" -le 0 ]]; then
    echo "Invalid time value: $TIME. Must be a positive integer." >&2
    usage
    exit 1
fi

if ! [[ "$PORT" =~ ^[0-9]+$ ]] || [[ "$PORT" -lt 1 ]] || [[ "$PORT" -gt 65535 ]]; then
    echo "Invalid port value: $PORT. Must be an integer between 1 and 65535." >&2
    usage
    exit 1
fi

if [[ $(id -u) -ne 0 ]]; then
    echo "This script must be run as root. Re-run with sudo." >&2
    exit 1
fi

# If both namespaces were provided on the command line, ensure they're different
if [[ -n "${SRC_NS:-}" && -n "${DST_NS:-}" && "$SRC_NS" == "$DST_NS" ]]; then
    echo "Source and destination namespaces must be different when provided as parameters." >&2
    exit 1
fi

if ! command -v ip >/dev/null 2>&1; then
    echo "ip command not found. Please install iproute2." >&2
    exit 1
fi

if ! command -v iperf3 >/dev/null 2>&1; then
    echo "iperf3 not found. Please install iperf3." >&2
    exit 1
fi

if command -v fzf >/dev/null 2>&1; then
    if [[ -z "${SRC_NS:-}" ]]; then
        echo "Select source namespace (use fzf):"
        SRC_NS=$(ip netns list | awk '{print $1}' | fzf --prompt="Source> " )
    fi
    if [[ -z "${DST_NS:-}" ]]; then
        echo "Select destination namespace (use fzf):"
        while true; do
            if [[ -n "${SRC_NS:-}" ]]; then
                DST_NS=$(ip netns list | awk '{print $1}' | grep -vFx "$SRC_NS" | fzf --prompt="Dest> " )
            else
                DST_NS=$(ip netns list | awk '{print $1}' | fzf --prompt="Dest> " )
            fi
            # if user pressed esc/empty, DST_NS will be empty
            if [[ -z "${DST_NS:-}" ]]; then
                echo "No destination selected. Cancelled." >&2
                exit 1
            fi
            if [[ -n "${SRC_NS:-}" && "$DST_NS" == "$SRC_NS" ]]; then
                echo "Destination cannot be the same as source. Please choose a different namespace." >&2
                DST_NS=""
                continue
            fi
            break
        done
    fi
else
    if [[ -z "${SRC_NS:-}" ]]; then
        read -rp "Source namespace name: " SRC_NS
    fi
    if [[ -z "${DST_NS:-}" ]]; then
        while true; do
            echo "Available namespaces:"
            if [[ -n "${SRC_NS:-}" ]]; then
                ip netns list | awk '{print $1}' | grep -vFx "$SRC_NS"
            else
                ip netns list
            fi
            read -rp "Destination namespace name: " DST_NS
            if [[ -z "${DST_NS:-}" ]]; then
                echo "No destination entered. Cancelled." >&2
                exit 1
            fi
            if [[ -n "${SRC_NS:-}" && "$DST_NS" == "$SRC_NS" ]]; then
                echo "Destination cannot be the same as source. Please enter a different namespace." >&2
                DST_NS=""
                continue
            fi
            break
        done
    fi
fi

if [[ -z "$SRC_NS" || -z "$DST_NS" ]]; then
    echo "Both source and destination namespace names are required." >&2
    exit 1
fi

if ! ip netns list | grep -qw "$SRC_NS"; then
    echo "Source namespace '$SRC_NS' not found. Available namespaces:" >&2
    ip netns list
    exit 1
fi

if ! ip netns list | grep -qw "$DST_NS"; then
    echo "Destination namespace '$DST_NS' not found. Available namespaces:" >&2
    ip netns list
    exit 1
fi

echo "Starting iperf3 server in namespace '$DST_NS' on port $PORT..."

# Start server in dst namespace, redirect logs
SERVER_LOG="/tmp/iperf3_${DST_NS}_server.log"
ip netns exec "$DST_NS" iperf3 -s -p "$PORT" >"$SERVER_LOG" 2>&1 &
SERVER_PID=$!

sleep 0.5

# Wait briefly and check server is listening (netstat may not be available)
if ! ip netns exec "$DST_NS" ss -ltn | grep -q ":$PORT"; then
    echo "iperf3 server did not start or is not listening on port $PORT. See $SERVER_LOG" >&2
    # Attempt to show some of the server log
    echo "--- server log (tail) ---"
    tail -n 50 "$SERVER_LOG" || true
    # Kill background pid if still running
    if kill -0 "$SERVER_PID" 2>/dev/null; then
        kill "$SERVER_PID" || true
    fi
    exit 1
fi


# Try to detect an IPv4 address inside the destination namespace.
# Prefer a non-loopback global address; if none found, fall back to 127.0.0.1
DST_IP=""
DST_IP=$(ip netns exec "$DST_NS" ip -4 addr show scope global 2>/dev/null \
| awk '/inet/ {print $2}' | head -n1 || true)

if [[ -z "$DST_IP" ]]; then
    # if no global inet, try any inet including loopback
    DST_IP=$(ip netns exec "$DST_NS" ip -4 addr show 2>/dev/null \
    | awk '/inet/ {print $2}' | head -n1 || true)
fi

if [[ -n "$DST_IP" ]]; then
    # strip CIDR suffix if present
    DST_IP=${DST_IP%%/*}
else
    DST_IP="127.0.0.1"
fi

echo "Running iperf3 client from namespace '$SRC_NS' to $DST_NS ($DST_IP) for ${TIME}s..."

# Run client for configured duration
ip netns exec "$SRC_NS" iperf3 -c "$DST_IP" -p "$PORT" -t "$TIME"

echo "Client finished. Cleaning up server..."

# Kill server process in destination namespace
if kill -0 "$SERVER_PID" 2>/dev/null; then
    kill "$SERVER_PID" || true
    wait "$SERVER_PID" 2>/dev/null || true
fi

echo "Done. Server log saved at $SERVER_LOG"
