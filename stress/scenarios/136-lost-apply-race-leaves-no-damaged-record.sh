#!/usr/bin/env bash
set -u
# Two applies of one folder, where the loser saved its journal before the
# winner moved the files. The dev-only watcher stops the first apply after its
# journal is saved and before its first move, and confirms the stop. The second
# apply then runs to the end and moves every file. The first apply resumes and
# finds its files gone.
#
# What must hold:
#   - exactly one run moves files, and every file is moved once
#   - the loser is refused as a stale plan, exactly like a loser that finds a
#     destination taken: exit 2, plan_stale, retry after_change, the words
#     "changed just now, perhaps by another run. Nothing was moved.", and
#     --json refused with no items moved
#   - `sweep status` lists no record as damaged or interrupted: the loser's
#     record holds no move, so it must read refused, not as moves without
#     their records
#   - one `sweep undo` puts every file back, and a second has nothing to undo
#
# stash runs the same apply code, so the same race is run with two `stash
# FOLDER`. The loser is refused as a stale plan with nothing moved, `stash
# status --id` reads its record as refused with nothing planned, `stash
# status` lists only the winner's stash, and once that is popped, status and
# pop both say nothing is stashed.
#
# Nothing here is real data. Every tree is generated and removed on exit.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

require python3 "builds the tree and reads --json output" || exit 0

N=600
DUMP="$(dirname "$SWEEP")/journal-dump"
if [ ! -x "$DUMP" ]; then
  unproven "lost apply race" "dev-only journal watcher unavailable"
  exit 0
fi

W=$(workdir) || exit 1
W=$(cd "${W:?}" && pwd -P) && [ -n "$W" ] || exit 1
PID=""
cleanup() {
  if [ -n "$PID" ]; then
    kill -CONT "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true
  fi
  rm -rf "${W:?}"
}
trap cleanup EXIT

# The group folder is there before either apply, so neither makes it, and a
# stop anywhere before the first move leaves the same state to resume from.
build_tree() {
  mkdir -p "$1/Screenshots"
  python3 -I - "$1" "$N" <<'PY'
import os, sys
d, n = sys.argv[1], int(sys.argv[2])
for i in range(n):
    p = os.path.join(d, f"Screenshot 2026-01-01 at 10.00.00 AM ({i}).png")
    with open(p, "w") as f:
        f.write(f"payload {i}\n")
PY
}

# The stop lands in the window between the journal save and the first move.
# A stop that comes too late resumes the apply and says so; try a fresh folder
# and a fresh state folder then, so a missed window is never read as a result.
WITNESS=""
for attempt in 1 2 3 4 5; do
  D="$W/try-$attempt/Desktop"; STATE="$W/try-$attempt/state"
  mkdir -p "$STATE"
  build_tree "$D"
  snapshot_tree "$D" "$W/before.json"
  ETUDE_STATE_DIR="$STATE" "$SWEEP" apply "$D" --yes --json >"$W/loser.json" 2>"$W/loser.err" &
  PID=$!
  if WITNESS=$("$DUMP" --watch-apply-saved "$STATE" "$D" "$PID" 2>"$W/watch.err"); then
    break
  fi
  WITNESS=""
  wait "$PID" 2>/dev/null
  PID=""
done
if [ -z "$WITNESS" ]; then
  unproven "lost apply race" "the first apply was not stopped before its first move: $(cat "$W/watch.err")"
else
export ETUDE_STATE_DIR="$STATE"
[[ "$(ps -o state= -p "$PID")" == *T* ]] \
  && pass "the first apply is stopped after saving its journal and before any move ($WITNESS)" \
  || fail "the watcher did not leave the first apply stopped"

"$SWEEP" apply "$D" --yes >"$W/winner.out" 2>&1
WINNER=$?
[[ "$(ps -o state= -p "$PID")" == *T* ]] \
  && pass "the second apply finished while the first stayed stopped" \
  || fail "the overlap was lost: the first apply ran on"
kill -CONT "$PID"
wait "$PID"; LOSER=$?
PID=""

assert_eq 0 "$WINNER" "the second apply succeeds"
grep -qx "Moved $N files." "$W/winner.out" \
  && pass "the second apply moved every file" \
  || fail "the second apply did not say it moved $N files: $(cat "$W/winner.out")"
