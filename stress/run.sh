#!/usr/bin/env bash
# Run every stress scenario against binaries built from this tree.
#
#   stress/run.sh            all scenarios
#   stress/run.sh scale      only scenarios whose name contains "scale"
#   STRESS_TIER=fast stress/run.sh  catalogued fast-contract tier only
#
# Exit: 0 all passed · 1 something failed · 2 nothing could be proven here ·
# 3 refused to start, another run is already in progress.
#
# Scenarios emulate deployed conditions: a real Desktop mid-project, a synced
# folder, a card dump, an interrupted run. None of it is your data; every tree
# is generated.
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
python3 scripts/check-stress-catalog.py || exit 1

# `mkdir` is atomic. The old check-then-write PID file allowed two runners to
# both observe an absent lock and start destructive volume scenarios together.
# Cleanup only removes the PID file and directory this process created.
LOCK_DIR="${TMPDIR:-/tmp}/etudes-stress.lock.d"
cleanup_run_lock() {
  [ "${STRESS_LOCK_OWNED:-0}" = 1 ] || return 0
  rm -f "$LOCK_DIR/pid"
  rmdir "$LOCK_DIR" 2>/dev/null || true
}
if ! mkdir "$LOCK_DIR" 2>/dev/null; then
  lock_pid=$(cat "$LOCK_DIR/pid" 2>/dev/null || true)
  if [ -n "$lock_pid" ] && kill -0 "$lock_pid" 2>/dev/null; then
    echo "another stress run is already in progress (pid $lock_pid): $LOCK_DIR"
    exit 3
  fi
  # Recover only an empty/stale lock we can identify. A foreign file in this
  # directory is evidence we do not own; leave it for a person to inspect.
  if [ -f "$LOCK_DIR/pid" ]; then rm -f "$LOCK_DIR/pid"; fi
  if ! rmdir "$LOCK_DIR" 2>/dev/null || ! mkdir "$LOCK_DIR" 2>/dev/null; then
    echo "could not acquire stress-run lock safely: $LOCK_DIR"
    exit 3
  fi
fi
STRESS_LOCK_OWNED=1
printf '%s\n' "$$" > "$LOCK_DIR/pid"
trap cleanup_run_lock EXIT

echo "building release binaries"
cargo build --release --quiet || { echo "build failed"; exit 1; }
export BIN="$PWD/target/release"

# The scenario wrapper and this runner use one record format and one verdict
# function.  Sourcing with SCENARIO=run defines helpers without wrapping this
# coordinator itself.
SCENARIO=run BIN="$BIN" source stress/lib.sh
trap 'cleanup_run_lock; eval "$_stress_state_cleanup"' EXIT

# Sweeps up whatever a previous SIGKILLed run left mounted. Nothing inside a
# killed process can do this for itself -- see sweep_orphaned_volumes in
# lib.sh for why -- so it runs once here, before any scenario, rather than
# per scenario.
SCENARIO=run BIN="$BIN" bash -c 'source stress/lib.sh; sweep_orphaned_volumes'

filter="${1:-}"
tier="${STRESS_TIER:-all}"
case "$tier" in all|fast|load|platform) ;; *) echo "unknown STRESS_TIER: $tier"; exit 2;; esac
TOTAL_P=0; TOTAL_F=0; TOTAL_U=0
ALL_FAIL=(); ALL_UNPROVEN=()
CASE_TIMEOUT_MS="${STRESS_CASE_TIMEOUT_MS:-300000}"
case "$CASE_TIMEOUT_MS" in ''|*[!0-9]*) echo "invalid STRESS_CASE_TIMEOUT_MS: $CASE_TIMEOUT_MS"; exit 2;; esac
[ "$CASE_TIMEOUT_MS" -gt 0 ] || { echo "STRESS_CASE_TIMEOUT_MS must be positive"; exit 2; }

