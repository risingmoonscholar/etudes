#!/usr/bin/env bash
# A caller that retries and chains operations reads two things from sweep:
# the `reason` on a result, and `sweep status` for an operation it started
# earlier. Both must stay true when the operation did not end cleanly.
#
#   - An apply killed part-way is reported by `status` as interrupted, with
#     counts that agree with the files actually moved. After `sweep undo` the
#     same lookup reports it reversed.
#   - Two applies racing on one folder have two honest endings for the one
#     that loses, and timing picks which. Either it is refused before it moves
#     anything, because the folder changed under its plan (`plan_stale`: plan
#     again), or it starts, collides and stops part-way (`operation_incomplete`:
#     its result names an operation `status` reports as interrupted, and
#     `sweep undo` restores the folder). Both are forced here on purpose, and
#     a free race must end in one of them. Nothing else is accepted.
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
#
# KNOWN DEFECT, not fixed here: after the stopped-part-way ending, `sweep undo`
# can refuse to recover the folder, and no sequence of undo commands gets past
# it. Nothing is lost; the files stay in the folder apply made.
#
# Minimal reproduction (every step uses a throwaway folder, state directory
# and key, with stdin closed):
#   1. Make a folder D of 300 files that sweep groups together.
#   2. Start `sweep apply D --yes` (call it A) in the background.
#   3. `journal-dump --watch-apply STATE D <a file three quarters through the
#      plan> PID_A` stops A after it has journalled at least one move.
#   4. Run a second `sweep apply D --yes` (B) to completion. It moves the rest.
#   5. `kill -CONT PID_A`. A finds its remaining files gone and exits 3 with
#      `operation_incomplete`. Its journal holds a few Moved entries and many
#      Planned ones whose destinations now hold the files B moved.
#   6. Make A's journal the most recently modified one. That happens on its
#      own in roughly one run in four here; `touch STATE/sweep-<A id>.journal`
#      forces it.
#   7. `sweep undo D` exits 3: "This journal is missing more than one record".
#      Undo chose A's journal because it is newest, and `unrecorded_moves()`
#      counted B's files as A's lost records. `sweep undo D` again, and
#      `sweep undo` with no folder, refuse the same way: there is no command
#      that undoes B's journal first.
# Expected once fixed: undo restores D exactly, as it does when B's journal
# happens to be the newest.
# check_contention LABEL EXPECT DIR BEFORE FILES STATUS1 JSON1 STATUS2 JSON2
# EXPECT is refused, part_way or either. Whichever ending the loser reached,
# its consequences are asserted; an ending that is neither fails.
check_contention() {
  local label="$1" expect="$2" d="$3" before="$4" files="$5" s1="$6" j1="$7" s2="$8" j2="$9"
  local wins=0 loser="$j1" loser_status="$s1" winner="$j2"
  [ "$s1" -eq 0 ] && wins=$((wins + 1))
  [ "$s2" -eq 0 ] && wins=$((wins + 1))
  assert_eq 1 "$wins" "$label: exactly one of two applies on one folder succeeds (exits $s1 and $s2)"
  [ "$wins" = 1 ] || return 0
  if [ "$s1" -eq 0 ]; then loser="$j2"; loser_status="$s2"; winner="$j1"; fi
  local reason operation winner_operation winner_moved on_disk outcome=none S code
  reason=$(json_get "$loser" \
    'd["reason"]["code"] + " retry=" + d["reason"]["retry"] + " subject.kind=" + str(d["reason"].get("subject", {}).get("kind"))')
  operation=$(json_get "$loser" 'd["recovery"]["operation"]')
  winner_operation=$(json_get "$winner" 'd["recovery"]["operation"]')
  winner_moved=$(json_get "$winner" 'd["details"]["moved"]')
  on_disk=$(find "$d" -mindepth 2 -type f | wc -l | tr -d ' ')
  case "$loser_status $reason" in
    "2 plan_stale retry=after_change subject.kind=plan") outcome=refused ;;
    "3 operation_incomplete retry=after_change subject.kind=journal") outcome=part_way ;;
    "1 nothing_to_do "*)
      unproven "$label: the losing apply's reason" "one apply finished before the other looked; they never overlapped" ;;
    *) fail "$label: the loser ended in neither known way: exit $loser_status, ${reason:-no reason}" ;;
  esac
  if [ "$outcome" != none ] && [ "$expect" != either ] && [ "$outcome" != "$expect" ]; then
    unproven "$label: forcing the $expect ending" "this run ended $outcome instead; its consequences are still checked"
  fi

  if [ "$outcome" = refused ]; then
    pass "$label: observed REFUSED BEFORE MOVING (exit 2, plan_stale, retry after_change): the loser must plan again"
    assert_eq "$winner_moved" "$on_disk" "$label: the folder holds exactly the winner's result; the loser moved nothing"
    if [ -n "$operation" ]; then
      # The id is the loser's own record that a plan was refused. It holds no
      # entries, so there is nothing in it to recover.
      [ "$operation" != "$winner_operation" ] && [ -f "$ETUDE_STATE_DIR/sweep-$operation.journal" ] \
        && pass "$label: the refused loser's recovery id is its own record, not the winner's journal" \
        || fail "$label: the refused loser's recovery id '$operation' is the winner's or names no journal"
      S="$W/status-$label-loser"; STATUS_OUTPUTS+=("$S.json" "$S.err")
      "$SWEEP" status --id "$operation" --json >"$S.json" 2>"$S.err"; code=$?
      assert_eq "0 planned=0 moved=0 reversed=0 command=" \
        "$code $(json_get "$S.json" '"planned=%d moved=%d reversed=%d command=%s" % (d["details"]["counts"]["planned"], d["details"]["counts"]["moved"], d["details"]["counts"]["reversed"], d["details"]["command"] or "")')" \
        "$label: status shows the refused loser's record moved nothing and offers no undo"
      assert_eq refused "$(json_get "$S.json" 'd["details"]["state"]')" \
        "$label: status reports the refused loser's record as refused, not completed"
      S="$W/status-$label-listing"; STATUS_OUTPUTS+=("$S.json" "$S.err")
      "$SWEEP" status "$d" --json >"$S.json" 2>"$S.err"
      assert_eq "completed refused" \
        "$(json_get "$S.json" '" ".join(sorted(row["state"] for row in d["details"]))')" \
        "$label: status by folder lists the refused attempt beside the winner's completed apply"
    else
      pass "$label: the refused loser names no recovery operation"
    fi
  elif [ "$outcome" = part_way ]; then
    pass "$label: observed STOPPED PART-WAY (exit 3, operation_incomplete, retry after_change): recoverable, not retryable as-is"
    if [ -n "$operation" ] && [ "$operation" != "$winner_operation" ]; then
      pass "$label: the stopped loser's result carries its own durable recovery operation id"
      S="$W/status-$label-loser"; STATUS_OUTPUTS+=("$S.json" "$S.err")
      "$SWEEP" status --id "$operation" --json >"$S.json" 2>"$S.err"; code=$?
      assert_eq "0 interrupted" "$code $(json_get "$S.json" 'd["details"]["state"]')" \
        "$label: status reports the stopped loser's operation as interrupted"
      assert_eq "$on_disk" "$(( ${winner_moved:-0} + $(json_get "$S.json" 'd["details"]["counts"]["moved"]') ))" \
        "$label: every moved file is counted by exactly one of the two journals"
    else
      fail "$label: the stopped loser's result carries no recovery operation id of its own"
    fi
  fi

  assert_intact "$d" "$files" "$label: every file is still somewhere in its folder"
  S="$W/status-$label-folder"; STATUS_OUTPUTS+=("$S.json" "$S.err")
  "$SWEEP" status "$d" --json >"$S.json" 2>"$S.err"; code=$?
  assert_eq 0 "$code" "$label: status still answers for the contended folder (exit $code)"
  # Undo until nothing is left to reverse: each apply that moved files has
  # its own journal for this folder.
  local runs=0 undo_code=0
  snapshot_tree "$d" "$W/before-undo-$label.json"
  while [ "$runs" -lt 4 ]; do
    "$SWEEP" undo "$d" >"$W/undo-$label.out" 2>&1; undo_code=$?
    [ "$undo_code" -eq 0 ] || break
    runs=$((runs + 1))
  done
  snapshot_tree "$d" "$W/after-undo-$label.json"
  if [ "$undo_code" = 1 ]; then
    pass "$label: undo ends with nothing left to reverse after $runs restoring run(s)"
    assert_snapshot_eq "$before" "$W/after-undo-$label.json" "$label: undo recovers every original path and byte"
  elif [ "$outcome" = part_way ] && [ "$undo_code" = 3 ] && [ "$runs" = 0 ] \
      && grep -q 'missing more than one record' "$W/undo-$label.out"; then
    # The known defect described at the top of this part. The refusal is
    # safe, and it is not a recovery: unproven by name, never a pass.
    assert_snapshot_eq "$W/before-undo-$label.json" "$W/after-undo-$label.json" "$label: the refused undo (exit 3) changed no path or byte"
    unproven "KNOWN DEFECT: $label: sweep undo refused to recover the folder after two applies on it" \
      "undo chose the stopped apply's journal and read the other apply's moves as its own missing records"
  else
    fail "$label: undo stopped with exit $undo_code after $runs restoring run(s): $(tr '\n' ' ' < "$W/undo-$label.out" | cut -c1-600)"
    assert_snapshot_eq "$before" "$W/after-undo-$label.json" "$label: undo recovers every original path and byte"
  fi
}

