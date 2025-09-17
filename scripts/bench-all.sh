#!/usr/bin/env zsh
# Run benches for all workspace crates that expose a `benches/` directory.
# Usage: ./scripts/bench-all.sh [--release] [--features "feat1 feat2"]
# You can set CRITERION_FLAGS env var to pass extra flags to Criterion (e.g.
# CRITERION_FLAGS="--measurement-time 3 --sample-size 50"). These are appended
# after a `--` to each `cargo bench` invocation.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT_DIR="$ROOT/target/bench-reports/$(date +%Y%m%d-%H%M%S)"
mkdir -p "$OUT_DIR"

RELEASE=false
FEATURES=""
WORKSPACE=false
PARALLEL=1
SHOW_SUMMARY=false

while [[ $# -gt 0 ]]; do
  case $1 in
    --release|-r)
      RELEASE=true
      shift
      ;;
    --features|-f)
      FEATURES="$2"
      shift 2
      ;;
    --help|-h)
      echo "Usage: $0 [--release] [--features \"feat1 feat2\"]"
      exit 0
      ;;
    --workspace)
      WORKSPACE=true
      shift
      ;;
    -j|--parallel)
      PARALLEL=$2
      shift 2
      ;;
    --show-summary)
      SHOW_SUMMARY=true
      shift
      ;;
    *)
      echo "Unknown argument: $1"
      exit 1
      ;;
  esac
done

echo "bench-all: output -> $OUT_DIR"

errors=0

# collect candidate crates first
candidates=()
if [[ "$WORKSPACE" == true ]]; then
  candidates+=("$ROOT")
else
  while IFS= read -r crate; do
    candidates+=("$crate")
  done < <(find "$ROOT" -maxdepth 2 -type d -name benches -print | sed 's#/benches##')
fi

