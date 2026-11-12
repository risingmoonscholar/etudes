#!/usr/bin/env bash
# Two applies of one folder, where the loser saved its journal before the
# winner moved the files. The dev-only watcher stops the first apply after its
# journal is saved and before its first move, and confirms the stop. The second
# apply then runs to the end and moves every file. The first apply resumes and
# finds its files gone.
#
# What must hold:
#   - exactly one run moves files, and every file is moved once
#   - the loser exits non-zero, says nothing was moved and that there is
#     nothing to undo, and its --json says refused with no items moved
#   - `sweep status` lists no record as damaged or interrupted: the loser's
#     record holds no move, so it must read refused, not as moves without
#     their records
#   - one `sweep undo` puts every file back, and a second has nothing to undo
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
  exit 0
fi
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
[ "$LOSER" -ne 0 ] \
  && pass "the first apply does not claim success (exit $LOSER)" \
  || fail "the first apply exited 0 after its files were taken"
grep -qx "Nothing was moved, so there is nothing to undo." "$W/loser.err" \
  && pass "the first apply says nothing was moved and there is nothing to undo" \
  || fail "the first apply did not say it moved nothing: $(cat "$W/loser.err")"
read -r STATUS MOVED <<<"$(python3 -I - "$W/loser.json" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
moved = [c["count"] for c in d["effects"]["counters"] if c["name"] == "items_moved"]
print(d["status"], moved[0] if len(moved) == 1 else moved)
PY
)"
assert_eq "refused moved=0" "$STATUS moved=$MOVED" "the first apply's --json says refused, no items moved"
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
