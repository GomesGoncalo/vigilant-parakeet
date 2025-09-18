#!/usr/bin/env bash
set -euo pipefail

files=("$@")
if [ ${#files[@]} -eq 0 ]; then
  files=(examples/*.yaml)
fi

printf "%-40s %-10s %-18s %s\n" "file" "type" "ip" "notes"
for f in "${files[@]}"; do
  [ -f "$f" ] || continue
  node_type=$(grep -E '^\s*node_type:' "$f" | head -n1 | sed -E 's/.*node_type:\s*//') || node_type=""
  ip=$(grep -E '^\s*ip:' "$f" | head -n1 | sed -E 's/.*ip:\s*//') || ip=""
  notes=()
  if [ -z "$node_type" ]; then notes+=("missing node_type"); fi
  if [ -z "$ip" ]; then notes+=("missing ip"); fi
  # detect simulator files
  if grep -qE '^\s*topology:' "$f" 2>/dev/null; then notes+=("simulator"); fi
  printf "%-40s %-10s %-18s %s\n" "$f" "${node_type:--}" "${ip:--}" "$(IFS=,; echo "${notes[*]}")"
done