run_one() {
  local crate_root="$1"
  local crate_name
  crate_name=$(basename "$crate_root")
  echo "--- Running benches for $crate_name ---"
  local out="$OUT_DIR/${crate_name}.txt"
  pushd "$crate_root" >/dev/null
  # if CRITERION_FLAGS is set, append them after a `--` so cargo passes them to the
  # benchmark harness / Criterion. Example: CRITERION_FLAGS="--measurement-time 3"
  # However, don't pass those flags to crates whose benches use the default test
  # harness (they will reject unknown options). Detect Criterion-style benches
  # by searching for `criterion_main!` in the benches/ sources.
  # Build per-crate Criterion arg array: only populate if benches contain
  # `criterion_main!`. This avoids passing Criterion CLI flags to non-Criterion
  # test harnesses which will reject unknown options.
  local -a crit_args=()
  if [[ -n "${CRITERION_FLAGS:-}" ]]; then
    if [[ -d "$crate_root/benches" ]] && grep -I -R --line-number --quiet "criterion_main!" "$crate_root/benches" 2>/dev/null; then
      # Pass Criterion CLI flags directly to the bench binary. Do NOT include a
      # leading `--` here because we invoke the benchmark executable directly
      # (the `--` would become a positional separator and break flag parsing).
      crit_args=(${=CRITERION_FLAGS})
    else
      echo "Note: skipping CRITERION_FLAGS for $crate_name (no criterion_main! found in benches)"
    fi
  fi
  # Build bench targets without running them, then run bench binaries directly.
  # Note: we will run `cargo` with --manifest-path so cargo places outputs in
  # the workspace-level `target/` (avoids creating nested crate-local target/)
  # Detect per-crate feature gates around benches (e.g. #[cfg(feature = "stats")])
  # so we can build benches with those features enabled when necessary. We
  # combine user-supplied $FEATURES with any discovered features.
  per_crate_features="$FEATURES"
  if [[ -d "$crate_root/benches" ]]; then
    # Use Python to reliably parse `cfg(feature = "name")` occurrences.
    discovered=$(python3 - <<'PY'
import re,glob,sys
feats=set()
for p in glob.glob('benches/*.rs'):
    try:
        s=open(p).read()
    except Exception:
        continue
    for m in re.finditer(r'cfg\s*\(\s*feature\s*=\s*"([^"]+)"', s):
        feats.add(m.group(1))
print(' '.join(sorted(feats)))
PY
    )
    if [[ -n "$discovered" ]]; then
      if [[ -n "$per_crate_features" ]]; then
        per_crate_features="$per_crate_features $discovered"
      else
        per_crate_features="$discovered"
      fi
    fi
  fi
  if [[ -n "$per_crate_features" ]]; then
    :
  fi
  # Pop back to the original CWD before invoking cargo so the workspace
  # workspace-level `target/` is used. We scoped all crate-local file checks
  # above while inside the crate dir.
  popd >/dev/null

  # Build benches (no-run) and capture cargo JSON messages that include
  # "executable" fields. Use --manifest-path to ensure outputs are placed in
  # the workspace `target/` directory.
  build_out=$(mktemp)
  # Place --manifest-path after the subcommand so older/newer cargo versions
  # accept it: `cargo bench --manifest-path <path> ...`
  cmd=(cargo bench --manifest-path "$crate_root/Cargo.toml" --benches)
  if [[ "$RELEASE" == true ]]; then
    cmd+=(--profile)
    cmd+=(release)
  fi
  if [[ -n "$per_crate_features" ]]; then
    cmd+=(--features)
    cmd+=("$per_crate_features")
  elif [[ -n "$FEATURES" ]]; then
    cmd+=(--features)
    cmd+=("$FEATURES")
  fi
  cmd+=(--no-run --message-format=json)

  if ! "${cmd[@]}" >"$build_out" 2>&1; then
    # If cargo failed, dump build output to crate log and record error
    cat "$build_out" >>"$out"
    echo "$crate_name: failed to build benches" >>"$OUT_DIR/errors.txt"
    rm -f "$build_out"
    return
  fi

  # Save the raw cargo JSON build output to the crate log for debugging
  echo "--- cargo build (json) output ---" >>"$out"
  cat "$build_out" >>"$out"
  echo "--- end cargo build output ---" >>"$out"

  bench_execs=()
  # Extract executables only for messages whose "target.kind" contains "bench".
  # Prefer jq if available; otherwise use a small Python fallback that reads
  # JSON lines reliably.
  parsed=()
  if command -v jq >/dev/null 2>&1; then
    # Use jq to stream executable paths
    while IFS= read -r line; do
      parsed+=("$line")
    done < <(jq -r 'select(.reason=="compiler-artifact") | select(.target.kind[]?=="bench") | .executable' "$build_out" 2>/dev/null || true)
  else
    # Python fallback
    while IFS= read -r line; do
      parsed+=("$line")
    done < <(python3 - <<'PY'
import sys, json
for line in sys.stdin:
    try:
        obj = json.loads(line)
    except Exception:
        continue
    if obj.get('reason') != 'compiler-artifact':
        continue
    target = obj.get('target') or {}
    kinds = target.get('kind') or []
    if any(k == 'bench' for k in kinds):
        exe = obj.get('executable')
        if exe:
            print(exe)
PY
    < "$build_out")
  fi
  # Debug: write parsed executable paths to crate log for inspection
  echo "--- parsed executable paths (from cargo JSON) ---" >>"$out"
  for p in "${parsed[@]:-}"; do
    echo "$p" >>"$out"
  done
  echo "--- end parsed executable paths ---" >>"$out"

  # Filter parsed paths: prefer executability, but also accept files that
  # exist even if not marked executable so we don't drop valid cargo-reported
  # paths (helps diagnose permission/quoting issues). We still log cases
  # where the path doesn't exist so the output can be investigated.
  for exe_path in "${parsed[@]:-}"; do
    if [[ -z "$exe_path" ]]; then
      continue
    fi
    if [[ -x "$exe_path" ]]; then
      bench_execs+=("$exe_path")
    elif [[ -f "$exe_path" ]]; then
      echo "Warning: $exe_path exists but is not executable; adding for diagnostic" >>"$out"
      bench_execs+=("$exe_path")
    else
      echo "Debug: parsed exe path missing on disk: $exe_path" >>"$out"
    fi
  done
  rm -f "$build_out"

  if [[ ${#bench_execs[@]} -eq 0 ]]; then
    echo "No bench executables detected for $crate_name" >>"$OUT_DIR/errors.txt"
    return
  fi

  for bench_exec in "${bench_execs[@]}"; do
    echo "--- Running bench binary: $(basename "$bench_exec") for $crate_name ---" >>"$out"
    if [[ ${#crit_args[@]} -gt 0 ]]; then
      set -o noglob
      "$bench_exec" "${crit_args[@]}" >>"$out" 2>&1 || echo "$crate_name:$(basename "$bench_exec"): failed" >>"$OUT_DIR/errors.txt"
      set +o noglob
    else
      "$bench_exec" >>"$out" 2>&1 || echo "$crate_name:$(basename "$bench_exec"): failed" >>"$OUT_DIR/errors.txt"
    fi
  done
}

# run with limited parallelism
pids=()
running=0

wait_for_any() {
  # portable: poll pids and wait/reap the first one that is no longer running
  while true; do
    n=${#pids[@]}
    if (( n == 0 )); then
      return 0
    fi
    for ((i=1; i<=n; i++)); do
      pid=${pids[i]}
      if ! kill -0 "$pid" 2>/dev/null; then
        # pid finished; reap it
        wait "$pid" || true
        # rebuild pids array without index i
        newpids=()
        for ((j=1; j<=n; j++)); do
          if (( j != i )); then
            newpids+=("${pids[j]}")
          fi
        done
        pids=("${newpids[@]}")
        return 0
      fi
    done
    sleep 0.2
  done
}

for crate in "${candidates[@]}"; do
  run_one "$crate" &
  pids+=("$!")
  running=$((running+1))
  if [[ $running -ge $PARALLEL ]]; then
    # wait for any to finish (portable)
    wait_for_any
    running=$((running-1))
  fi
done

# wait for remaining
for pid in "${pids[@]}"; do
  wait "$pid" || true
done

echo
echo "Bench reports saved: $OUT_DIR"
if [[ -f "$OUT_DIR/errors.txt" ]]; then
  echo "Some benches failed; see $OUT_DIR/errors.txt"
  exit 1
fi

# Collect Criterion per-bench estimates into a single summary JSON
SUMMARY_FILE="$OUT_DIR/bench-summary.json"
echo "Collecting Criterion summaries into $SUMMARY_FILE"
declare -a estimates
while IFS= read -r file; do
  estimates+=("$file")
done < <(find "$ROOT/target/criterion" -type f -name estimates.json 2>/dev/null || true)

if [[ ${#estimates[@]} -eq 0 ]]; then
  echo "No estimates.json files found under target/criterion. Skipping summary generation."
else
  # Prefer jq if available, otherwise build a minimal JSON array by concatenating
  echo "building summary with jq"
  echo "[]" >"$SUMMARY_FILE"
  for f in "${estimates[@]}"; do
      # extract directory_name and mean.point_estimate and std_dev
      # If .directory_name is null, derive a bench name from the path under
      # target/criterion (use the parent directory name).
      bench_name=$(jq -r '.directory_name // empty' "$f" )
      if [[ -z "$bench_name" ]]; then
        # derive from the file path: e.g. target/criterion/<benchdir>/.../estimates.json
        bench_name=$(basename "$(dirname "$f")")
      fi
      # derive artifact relative path under target/criterion so we can trace the
      # measurement back to the exact bench directory (e.g.
      # routing_get_route_group/routing_get_route_1024/base)
    # Compute the relative path under target/criterion for traceability.
    root_crit="$ROOT/target/criterion"
    artifact_rel=$(python3 -c 'import os,sys
f=sys.argv[1]
root=sys.argv[2]
try:
  print(os.path.relpath(os.path.dirname(f), root))
except Exception:
  print(os.path.dirname(f))
' "$f" "$root_crit")
      # top-level group is the first path component of the artifact_rel
      group=$(echo "$artifact_rel" | cut -d'/' -f1)
      entry=$(jq -n --arg b "$bench_name" --arg art "$artifact_rel" --arg grp "$group" --slurpfile j "$f" '{bench: $b, artifact: $art, group: $grp, mean: ($j[0].mean.point_estimate), mean_ci: ($j[0].mean.confidence_interval), std_dev: ($j[0].std_dev.point_estimate? // $j[0].std_dev)}')
      jq ". + [${entry}]" "$SUMMARY_FILE" >"${SUMMARY_FILE}.tmp" && mv "${SUMMARY_FILE}.tmp" "$SUMMARY_FILE"
    done
  echo "Wrote summary to $SUMMARY_FILE"
fi

  if [[ -f "$OUT_DIR/errors.txt" ]]; then
    echo "Some benches failed; see $OUT_DIR/errors.txt"
    exit 1
  fi

  if [[ "$SHOW_SUMMARY" == true ]]; then
    if [[ -f "$SUMMARY_FILE" ]]; then
      echo
      echo "---- Bench summary ($SUMMARY_FILE) ----"
  jq '.' "$SUMMARY_FILE" || cat "$SUMMARY_FILE"
      echo "-------------------------------------"
    else
      echo "--show-summary requested but $SUMMARY_FILE not found."
      exit 2
    fi
  fi

  exit 0