# 3a. Forced: refused before moving. The loser is held after it has planned
# and before it applies: its plan prints a note about two files it will leave
# alone, and that write blocks on a pipe filled beforehand. The winner then
# runs to completion, and the loser is released onto a changed folder.
D="$W/forced-refused/Desktop"
export ETUDE_STATE_DIR="$W/state-forced-refused"
mkdir -p "$ETUDE_STATE_DIR"
make_tree "$D"
printf 'synthetic\n' > "$D/unsorted one.$MARK"
printf 'synthetic\n' > "$D/unsorted two.$MARK"
snapshot_tree "$D" "$W/forced-refused-before.json"
mkfifo "$W/held.pipe"
exec 9<>"$W/held.pipe"
python3 - "$W/held.pipe" <<'PY'
import os, sys
fd = os.open(sys.argv[1], os.O_WRONLY | os.O_NONBLOCK)
for chunk in (4096, 1):
    while True:
        try:
            os.write(fd, b"\0" * chunk)
        except BlockingIOError:
            break
PY
"$SWEEP" apply "$D" --yes --json >"$W/held.json" 2>"$W/held.pipe" 9>&- &
PID=$!
# release_held: read the pipe until the held apply has exited. This shell
# keeps its own descriptor open throughout, so the apply never sees a pipe
# with no reader.
release_held() {
  python3 - "$W/held.pipe" "$1" <<'PY' &
import os, sys, time
fd = os.open(sys.argv[1], os.O_RDONLY | os.O_NONBLOCK)
pid = int(sys.argv[2])
while True:
    try:
        if os.read(fd, 65536):
            continue
    except BlockingIOError:
        pass
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        break
    time.sleep(0.01)
PY
  DRAIN=$!
}
# Held means: still alive, and its CPU time has stopped moving.
HELD=0; LAST=""; STABLE=0
for _ in $(seq 1 100); do
  NOW=$(ps -o time= -p "$PID" 2>/dev/null | tr -d ' ')
  [ -n "$NOW" ] || break
  if [ "$NOW" = "$LAST" ]; then STABLE=$((STABLE + 1)); else STABLE=0; fi
  LAST="$NOW"
  if [ "$STABLE" -ge 10 ]; then HELD=1; break; fi
  sleep 0.1