# Runtime facts belong to a generated bundle, not the checked-in catalog.
# Keeping one line per completed case lets CI retain a small, useful artifact
# while failure-only copies preserve the assertion record and transcript that
# explain a red result.  Callers can set STRESS_RESULTS_DIR to their CI
# artifact directory; local runs default to an ignored directory in stress/.
RESULTS_DIR="${STRESS_RESULTS_DIR:-$PWD/stress/results}"
RUN_ID="${STRESS_RUN_ID:-$(date -u +%Y%m%dT%H%M%SZ)-$$}"
RUN_DIR="$RESULTS_DIR/$RUN_ID"
mkdir -p "$RUN_DIR/failures" || { echo "could not create stress result directory: $RUN_DIR"; exit 1; }
RESULT_ROWS="$RUN_DIR/cases.tsv"
printf 'id\tpassed\tfailed\tunproven\tduration_ms\tchild_exit\tevidence\n' > "$RESULT_ROWS"

for s in stress/scenarios/*.sh; do
  name=$(basename "$s" .sh)
  [ -n "$filter" ] && [[ "$name" != *"$filter"* ]] && continue
  if [ "$tier" != all ]; then
    scenario_tier=$(python3 - "$name" <<'PY'
import json, sys
for row in json.load(open("stress/catalog.json")):
    if row["id"] == sys.argv[1]:
        print(row["tier"])
        break
PY
)
    [ "$scenario_tier" = "$tier" ] || continue
  fi
  echo ""
  echo "── $name"
  started_ns=$(python3 -c 'import time; print(time.monotonic_ns())')
  record=$(mktemp "${TMPDIR:-/tmp}/etudes-stress-run-record-XXXXXX")
  transcript=$(mktemp "${TMPDIR:-/tmp}/etudes-stress-transcript-XXXXXX")
  process_evidence="$RUN_DIR/$name.process.json"
  case_artifacts="$RUN_DIR/$name.artifacts"
  mkdir -p "$case_artifacts"
  exec 197>>"$record"
  # Each case executes in the bounded helper's owned session. Disable nested
  # job control so the direct-run wrapper stays in that session and is reaped
  # with every descendant on timeout or interruption.
  if STRESS_RESULT_FD=197 SCENARIO="$name" STRESS_NO_JOB_CONTROL=1 STRESS_FAILURE_ARTIFACTS="$case_artifacts" \
      python3 stress/bounded.py --timeout-ms "$CASE_TIMEOUT_MS" --evidence "$process_evidence" --pass-fd 197 -- bash "$s"; then
    bounded_status=0
  else
    bounded_status=$?
  fi
  python3 - "$process_evidence" "$transcript" <<'PY'
import json
import pathlib
import sys

result = json.load(open(sys.argv[1]))
pathlib.Path(sys.argv[2]).write_text(result["stdout"] + result["stderr"])
print(result["stdout"] + result["stderr"], end="")
PY
  child_status=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["exit"])' "$process_evidence")
  if [ "$bounded_status" -eq 124 ]; then
    echo "    FAIL     case deadline ${CASE_TIMEOUT_MS}ms expired; process group was reaped"
  fi
  # Optional machine-readable observation of the actual pipeline child status.
  # Keep it separate from assertion counts and the human-readable transcript.
  if [ -n "${STRESS_STATUS_FD:-}" ]; then
    printf '%s\t%d\n' "$name" "$child_status" >&"$STRESS_STATUS_FD" || exit 1
  fi
  exec 197>&-
  unset STRESS_RESULT_FD
  stress_outcome "$record" "$child_status"; scenario_status=$?
  p=$STRESS_PASSED; f=$STRESS_FAILED; u=$STRESS_UNPROVEN
  # Diagnostics retain the assertion text; counts and verdict use the record.
  visible_f=$(grep -c '^    FAIL ' "$transcript" || true)
  if [ "$f" -gt "$visible_f" ]; then
    echo "    FAIL     $((f - visible_f)) failure(s) recorded outside visible assertion output"
  fi
  finished_ns=$(python3 -c 'import time; print(time.monotonic_ns())')
  duration_ms=$(( (finished_ns - started_ns) / 1000000 ))
  evidence=""
  if [ "$f" -gt 0 ]; then
    evidence="failures/$name"
    mkdir -p "$RUN_DIR/$evidence"
    mv "$record" "$RUN_DIR/$evidence/assertions.tsv"
    mv "$transcript" "$RUN_DIR/$evidence/transcript.txt"
    mv "$process_evidence" "$RUN_DIR/$evidence/process.json"
    mv "$case_artifacts" "$RUN_DIR/$evidence/manifests"
  else
    rm -rf "$record" "$transcript" "$process_evidence" "$case_artifacts"
  fi
  printf '%s\t%d\t%d\t%d\t%d\t%d\t%s\n' \
    "$name" "$p" "$f" "$u" "$duration_ms" "$child_status" "$evidence" >> "$RESULT_ROWS"
  TOTAL_P=$((TOTAL_P+p)); TOTAL_F=$((TOTAL_F+f)); TOTAL_U=$((TOTAL_U+u))
  [ "$f" -gt 0 ] && ALL_FAIL+=("$name: $f recorded failure(s)")
  [ "$u" -gt 0 ] && ALL_UNPROVEN+=("$name: $u assertion(s) not proven")
done

REVISION=$(git rev-parse HEAD)
RUNNER_OS=$(uname -s)
RUNNER_ARCH=$(uname -m)
python3 - "$RUN_DIR" "$RESULT_ROWS" "$PWD/stress/catalog.json" "$tier" "$REVISION" "$RUNNER_OS" "$RUNNER_ARCH" <<'PY'
import csv
import json
import pathlib
import sys

run_dir = pathlib.Path(sys.argv[1])
catalog_rows = json.load(open(sys.argv[3]))
catalog = {row["id"]: row for row in catalog_rows}
tier = sys.argv[4]
revision, runner_os, runner_arch = sys.argv[5:8]
rows = []
with open(sys.argv[2], newline="") as source:
    for row in csv.DictReader(source, delimiter="\t"):
        static = catalog[row["id"]]
        rows.append({
            "id": row["id"],
            "contract": static["contract"],
            "capability": static["capability"],
            "tier": static["tier"],
            "passed": int(row["passed"]),
            "failed": int(row["failed"]),
            "unproven": int(row["unproven"]),
            "duration_ms": int(row["duration_ms"]),
            "child_exit": int(row["child_exit"]),
            "failure_evidence": row["evidence"] or None,
        })
with open(run_dir / "summary.json", "w") as output:
    json.dump({
        "revision": revision,
        "runner": {"os": runner_os, "arch": runner_arch},
        "cases": rows,
        "slowest_cases": sorted(
            ({"id": row["id"], "duration_ms": row["duration_ms"]} for row in rows),
            key=lambda row: row["duration_ms"], reverse=True,
        )[:5],
        "not_run": [] if tier == "all" else [
            {key: row[key] for key in ("id", "tier", "capability", "contract")}
            for row in catalog_rows if row["tier"] != tier
        ],
    }, output, indent=2, sort_keys=True)
    output.write("\n")
PY

echo ""
echo "═══════════════════════════════════════════"
printf "  passed   %d\n  failed   %d\n  unproven %d\n" "$TOTAL_P" "$TOTAL_F" "$TOTAL_U"
printf "  results  %s\n" "$RUN_DIR/summary.json"
echo "  slowest:"
python3 - "$RUN_DIR/summary.json" <<'PY'
import json
import sys

for row in json.load(open(sys.argv[1]))["slowest_cases"]:
    print(f"    {row['id']}: {row['duration_ms']}ms")
PY

if [ ${#ALL_FAIL[@]} -gt 0 ]; then
  echo ""; echo "  FAILURES:"; printf '    %s\n' "${ALL_FAIL[@]}"
fi
if [ ${#ALL_UNPROVEN[@]} -gt 0 ]; then
  echo ""
  echo "  NOT PROVEN ON THIS HOST (not passes):"
  printf '    %s\n' "${ALL_UNPROVEN[@]}"
fi
if [ "$tier" != all ]; then
  echo ""
  echo "  NOT RUN (not passes):"
  for skipped_tier in fast load platform; do
    [ "$skipped_tier" = "$tier" ] && continue
    skipped_count=$(python3 - "$skipped_tier" <<'PY'
import json, sys
print(sum(row["tier"] == sys.argv[1] for row in json.load(open("stress/catalog.json"))))
PY
)
    printf '    %s tier: %s catalogued scenario(s)\n' "$skipped_tier" "$skipped_count"
  done
fi
echo "═══════════════════════════════════════════"

[ "$TOTAL_F" -gt 0 ] && exit 1
[ "$TOTAL_P" -eq 0 ] && { echo "nothing was proven here"; exit 2; }
exit 0
