#!/usr/bin/env bash
# A caller that retries and chains operations reads two things from sweep:
# the `reason` on a result, and `sweep status` for an operation it started
# earlier. Both must stay true when the operation did not end cleanly.
#
#   - An apply killed part-way is reported by `status` as interrupted, with
#     counts that agree with the files actually moved. After `sweep undo` the
#     same lookup reports it reversed.
#   - Of two applies racing on one folder, the one that loses is not refused
#     up front: it starts, collides and stops part-way. Its reason says so
#     (`operation_incomplete`, change something first), its result names a
#     durable operation that `status` reports as interrupted, and `sweep undo`
#     restores the folder exactly. It is recoverable, not retryable as-is.
#   - `status --id` for an id no journal holds is exit 1, `unknown_operation`.
#   - No status output names a file from the folder.
#
# The kill is witnessed, not timed: the dev-only journal watcher stops apply
# after real journal progress, the OS confirms the stop, and only then is the
# process killed. A missed window is unproven, never a pass or a failure.
#
# Nothing here is real data. Every tree is generated and removed on exit.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
# No tool in this scenario may wait on a terminal.
exec </dev/null

require python3 "builds fixture names and reads --json results" || exit 0

W=$(workdir)
PID=""
cleanup() {
  if [ -n "$PID" ]; then
    kill -CONT "$PID" 2>/dev/null || true
    kill -KILL "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true
  fi
  rm -rf "$W"
  eval "$_stress_state_cleanup"
}
trap cleanup EXIT
export ETUDE_JOURNAL_KEY="$(python3 -c 'import secrets; print(secrets.token_hex(32))')"

N=500

# Every name carries one marker that appears nowhere else in this scenario's
# paths, so a leaked name is recognisable in any output.
MARK="refusalfixture"
make_tree() {
  local d="$1" i
  mkdir -p "$d"
  for i in $(seq 1 "$N"); do
    printf 'synthetic screenshot payload %s\n' "$i" \
      > "$d/Screenshot 2026-01-01 at 10.00.00 AM ($MARK $i).png"
  done
}

# json_get FILE EXPRESSION: evaluate a Python expression over the parsed
# result `d`. Prints nothing and returns 1 when the file is not one result.
json_get() {
  python3 - "$1" "$2" <<'PY'
import json, sys
try:
    d = json.load(open(sys.argv[1]))
    value = eval(sys.argv[2], {"d": d})
except Exception:
    sys.exit(1)
print("" if value is None else value)
PY
}

# names_in FILE...: how many of the fixture's file names occur in the output.
names_in() {
  python3 - "$N" "$MARK" "$@" <<'PY'
import sys
count, mark = int(sys.argv[1]), sys.argv[2]
text = "".join(open(path, errors="replace").read() for path in sys.argv[3:])
names = [f"Screenshot 2026-01-01 at 10.00.00 AM ({mark} {i}).png" for i in range(1, count + 1)]
print(sum(name in text for name in names) + (mark in text))
PY
}

DUMP="$(dirname "$SWEEP")/journal-dump"
STATUS_OUTPUTS=()

# --- 1. an apply killed part-way ----------------------------------------
KILLED=0
if [ ! -x "$DUMP" ]; then
  unproven "apply killed part-way" "dev-only journal watcher unavailable"
