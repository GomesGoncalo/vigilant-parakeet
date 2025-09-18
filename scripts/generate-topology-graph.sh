#!/usr/bin/env bash
set -euo pipefail


# Usage: generate-topology-graph.sh [--directed] [--out <file>] [input]
IN="examples/simulator.yaml"
OUT="topology.dot"
DIRECTED=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --directed) DIRECTED=1; shift ;;
    --out) OUT="$2"; shift 2 ;;
    -o) OUT="$2"; shift 2 ;;
    -h|--help) echo "Usage: $0 [--directed] [--out <file>] [input]"; exit 0 ;;
    *) IN="$1"; shift ;;
  esac
done

if [ ! -f "$IN" ]; then
  echo "Input file $IN not found" >&2
  exit 2
fi

TMP_EDGES=$(mktemp)
TMP_NODES=$(mktemp)

# compute dot output file (if user requested PNG, write dot then render)
DOTFILE="$OUT"
if [[ "$OUT" == *.png ]]; then
  DOTFILE="${OUT%.png}.dot"
fi

# collect nodes and types (only top-level entries under nodes:)
awk '/^nodes:/, /^topology:/' "$IN" | sed '1d;$d' | awk '/^  [^ ]/ { line=$0; sub(/^  /, "", line); sub(/:.*$/, "", line); print line }' > "$TMP_NODES"

# collect topology edges using a per-source extraction (robust to formatting)
# start with an empty edges file
> "$TMP_EDGES"
while read -r src; do
  # extract block for this source under topology: (lines between "  src:" and next top-level entry)
  sed -n "/^  ${src}:/,/^  [^ ]/p" "$IN" | sed '1d;$d' > /tmp/topo_block.$$ || true
  # for each peer inside the block
  awk '/^[[:space:]]+[a-zA-Z0-9._-]+:/{gsub(/^[[:space:]]*/,"",$0); sub(/:.*$/,"",$0); print $0}' /tmp/topo_block.$$ | while read -r peer; do
    # find latency and loss for this peer within the block
    lat=$(sed -n "/^[[:space:]]\+${peer}:/,/^[[:space:]]\+[a-zA-Z0-9._-]\+:/p" /tmp/topo_block.$$ | sed '1d' | sed -n '1,5p' | grep -E 'latency:' | sed -E 's/[^0-9.]*([0-9.]+).*/\1/' || true)
    loss=$(sed -n "/^[[:space:]]\+${peer}:/,/^[[:space:]]\+[a-zA-Z0-9._-]\+:/p" /tmp/topo_block.$$ | sed '1d' | sed -n '1,5p' | grep -E 'loss:' | sed -E 's/[^0-9.]*([0-9.]+).*/\1/' || true)
    lat=${lat:-0}
    loss=${loss:-0}
    echo "$src $peer $lat $loss" >> "$TMP_EDGES"
  done
  rm -f /tmp/topo_block.$$ || true
done < "$TMP_NODES"

# compute MAX_LAT (safe numeric handling) - default to 1 when no edges
if [ -s "$TMP_EDGES" ]; then
  MAX_LAT=$(awk '{if(($3+0)>max) max=($3+0)}END{if(max=="") print 1; else print max}' "$TMP_EDGES")
else
  MAX_LAT=1
fi

GRAPH_TYPE="graph"
EDGE_OP="--"
if [ "$DIRECTED" -ne 0 ]; then GRAPH_TYPE="digraph"; EDGE_OP="->"; fi

echo "$GRAPH_TYPE topology {" > "$DOTFILE"
echo "  rankdir=LR;" >> "$DOTFILE"
echo "  node [shape=ellipse, style=filled, fillcolor=white];" >> "$DOTFILE"

# clusters: detect RSU vs OBU by name prefix if available
RSU_NODES=()
OBU_NODES=()
while read -r n; do
  [ -z "$n" ] && continue
  case "$n" in
    rsu*|RSU*) RSU_NODES+=("$n") ;;
    obu*|OBU*) OBU_NODES+=("$n") ;;
  *) echo "  \"$n\";" >> "$DOTFILE" ;;
  esac
done < "$TMP_NODES"

if [ ${#RSU_NODES[@]} -gt 0 ]; then
  echo "  subgraph cluster_rsu {" >> "$DOTFILE"
  echo "    label=\"RSU\"; color=lightgrey;" >> "$DOTFILE"
  for n in "${RSU_NODES[@]}"; do echo "    \"$n\" [shape=box, fillcolor=lightblue];" >> "$DOTFILE"; done
  echo "  }" >> "$DOTFILE"
fi

if [ ${#OBU_NODES[@]} -gt 0 ]; then
  echo "  subgraph cluster_obu {" >> "$DOTFILE"
  echo "    label=\"OBU\"; color=lightgrey;" >> "$DOTFILE"
  for n in "${OBU_NODES[@]}"; do echo "    \"$n\" [shape=ellipse, fillcolor=lightyellow];" >> "$DOTFILE"; done
  echo "  }" >> "$DOTFILE"
fi

# print edges with labels and penwidth scaled by latency (lower latency -> thicker)
# deduplicate edges (handles symmetric listings) by sorting unique lines first
if [ -s "$TMP_EDGES" ]; then
  while read -r src dst lat loss; do
  [ -z "$src" ] && continue
  label="${lat}ms / ${loss}%"
  # penwidth: 1 + (max_lat - lat)/10
  pw=$(awk -v m=$MAX_LAT -v l=$lat 'BEGIN{pw=1+(m-l)/10; if(pw<0.5) pw=0.5; printf("%.2f",pw)}')
  # avoid duplicate undirected edges: only print when src < dst lexicographically
    if [ "$DIRECTED" -ne 0 ]; then
    echo "  \"$src\" $EDGE_OP \"$dst\" [label=\"$label\", penwidth=$pw];" >> "$DOTFILE"
  else
    if [[ "$src" < "$dst" ]]; then
      echo "  \"$src\" $EDGE_OP \"$dst\" [label=\"$label\", penwidth=$pw];" >> "$DOTFILE"
    fi
  fi
  done < <(sort -u "$TMP_EDGES")
fi
echo "}" >> "$DOTFILE"

rm -f "$TMP_EDGES" "$TMP_NODES"

echo "Wrote $DOTFILE"

# If desired, render to PNG
if [[ "$OUT" == *.png ]]; then
  if command -v dot >/dev/null 2>&1; then
    dot -Tpng -o "$OUT" "$DOTFILE"
    echo "Rendered $OUT"
  else
    echo "dot not found; install graphviz to render PNG" >&2
  fi
fi
