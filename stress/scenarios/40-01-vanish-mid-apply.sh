#!/usr/bin/env bash
# Race: a source file is deleted after sweep's internal re-scan finds it but
# before its own turn in the move loop. `sweep apply` re-scans and re-plans
# in one call. The "plan" and "apply" the family targets are two passes
# inside that single invocation: pass 1 fingerprints every accepted file and
# writes the journal, pass 2 actually moves each one, and journal writes are
# fsynced per entry. The dev-only watcher observes this trial journal, stops
# apply after progress, confirms OS stopped state, and reloads the journal
# before plain rm deletes a still-present, pending source.
#
# What must hold, with the source deleted mid-run:
#   - apply does not crash or leave an inconsistent tree
#   - it reports the failure honestly (nonzero exit)
#   - the journal never claims to have moved the file it could not touch
#   - nothing else is lost: total file count drops by exactly the one file
#     we ourselves deleted, nothing more
#
# Nothing here is real data. Every tree is generated and removed on exit.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

require python3 "builds fixture trees fast and reads the --json plan" || exit 0

N=500

# Letter-only stems so no filename accidentally collides with a sensitive
# marker (a plain zero-padded number can: "batch_1099.csv" reads as a 1099).
build_tree() {
  mkdir -p "$1"
  python3 - "$1" "$N" <<'PY'
import sys, os
d, n = sys.argv[1], int(sys.argv[2])
def letters(i, width=5):
    s = ""
    for _ in range(width):
        s = chr(97 + i % 26) + s
        i //= 26
    return s
for i in range(n):
    open(os.path.join(d, f"batch_{letters(i)}.csv"), "w").close()
PY
}


# The actual journal watcher will verify this earlier scan's target belongs
# to the accepted apply and is still Planned while the OS confirms SIGSTOP.
pick_target() {
  "$SWEEP" "$1" --json 2>/dev/null | python3 -c '
import json, sys
g = next(g for g in json.load(sys.stdin)["details"]["groups"] if g["name"] == "Data")
i = int(len(g["members"]) * 0.75)
print(i)
print(g["members"][i])
'
}

CONTROL="${1:-}"
case "$CONTROL" in ''|--control-late|--control-ignored) ;; *) fail "unknown mid-apply control"; exit 0;; esac
DUMP="$(dirname "$SWEEP")/journal-dump"
if [ ! -x "$DUMP" ]; then
  unproven "mid-apply injection" "dev-only journal watcher unavailable"
  exit 0
fi
export ETUDE_JOURNAL_KEY="$(python3 -c 'import secrets; print(secrets.token_hex(32))')"
W=$(workdir)
PID=""
D=""
cleanup() {
  [ -z "$D" ] || chmod 755 "$D" 2>/dev/null || true
  if [ -n "$PID" ]; then
    kill -CONT "$PID" 2>/dev/null || true
    kill -TERM "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true
  fi
  rm -rf "$W"
  eval "$_stress_state_cleanup"
}
trap cleanup EXIT

# Each control runs the complete witness with its own assertion channel.
# Late apply must be all-unproven; masking a proven fault while stopped must
# make the unchanged failure assertions reject a successful apply.
run_controls() {
  local control result expected output
  for control in late ignored; do
    output="$W/control-$control.out"
    env -u STRESS_WRAP_DEPTH -u STRESS_WRAPPER_PID -u STRESS_RESULT_FD \
      -u STRESS_FAILURE_ARTIFACTS /bin/bash "${BASH_SOURCE[0]}" "--control-$control" \
      >"$output" 2>&1 198<&- 199>&-
    result=$?
    if [ "$control" = late ]; then
      expected=2
      if [ "$result" -eq "$expected" ] && grep -q 'unproven.*injection' "$output"; then
        pass "late injection control is unproven, never a pass or a product failure"
      else
        fail "late injection control wanted unproven exit 2, got $result: $(cat "$output")"
      fi
    elif [ "$result" -eq 1 ] && grep -q 'confirmed stopped' "$output" && grep -q 'FAIL.*exit 0' "$output"; then
      pass "paused ignored-fault control fails the success assertion after a proven injection"
    else
      fail "paused ignored-fault control did not reject success (exit $result): $(cat "$output")"
    fi
  done
}