else
  for trial in 1 2 3 4; do
    D="$W/kill-$trial/Desktop"
    export ETUDE_STATE_DIR="$W/state-kill-$trial"
    mkdir -p "$ETUDE_STATE_DIR"
    make_tree "$D"
    snapshot_tree "$D" "$W/kill-before.json"
    "$SWEEP" "$D" --json >"$W/plan.json" 2>/dev/null
    # A member three quarters through the plan is still pending when the
    # watcher first sees progress.
    TARGET=$(json_get "$W/plan.json" \
      '(lambda m: m[len(m) * 3 // 4])(max(d["details"]["groups"], key=lambda g: len(g["members"]))["members"])')
    if [ -z "$TARGET" ]; then
      fail "kill mid-apply: the scan produced no group to apply"
      break
    fi
    "$SWEEP" apply "$D" --yes >"$W/apply.out" 2>"$W/apply.err" &
    PID=$!
    if ! WITNESS=$("$DUMP" --watch-apply "$ETUDE_STATE_DIR" "$D" "$TARGET" "$PID" 2>"$W/watch.err"); then
      kill -CONT "$PID" 2>/dev/null || true
      wait "$PID" 2>/dev/null || true
      PID=""
      continue
    fi
    read -r JOURNAL_MOVED JOURNAL_TOTAL _ <<< "$WITNESS"
    STOPPED=$(ps -o state= -p "$PID" 2>/dev/null)
    if [[ "$STOPPED" != *T* ]] || [ "$JOURNAL_TOTAL" != "$N" ]; then
      kill -CONT "$PID" 2>/dev/null || true
      wait "$PID" 2>/dev/null || true
      PID=""
      continue
    fi
    # Counted while the process cannot move anything.
    MOVED_ON_DISK=$(find "$D" -mindepth 2 -type f | wc -l | tr -d ' ')
    # Apply moves a file and then records it. A stop between the two is a
    # real state, but not the one the count claim is about; while trials
    # remain, let this apply finish and look for a stop on a record boundary.
    if [ "$MOVED_ON_DISK" != "$JOURNAL_MOVED" ] && [ "$trial" -lt 4 ]; then
      echo "    (trial $trial stopped between a move and its record: $MOVED_ON_DISK on disk, $JOURNAL_MOVED journalled; trying again)"
      kill -CONT "$PID" 2>/dev/null || true
      wait "$PID" 2>/dev/null || true
      PID=""
      continue
    fi
    pass "kill mid-apply: apply is confirmed stopped with $JOURNAL_MOVED of $N moves journalled"
    if kill -9 "$PID" 2>/dev/null; then
      pass "kill mid-apply: SIGKILL was delivered to the stopped apply"
    else
      fail "kill mid-apply: could not deliver SIGKILL to the stopped apply"
    fi
    wait "$PID" 2>/dev/null; KILL_STATUS=$?
    PID=""
    [ "$KILL_STATUS" -ne 0 ] \
      && pass "kill mid-apply: the killed apply exited non-zero ($KILL_STATUS)" \
      || fail "kill mid-apply: the killed apply exited zero"
    KILLED=1
    break
  done
  if [ "$KILLED" != 1 ] && [ "$FAILED" -eq 0 ]; then
    unproven "apply killed part-way" "no confirmed stopped apply with pending moves across four trials: $(cat "$W/watch.err" 2>/dev/null)"
  fi
fi