assert_eq 2 "$LOSER" "the first apply is refused, not reported as stopped part-way"
grep -qxF "sweep: $D changed just now, perhaps by another run. Nothing was moved. Scan and apply again." "$W/loser.err" \
  && pass "the first apply says the folder changed and nothing was moved" \
  || fail "the first apply did not give the stale-plan words: $(cat "$W/loser.err")"
read -r STATUS MOVED REASON <<<"$(python3 -I - "$W/loser.json" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
moved = [c["count"] for c in d["effects"]["counters"] if c["name"] == "items_moved"]
r = d["reason"]
print(d["status"], moved[0] if len(moved) == 1 else moved,
      f'{r["code"]}/{r["retry"]}/{r["subject"]["kind"]}')
PY
)"
assert_eq "refused moved=0 plan_stale/after_change/plan" "$STATUS moved=$MOVED $REASON" \
  "the first apply's --json says refused, plan_stale, no items moved"
assert_eq "$N" "$(find "$D" -mindepth 2 -type f | wc -l | tr -d ' ')" "every file is at its destination exactly once"
assert_eq 0 "$(find "$D" -maxdepth 1 -type f | wc -l | tr -d ' ')" "no file is left at the top of the folder"

"$SWEEP" status "$D" --json >"$W/status.json" 2>"$W/status.err"
assert_eq 0 "$?" "sweep status answers"
read -r BAD MOVERS TOTAL <<<"$(python3 -I - "$W/status.json" <<'PY'
import json, sys
records = json.load(open(sys.argv[1]))["details"]
bad = [r["state"] for r in records if r["damaged"] or r["state"] == "interrupted"]
movers = [r for r in records if r["counts"]["moved"] > 0]
print(len(bad), len(movers), sum(r["counts"]["moved"] for r in records))
PY
)"
assert_eq 0 "$BAD" "sweep status lists no record as damaged or interrupted"
assert_eq "1 $N" "$MOVERS $TOTAL" "sweep status shows one record holding every move"
STATUS_TEXT=$("$SWEEP" status "$D" 2>&1)
# The folder path holds this scenario's name, so match the state word and the
# damaged-record line, not any text.
if grep -qE '^[^ ]+ interrupted planned=|This undo record is damaged' <<<"$STATUS_TEXT"; then
  fail "sweep status reads a record as damaged or interrupted: $STATUS_TEXT"
else
  pass "sweep status text names no damaged or interrupted record"
fi

"$SWEEP" undo "$D" >"$W/undo.out" 2>&1
assert_eq 0 "$?" "one sweep undo succeeds"
grep -qx "Restored $N files." "$W/undo.out" \
  && pass "one undo restores every file" \
  || fail "the undo did not say it restored $N files: $(cat "$W/undo.out")"
snapshot_tree "$D" "$W/after-undo.json"
assert_snapshot_eq "$W/before.json" "$W/after-undo.json" "the folder equals its original after one undo"
OUT=$("$SWEEP" undo "$D" 2>&1); CODE=$?
assert_eq 1 "$CODE" "a second undo has nothing to do"
grep -qi 'nothing to undo' <<<"$OUT" \
  && pass "the second undo says there is nothing to undo" \
  || fail "the second undo did not say there is nothing to undo: $OUT"
fi

# --- stash: the same race ---------------------------------------------------
build_stash_tree() {
  mkdir -p "$1"
  python3 -I - "$1" "$N" <<'PY'
import os, sys
d, n = sys.argv[1], int(sys.argv[2])
for i in range(n):
    with open(os.path.join(d, f"note {i}.pdf"), "w") as f:
        f.write(f"payload {i}\n")
PY
}
WITNESS=""
for attempt in 1 2 3 4 5; do
  G="$W/stash-$attempt/Inbox"; SSTATE="$W/stash-$attempt/state"
  mkdir -p "$SSTATE"
  build_stash_tree "$G"
  snapshot_tree "$G" "$W/stash-before.json"
  ETUDE_STATE_DIR="$SSTATE" "$STASH" "$G" --json >"$W/stash-loser.json" 2>"$W/stash-loser.err" &
  PID=$!
  if WITNESS=$("$DUMP" --watch-apply-saved "$SSTATE" "$G" "$PID" stash 2>"$W/stash-watch.err"); then
    break
  fi
  WITNESS=""
  wait "$PID" 2>/dev/null
  PID=""