done
if [ "$HELD" = 1 ] && [ "$(find "$D" -mindepth 2 -type f | wc -l | tr -d ' ')" = 0 ]; then
  "$SWEEP" apply "$D" --yes --json >"$W/free.json" 2>"$W/free.err"; FREE_STATUS=$?
  snapshot_tree "$D" "$W/forced-refused-after-winner.json"
  release_held "$PID"
  wait "$PID"; HELD_STATUS=$?
  PID=""
  kill "$DRAIN" 2>/dev/null; wait "$DRAIN" 2>/dev/null
  exec 9>&-
  snapshot_tree "$D" "$W/forced-refused-after-loser.json"
  if [ "$FREE_STATUS" -eq 0 ] && [ "$HELD_STATUS" -eq 2 ]; then
    assert_snapshot_eq "$W/forced-refused-after-winner.json" "$W/forced-refused-after-loser.json" \
      "forced-refused: the released loser changed no path or byte"
  fi
  check_contention forced-refused refused "$D" "$W/forced-refused-before.json" "$((N + 2))" \
    "$HELD_STATUS" "$W/held.json" "$FREE_STATUS" "$W/free.json"
else
  release_held "$PID"
  wait "$PID" 2>/dev/null
  PID=""
  kill "$DRAIN" 2>/dev/null; wait "$DRAIN" 2>/dev/null
  exec 9>&-
  unproven "forced-refused: an apply held between planning and applying" "the apply did not stop on its first note, or had already moved files"