if [ "$KILLED" = 1 ]; then
  if [ "$MOVED_ON_DISK" -gt 0 ] && [ "$MOVED_ON_DISK" -lt "$N" ]; then
    pass "kill mid-apply: the tree is really part-way ($MOVED_ON_DISK of $N files moved)"
  else
    fail "kill mid-apply: the tree is not part-way ($MOVED_ON_DISK of $N files moved)"
  fi

  snapshot_tree "$D" "$W/kill-partial.json"
  S="$W/status-interrupted"; STATUS_OUTPUTS+=("$S.json" "$S.err")
  "$SWEEP" status "$D" --json >"$S.json" 2>"$S.err"; CODE=$?
  assert_eq 0 "$CODE" "interrupted: status by folder finds the operation (exit $CODE)"
  assert_eq interrupted "$(json_get "$S.json" 'd["details"]["state"]')" "interrupted: status reports the killed apply as interrupted"
  STATUS_MOVED=$(json_get "$S.json" 'd["details"]["counts"]["moved"]')
  STATUS_PLANNED=$(json_get "$S.json" 'd["details"]["counts"]["planned"]')
  if [ "$STATUS_MOVED" = "$MOVED_ON_DISK" ]; then
    pass "interrupted: status counts $STATUS_MOVED moved, the number of files actually moved"
  elif [ "${STATUS_MOVED:-x}" = "$((MOVED_ON_DISK - 1))" ]; then
    # Apply moves a file and then records it. A stop between the two leaves
    # one move the journal has not written down, and status reads the journal.
    unproven "interrupted: status moved count equals the files actually moved" \
      "apply was stopped between a move and its record: $MOVED_ON_DISK moved on disk, $STATUS_MOVED journalled"
  else
    fail "interrupted: status counts ${STATUS_MOVED:-nothing} moved but $MOVED_ON_DISK files were actually moved"
  fi
  assert_eq "$N" "$(( ${STATUS_MOVED:-0} + ${STATUS_PLANNED:-0} ))" "interrupted: moved and still-planned counts add up to every planned file"
  assert_eq 0 "$(json_get "$S.json" 'd["details"]["counts"]["reversed"]')" "interrupted: nothing is counted as reversed before undo"
  assert_eq "done not_needed" "$(json_get "$S.json" 'd["reason"]["code"] + " " + d["reason"]["retry"]')" "interrupted: the lookup itself succeeded and says so in its reason"
  OPERATION=$(json_get "$S.json" 'd["details"]["operation_id"]')
  if [ -n "$OPERATION" ] && [ -f "$ETUDE_STATE_DIR/sweep-$OPERATION.journal" ]; then
    pass "interrupted: status names the durable id of the journal on disk"
  else
    fail "interrupted: status operation id '${OPERATION}' names no journal in the state directory"
  fi

  S="$W/status-by-id"; STATUS_OUTPUTS+=("$S.json" "$S.err")
  "$SWEEP" status --id "$OPERATION" --json >"$S.json" 2>"$S.err"; CODE=$?
  assert_eq "0 interrupted $STATUS_MOVED" \
    "$CODE $(json_get "$S.json" 'd["details"]["state"] + " " + str(d["details"]["counts"]["moved"])')" \
    "interrupted: status by id agrees with status by folder"
  snapshot_tree "$D" "$W/kill-after-status.json"
  assert_snapshot_eq "$W/kill-partial.json" "$W/kill-after-status.json" "interrupted: status moved nothing"

  assert_exit 0 "reversed: undo restores the killed apply" -- "$SWEEP" undo "$D"
  snapshot_tree "$D" "$W/kill-after-undo.json"
  assert_snapshot_eq "$W/kill-before.json" "$W/kill-after-undo.json" "reversed: undo restored every original path and byte"
  S="$W/status-reversed"; STATUS_OUTPUTS+=("$S.json" "$S.err")
  "$SWEEP" status "$D" --json >"$S.json" 2>"$S.err"; CODE=$?
  assert_eq "0 reversed" "$CODE $(json_get "$S.json" 'd["details"]["state"]')" "reversed: status reports the operation as reversed after undo"
  STATUS_REVERSED=$(json_get "$S.json" 'd["details"]["counts"]["reversed"]')
  if [ "$STATUS_MOVED" = "$MOVED_ON_DISK" ]; then
    assert_eq "$MOVED_ON_DISK" "$STATUS_REVERSED" "reversed: status counts every moved file as reversed"
  else
    unproven "reversed: status counts every moved file as reversed" \
      "apply was stopped between a move and its record: $MOVED_ON_DISK files were put back, ${STATUS_REVERSED:-nothing} counted"
  fi
  assert_eq 0 "$(json_get "$S.json" 'd["details"]["counts"]["moved"]')" "reversed: status counts nothing as still moved"
fi

# --- 2. an id no journal holds ------------------------------------------
# Asked twice: beside real journals, and of an empty state directory.
S="$W/status-unknown"; STATUS_OUTPUTS+=("$S.json" "$S.err")
"$SWEEP" status --id operation-that-does-not-exist --json >"$S.json" 2>"$S.err"; CODE=$?
assert_eq "1 unknown_operation never" \
  "$CODE $(json_get "$S.json" 'd["reason"]["code"] + " " + d["reason"]["retry"]')" \
  "unknown id: exit 1 with reason unknown_operation, never retry"
export ETUDE_STATE_DIR="$W/state-empty"
mkdir -p "$ETUDE_STATE_DIR"
S="$W/status-unknown-empty"; STATUS_OUTPUTS+=("$S.json" "$S.err")
"$SWEEP" status --id operation-that-does-not-exist --json >"$S.json" 2>"$S.err"; CODE=$?
assert_eq "1 unknown_operation never" \
  "$CODE $(json_get "$S.json" 'd["reason"]["code"] + " " + d["reason"]["retry"]')" \
  "unknown id: the same answer when no journal exists at all"
assert_eq 0 "$(find "$ETUDE_STATE_DIR" -mindepth 1 | wc -l | tr -d ' ')" "unknown id: the lookup wrote nothing to an empty state directory"

