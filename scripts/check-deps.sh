#!/usr/bin/env bash
set -euo pipefail

# check-deps.sh - simple dependency checker for this repo's scripts
# Checks for required and optional commands and prints a short report.

REQUIRED=(ip iperf3 ss curl python3)
OPTIONAL=(fzf)

missing_req=()
missing_opt=()

echo "Checking runtime dependencies..."

for cmd in "${REQUIRED[@]}"; do
  if ! command -v "$cmd" >/dev/null 2>&1; then
    missing_req+=("$cmd")
  fi
done

for cmd in "${OPTIONAL[@]}"; do
  if ! command -v "$cmd" >/dev/null 2>&1; then
    missing_opt+=("$cmd")
  fi
done

if [ ${#missing_req[@]} -eq 0 ]; then
  echo "All required commands present: ${REQUIRED[*]}"
else
  echo "Missing REQUIRED commands: ${missing_req[*]}"
  echo "Please install them. On Debian/Ubuntu:"
  echo "  sudo apt update && sudo apt install -y iproute2 iperf3 curl python3 iproute2"
  exit 2
fi

if [ ${#missing_opt[@]} -ne 0 ]; then
  echo "Optional commands not found (script will still work): ${missing_opt[*]}"
fi

echo "Note: running commands that enter network namespaces requires root privileges (sudo)."

exit 0