INJECTED=0
CODE=0
for trial in 1 2 3 4; do
  D="$W/attempt-$trial/Desktop"
  export ETUDE_STATE_DIR="$W/state-$trial"
  mkdir -p "$ETUDE_STATE_DIR"
  build_tree "$D"
  PICK="$(pick_target "$D")"
  IDX="${PICK%%$'\n'*}"
  TARGET="${PICK#*$'\n'}"
  BEFORE=$(find "$D" -type f | wc -l | tr -d ' ')
  snapshot_tree "$D" "$W/original.json"
  "$SWEEP" apply "$D" --yes >"$W/apply.out" 2>"$W/apply.err" &
  PID=$!
  # Force the late interleaving in the control, without guessing milliseconds.
  if [ "$CONTROL" = --control-late ]; then
    wait "$PID" 2>/dev/null || true
  fi
  if ! WITNESS=$("$DUMP" --watch-apply "$ETUDE_STATE_DIR" "$D" "$TARGET" "$PID" 2>"$W/watch.err"); then
    kill -CONT "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true
    PID=""
    [ "$CONTROL" != --control-late ] || break
    continue
  fi
  read -r JOURNAL_MOVED JOURNAL_TOTAL PENDING_INDEX <<< "$WITNESS"
  STOPPED=$(ps -o state= -p "$PID" 2>/dev/null)
  if [ "$JOURNAL_TOTAL" != "$N" ] || [ "$PENDING_INDEX" != "$IDX" ] || \
      [[ "$STOPPED" != *T* ]] || [ ! -f "$TARGET" ] || [ -e "$D/Data/$(basename "$TARGET")" ]; then
    kill -CONT "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true
    PID=""
    continue
  fi

  # A stopped apply cannot relocate between this check and non-idempotent rm.
  if rm -- "$TARGET" && [ ! -e "$TARGET" ] && [ ! -e "$D/Data/$(basename "$TARGET")" ]; then
    INJECTED=1
    # The expected recovery is the original layout minus the one victim,
    # not the already-partially-moved layout observed while paused.
    python3 - "$W/original.json" "$TARGET" > "$W/survivors.json" <<'PY_SURVIVORS'
import json, os, sys
manifest = json.load(open(sys.argv[1]))
manifest['entries'] = [e for e in manifest['entries'] if e['path'] != os.path.basename(sys.argv[2])]
json.dump(manifest, sys.stdout, ensure_ascii=False, sort_keys=True, separators=(',', ':'))
sys.stdout.write('\n')
PY_SURVIVORS
  else
    kill -CONT "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true
    PID=""
    continue
  fi
  pass "file vanishes mid-apply: deleted pending source at group position $IDX while apply was confirmed stopped ($JOURNAL_MOVED/$N journalled moves)"
  if [ "$CONTROL" = --control-ignored ]; then
    # Simulate an apply that ignores a proven deletion: restore the empty
    # synthetic source while still paused, then let the real apply succeed.
    touch "$TARGET"
  fi
  if ! kill -CONT "$PID"; then
    fail "could not resume apply after a proven deletion"
    exit 0
  fi
  wait "$PID"; CODE=$?
  PID=""
  break
done
if [ "$INJECTED" != 1 ]; then
  unproven "file vanishes mid-apply injection" "no confirmed stopped apply with a pending source across four trials: $(cat "$W/watch.err")"
  exit 0
fi

# The witnessed deletion decides that the test ran. A silent success is a
# failure, never a reason to relabel the attempt as unproven.
assert_eq 3 "$CODE" "apply reported the deletion as a failure, not a silent success (exit $CODE)"

if [ -s "$W/apply.err" ] && grep -qi "sweep:" "$W/apply.err"; then
  pass "the failure was printed, not swallowed"
else
  fail "apply exited $CODE but printed nothing identifiable on stderr: $(cat "$W/apply.err")"
fi

# The only file that should be missing anywhere in the tree is the one we
# ourselves deleted. Nothing else the tool touched should have vanished.
AFTER=$(find "$D" -type f | wc -l | tr -d ' ')
assert_eq "$((BEFORE - 1))" "$AFTER" "exactly one file missing after the crash (the one we deleted, nothing more)"

MOVED=$(find "$D" -mindepth 2 -type f 2>/dev/null | wc -l | tr -d ' ')
echo "    ($MOVED files had already landed in the destination group before the abort)"

# The journal must not claim the deleted file as moved. If it lied, `undo`
# would either try to restore a file that was never really relocated (and
# find nothing at the recorded destination, a story that would look
# identical to "already gone", masking the lie) or the restored count would
# not match what we can independently verify was actually moved.
UNDO_OUT=$("$SWEEP" undo 2>&1); UNDO_CODE=$?
RESTORED=$(echo "$UNDO_OUT" | grep -o 'Restored [0-9]*' | grep -o '[0-9]*')
if [ "$MOVED" -eq 0 ]; then
  assert_eq 2 "$UNDO_CODE" "undo reports that no journal exists when the deletion stopped apply before any move"
else
  assert_eq 0 "$UNDO_CODE" "undo succeeds after a partial apply"
  assert_eq "$MOVED" "${RESTORED:-BAD}" "undo restored exactly the files that were actually moved before the crash"
fi

if echo "$UNDO_OUT" | grep -qi "already gone"; then
  fail "undo reported files as 'already gone'. The journal claimed a move that never happened: $UNDO_OUT"
else
  pass "no phantom entries: undo never claimed anything was moved-then-missing"
fi

FINAL=$(find "$D" -type f | wc -l | tr -d ' ')
assert_eq "$((BEFORE - 1))" "$FINAL" "after undo, every survivor is back where it started"

snapshot_tree "$D" "$W/recovered.json"
assert_snapshot_eq "$W/survivors.json" "$W/recovered.json" "undo restored exact survivor paths and bytes"
[ -n "$CONTROL" ] || run_controls
true
