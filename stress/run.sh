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
  record=$(mktemp "${TMPDIR:-/tmp}/etudes-stress-run-record-XXXXXX")
  transcript=$(mktemp "${TMPDIR:-/tmp}/etudes-stress-transcript-XXXXXX")
  exec 197>>"$record"
  # A nonzero pipeline must not invoke the inherited ERR trap before we
  # save PIPESTATUS: that trap can replace an unproven child's 2 with 1.
  if STRESS_RESULT_FD=197 SCENARIO="$name" bash "$s" 2>&1 | tee "$transcript"; then
    child_status=${PIPESTATUS[0]}
  else
    child_status=${PIPESTATUS[0]}
  fi
  # Optional machine-readable observation of the actual pipeline child status.
  # Keep it separate from assertion counts and the human-readable transcript.
  if [ -n "${STRESS_STATUS_FD:-}" ]; then
    printf '%s\t%d\n' "$name" "$child_status" >&"$STRESS_STATUS_FD" || exit 1
  fi
  exec 197>&-
  unset STRESS_RESULT_FD
  stress_outcome "$record" "$child_status"; scenario_status=$?
  rm -f "$record"
  p=$STRESS_PASSED; f=$STRESS_FAILED; u=$STRESS_UNPROVEN
  # Diagnostics retain the assertion text; counts and verdict use the record.
  visible_f=$(grep -c '^    FAIL ' "$transcript" || true)
  if [ "$f" -gt "$visible_f" ]; then
    echo "    FAIL     $((f - visible_f)) failure(s) recorded outside visible assertion output"
  fi
  rm -f "$transcript"
  TOTAL_P=$((TOTAL_P+p)); TOTAL_F=$((TOTAL_F+f)); TOTAL_U=$((TOTAL_U+u))
  [ "$f" -gt 0 ] && ALL_FAIL+=("$name: $f recorded failure(s)")
  [ "$u" -gt 0 ] && ALL_UNPROVEN+=("$name: $u assertion(s) not proven")
done

echo ""
echo "═══════════════════════════════════════════"
printf "  passed   %d\n  failed   %d\n  unproven %d\n" "$TOTAL_P" "$TOTAL_F" "$TOTAL_U"

if [ ${#ALL_FAIL[@]} -gt 0 ]; then
  echo ""; echo "  FAILURES:"; printf '    %s\n' "${ALL_FAIL[@]}"
fi
if [ ${#ALL_UNPROVEN[@]} -gt 0 ]; then
  echo ""
  echo "  NOT PROVEN ON THIS HOST (not passes):"
  printf '    %s\n' "${ALL_UNPROVEN[@]}"
fi
echo "═══════════════════════════════════════════"

[ "$TOTAL_F" -gt 0 ] && exit 1
[ "$TOTAL_P" -eq 0 ] && { echo "nothing was proven here"; exit 2; }
exit 0
