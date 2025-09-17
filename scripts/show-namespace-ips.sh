#!/usr/bin/env bash
set -euo pipefail

# show-namespace-ips.sh
# List network namespaces and their IPv4 addresses.
# Usage:
#   scripts/show-namespace-ips.sh            # list all ns with IPs
#   scripts/show-namespace-ips.sh ns1 ns2   # list given namespaces only
#   scripts/show-namespace-ips.sh -j        # output JSON array of {ns,addrs}

JSON_OUTPUT=false
names=()

# If not running as root, prefix ip commands with sudo so we can enter other namespaces.
# This makes the script work whether namespaces were created as root or not.
if [ "$(id -u)" -ne 0 ]; then
  IP_CMD="sudo ip"
else
  IP_CMD="ip"
fi

usage(){
  cat <<EOF
Usage: $0 [-j] [NAMESPACE...]
  -j    output JSON
If no namespaces are provided, all current network namespaces are listed.
EOF
}

# parse options
while getopts ":hj" opt; do
  case "$opt" in
    j) JSON_OUTPUT=true ;;
    h) usage; exit 0 ;;
    \?) usage; exit 1 ;;
  esac
done
shift $((OPTIND - 1))

# build list of namespaces (either passed as args or discovered)
if [ "$#" -gt 0 ]; then
  names=("$@")
else
  # get namespaces using configured IP_CMD (may include sudo)
  while IFS= read -r line; do
    # ip netns list prints one per line, first token is the name
    ns=$(printf "%s" "$line" | awk '{print $1}')
    [ -n "$ns" ] && names+=("$ns")
  done < <($IP_CMD netns list 2>/dev/null)
fi

# collect IPv4 addresses per namespace
declare -A ns_map
for ns in "${names[@]}"; do
  addrs=()
  NETNS_CMD="$IP_CMD netns"
  # get only IPv4 'inet' lines and extract the address/prefix (second field)
  while IFS= read -r line; do
    # line like:    inet 10.0.0.2/24 brd 10.0.0.255 scope global eth0
    addr=$(printf "%s" "$line" | awk '{print $2}') || continue
    addrs+=("$addr")
  done < <($NETNS_CMD exec "$ns" ip -4 addr show scope global 2>/dev/null || true)

  ns_map["$ns"]="$(IFS=','; echo "${addrs[*]}")"
done

if [ "$JSON_OUTPUT" = true ]; then
  # Build JSON array
  python3 - <<'PY'
import json,sys
ns_map = {
  k: v for k,v in (
    (k, v) for k,v in (
      (line.split(':')[0], line.split(':',1)[1]) for line in []
    )
  )
}
PY
  # fallback: simple shell-built JSON
  printf '['
  first=true
  for ns in "${!ns_map[@]}"; do
    if [ "$first" = true ]; then first=false; else printf ','; fi
    addrs=${ns_map[$ns]}
    # convert comma-separated to JSON array
    IFS=',' read -r -a arr <<< "$addrs"
    ADDR_JSON=$(./target/debug/scripts_tools nsaddrs <<<"${addrs}")
    printf '{"ns":%s,"addrs":%s}' "$(printf '%s' "$ns" | python3 -c 'import json,sys; print(json.dumps(sys.stdin.read().strip()))')" "$ADDR_JSON"
  done
  printf ']\n'
else
  for ns in "${!ns_map[@]}"; do
    echo "$ns:"
    addrs=${ns_map[$ns]}
    if [ -z "$addrs" ]; then
      echo "  (no IPv4 addresses)"
    else
      IFS=',' read -r -a arr <<< "$addrs"
      for a in "${arr[@]}"; do
        echo "  $a"
      done
    fi
  done
fi

exit 0