# --- 3. two applies on one folder at once ---------------------------------
D="$W/contend/Desktop"
export ETUDE_STATE_DIR="$W/state-contend"
mkdir -p "$ETUDE_STATE_DIR"
make_tree "$D"
snapshot_tree "$D" "$W/contend-before.json"
"$SWEEP" apply "$D" --yes --json >"$W/first.json" 2>"$W/first.err" & FIRST=$!
"$SWEEP" apply "$D" --yes --json >"$W/second.json" 2>"$W/second.err" & SECOND=$!
wait "$FIRST"; FIRST_STATUS=$?
wait "$SECOND"; SECOND_STATUS=$?
WINS=0
[ "$FIRST_STATUS" -eq 0 ] && WINS=$((WINS + 1))
[ "$SECOND_STATUS" -eq 0 ] && WINS=$((WINS + 1))
assert_eq 1 "$WINS" "contention: exactly one of two applies on one folder succeeds (exits $FIRST_STATUS and $SECOND_STATUS)"
LOSER="$W/first.json"
[ "$FIRST_STATUS" -eq 0 ] && LOSER="$W/second.json"
LOSER_REASON=$(json_get "$LOSER" \
  'd["reason"]["code"] + " retry=" + d["reason"]["retry"] + " subject.kind=" + str(d["reason"].get("subject", {}).get("kind"))')
if [ "$WINS" != 1 ]; then
  :
elif [ "$(json_get "$LOSER" 'd["reason"]["code"]')" = nothing_to_do ]; then
  unproven "contention: the losing apply's reason" "the first apply finished before the second looked; they never overlapped"
else
  # The loser was not refused before it started, so "wait and send the same
  # request" would be the wrong advice. It stopped part-way: recoverable, not
  # retryable as-is.
  assert_eq "operation_incomplete retry=after_change subject.kind=journal" "$LOSER_REASON" \
    "contention: the loser says it stopped part-way and is recoverable, not retryable as-is"
  LOSER_OPERATION=$(json_get "$LOSER" 'd["recovery"]["operation"]')
  if [ -n "$LOSER_OPERATION" ]; then
    pass "contention: the loser's result carries a durable recovery operation id"
    S="$W/status-loser"; STATUS_OUTPUTS+=("$S.json" "$S.err")
    "$SWEEP" status --id "$LOSER_OPERATION" --json >"$S.json" 2>"$S.err"; CODE=$?
    assert_eq "0 interrupted" "$CODE $(json_get "$S.json" 'd["details"]["state"]')" \
      "contention: status reports the loser's operation as interrupted"
  else
    fail "contention: the loser's result carries no recovery operation id"
  fi
fi
assert_intact "$D" "$N" "contention: every file is still somewhere in its folder"
S="$W/status-contend"; STATUS_OUTPUTS+=("$S.json" "$S.err")
"$SWEEP" status "$D" --json >"$S.json" 2>"$S.err"; CODE=$?
assert_eq 0 "$CODE" "contention: status still answers for the contended folder (exit $CODE)"
# Undo until nothing is left to reverse: the winner and the loser each have
# their own journal for this folder.
UNDO_RUNS=0; UNDO_CODE=0
while [ "$UNDO_RUNS" -lt 4 ]; do
  "$SWEEP" undo "$D" >"$W/contend-undo.out" 2>&1; UNDO_CODE=$?
  [ "$UNDO_CODE" -eq 0 ] || break
  UNDO_RUNS=$((UNDO_RUNS + 1))
done
assert_eq 1 "$UNDO_CODE" "contention: undo ends with nothing left to reverse after $UNDO_RUNS restoring run(s)"
snapshot_tree "$D" "$W/contend-after-undo.json"
assert_snapshot_eq "$W/contend-before.json" "$W/contend-after-undo.json" "contention: undo recovers every original path and byte"

# --- 4. status never names a file ---------------------------------------
# The detector is proven first on output that must contain names.
"$SWEEP" "$D" --json >"$W/plan-with-names.json" 2>/dev/null
"$SWEEP" "$W/kill-1/Desktop" --json >>"$W/plan-with-names.json" 2>/dev/null
if [ "$(names_in "$W/plan-with-names.json")" -gt 0 ]; then
  pass "names: the detector finds fixture names in a scan plan, which lists them"
  assert_eq 0 "$(names_in "${STATUS_OUTPUTS[@]}")" "names: no status output contains a file name from the fixture (${#STATUS_OUTPUTS[@]} outputs checked)"
else
  unproven "names: no status output contains a file name" "the detector found no names in a scan plan either"
fi
true