done
if [ -z "$WITNESS" ]; then
  unproven "lost stash race" "the first stash was not stopped before its first move: $(cat "$W/stash-watch.err")"
else
export ETUDE_STATE_DIR="$SSTATE"
[[ "$(ps -o state= -p "$PID")" == *T* ]] \
  && pass "stash: the first stash is stopped after saving its journal and before any move ($WITNESS)" \
  || fail "stash: the watcher did not leave the first stash stopped"
"$STASH" "$G" >"$W/stash-winner.out" 2>&1
assert_eq 0 "$?" "stash: the second stash succeeds"
[[ "$(ps -o state= -p "$PID")" == *T* ]] \
  && pass "stash: the second stash finished while the first stayed stopped" \
  || fail "stash: the overlap was lost: the first stash ran on"
kill -CONT "$PID"
wait "$PID"; LOSER=$?
PID=""
assert_eq 2 "$LOSER" "stash: the first stash is refused, not reported as stopped part-way"
grep -qF "Nothing was moved." "$W/stash-loser.err" \
  && pass "stash: the first stash says nothing was moved" \
  || fail "stash: the first stash did not say nothing was moved: $(cat "$W/stash-loser.err")"
read -r STATUS MOVED REASON LOSER_ID <<<"$(python3 -I - "$W/stash-loser.json" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
moved = [c["count"] for c in d["effects"]["counters"] if c["name"] == "items_moved"]
r = d["reason"]
print(d["status"], moved[0] if len(moved) == 1 else moved,
      f'{r["code"]}/{r["retry"]}', (d.get("recovery") or {}).get("operation") or "none")
PY
)"
assert_eq "refused moved=0 plan_stale/after_change" "$STATUS moved=$MOVED $REASON" \
  "stash: the first stash's --json says refused, plan_stale, no items moved"
"$STASH" status --id "$LOSER_ID" --json >"$W/stash-id.json" 2>"$W/stash-id.err"
assert_eq "refused planned=0 moved=0" \
  "$(python3 -I -c 'import json, sys; d = json.load(open(sys.argv[1]))["details"]; print(d["state"], "planned=%d" % d["planned"], "moved=%d" % d["moved"])' "$W/stash-id.json" 2>&1)" \
  "stash: status --id reads the first stash's record as refused with nothing planned"
WINNER_ID=$(find "$SSTATE" -maxdepth 1 -name 'stash-*.journal' ! -name "stash-$LOSER_ID.journal" | sed 's#.*/stash-##; s#\.journal$##')
STATUS_TEXT=$("$STASH" status "$G" 2>&1)
if grep -qF "$N items in 1 operation stashed" <<<"$STATUS_TEXT" && grep -qF "$WINNER_ID" <<<"$STATUS_TEXT" \
    && ! grep -qF "$LOSER_ID" <<<"$STATUS_TEXT"; then
  pass "stash: status lists only the second stash"
else
  fail "stash: status should list only the second stash ($WINNER_ID): $STATUS_TEXT"
fi
"$STASH" pop "$G" >"$W/stash-pop.out" 2>&1
assert_eq 0 "$?" "stash: one pop succeeds"
snapshot_tree "$G" "$W/stash-after.json"
assert_snapshot_eq "$W/stash-before.json" "$W/stash-after.json" "stash: the folder equals its original after one pop"
OUT=$("$STASH" status "$G" 2>&1); CODE=$?
assert_eq 1 "$CODE" "stash: status after the pop has nothing to report"
grep -qF "Nothing stashed in" <<<"$OUT" \
  && pass "stash: status says nothing is stashed" \
  || fail "stash: status after the pop said: $OUT"
OUT=$("$STASH" pop "$G" 2>&1); CODE=$?
assert_eq 1 "$CODE" "stash: a second pop has nothing to restore"
grep -qF "no stash from" <<<"$OUT" \
  && pass "stash: a second pop says there is no stash" \
  || fail "stash: a second pop said: $OUT"
fi