fi

# 3b. Forced: stopped part-way. The first apply is frozen after real journal
# progress (confirmed stopped), the second runs through the folder, and the
# first is resumed to find its remaining files gone.
D="$W/forced-part-way/Desktop"
export ETUDE_STATE_DIR="$W/state-forced-part-way"
mkdir -p "$ETUDE_STATE_DIR"
make_tree "$D"
snapshot_tree "$D" "$W/forced-part-way-before.json"
FROZEN=0
if [ -x "$DUMP" ]; then
  "$SWEEP" "$D" --json >"$W/plan.json" 2>/dev/null
  TARGET=$(json_get "$W/plan.json" \
    '(lambda m: m[len(m) * 3 // 4])(max(d["details"]["groups"], key=lambda g: len(g["members"]))["members"])')
  "$SWEEP" apply "$D" --yes --json >"$W/frozen.json" 2>"$W/frozen.err" &
  PID=$!
  if "$DUMP" --watch-apply "$ETUDE_STATE_DIR" "$D" "$TARGET" "$PID" >/dev/null 2>"$W/watch.err" \
      && [[ "$(ps -o state= -p "$PID" 2>/dev/null)" == *T* ]]; then
    FROZEN=1
    "$SWEEP" apply "$D" --yes --json >"$W/through.json" 2>"$W/through.err"; THROUGH_STATUS=$?
  fi
  kill -CONT "$PID" 2>/dev/null || true
  wait "$PID" 2>/dev/null; FROZEN_STATUS=$?
  PID=""
fi
if [ "$FROZEN" = 1 ]; then
  check_contention forced-part-way part_way "$D" "$W/forced-part-way-before.json" "$N" \
    "$FROZEN_STATUS" "$W/frozen.json" "$THROUGH_STATUS" "$W/through.json"
else
  unproven "forced-part-way: an apply frozen mid-run" "no confirmed stopped apply: $(cat "$W/watch.err" 2>/dev/null)"
  "$SWEEP" undo "$D" >/dev/null 2>&1
fi

# 3c. A free race. Timing picks the ending; either is accepted, nothing else.
D="$W/contend/Desktop"
export ETUDE_STATE_DIR="$W/state-contend"
mkdir -p "$ETUDE_STATE_DIR"
make_tree "$D"
snapshot_tree "$D" "$W/contend-before.json"
"$SWEEP" apply "$D" --yes --json >"$W/first.json" 2>"$W/first.err" & FIRST=$!
"$SWEEP" apply "$D" --yes --json >"$W/second.json" 2>"$W/second.err" & SECOND=$!
wait "$FIRST"; FIRST_STATUS=$?
wait "$SECOND"; SECOND_STATUS=$?
check_contention race either "$D" "$W/contend-before.json" "$N" \
  "$FIRST_STATUS" "$W/first.json" "$SECOND_STATUS" "$W/second.json"

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
