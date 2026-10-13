#!/usr/bin/env bash
# Race: something else creates a file at a destination path after sweep's
# internal re-scan already decided that path was free, but before sweep's
# own move lands there. This is the filesystem-level version of the
# plan-level overwrite bug. Verify the window is closed where it actually
# matters: at the moment of the move, not just at plan time.
#
# What must hold: sweep must never clobber a file that beat it to the
# destination. Either it refuses before touching that path, or the attempt
# fails cleanly (EEXIST) and the intruder's content survives untouched.
#
# Nothing here is real data. Every tree is generated and removed on exit.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

require python3 "builds fixture trees fast and reads the --json plan" || exit 0

N=500

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

# idx, source path, and the group's destination directory name.
pick_target() {
  "$SWEEP" "$1" --json 2>/dev/null | python3 -c '
import json, sys
p = json.load(sys.stdin)["details"]
g = next(g for g in p["groups"] if g["name"] == "Data")
m = g["members"]
idx = int(len(m) * 0.75)
print(idx)
print(m[idx])
print(g["name"])
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
  if [ -n "${WRITER_PID:-}" ]; then
    touch "${RELEASE:-$W/release-writer}"
    wait "$WRITER_PID" 2>/dev/null || true
  fi
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
# Late apply must report an unproven injection; the unsafe-outcome control
# must make the unchanged assertions reject its simulated faulty result.
run_controls() {
  local control result output
  for control in late ignored; do
    output="$W/control-$control.out"
    env -u STRESS_WRAP_DEPTH -u STRESS_WRAPPER_PID -u STRESS_RESULT_FD \
      -u STRESS_FAILURE_ARTIFACTS /bin/bash "${BASH_SOURCE[0]}" "--control-$control" \
      >"$output" 2>&1 198<&- 199>&-
    result=$?
    if [ "$control" = late ]; then
      if [ "$result" -eq 2 ] && ! grep -q "FAIL" "$output" && ! grep -q "injection proven" "$output" && grep -q 'unproven.*injection' "$output"; then
        pass "late injection control is unproven, never a pass or a product failure"
      else
        fail "late injection control wanted unproven exit 2, got $result: $(cat "$output")"
      fi
    elif [ "$result" -eq 1 ] && grep -q 'confirmed stopped' "$output" && grep -q 'FAIL' "$output"; then
      pass "paused unsafe-outcome control fails retained assertions after a proven injection"
    else
      fail "paused unsafe-outcome control did not reject the fault (exit $result): $(cat "$output")"
    fi
  done
}

SENTINEL="INTRUDER-CONTENT-DO-NOT-TOUCH-$$"
INJECTED=0
CODE=0
TARGET=""
BLOCKER=""
IDX=0
BEFORE=0
D=""

for trial in 1 2 3 4; do
  D="$W/attempt-$trial/Desktop"
  export ETUDE_STATE_DIR="$W/state-$trial"
  mkdir -p "$ETUDE_STATE_DIR"
  build_tree "$D"
  PICK="$(pick_target "$D")"
  IDX="$(printf '%s\n' "$PICK" | sed -n '1p')"
  TARGET="$(printf '%s\n' "$PICK" | sed -n '2p')"
  GROUPNAME="$(printf '%s\n' "$PICK" | sed -n '3p')"
  NAME="$(basename "$TARGET")"
  BLOCKER="$D/$GROUPNAME/$NAME"
  DEST="$BLOCKER"
  BEFORE=$(find "$D" -type f | wc -l | tr -d ' ')

  "$SWEEP" apply "$D" --yes >"$W/apply.out" 2>"$W/apply.err" &
  PID=$!
  if [ "$CONTROL" = --control-late ]; then wait "$PID" 2>/dev/null || true; fi
  if ! WITNESS=$("$DUMP" --watch-apply "$ETUDE_STATE_DIR" "$D" "$TARGET" "$PID" 2>"$W/watch.err"); then
    kill -CONT "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true
    PID=""
    [ "$CONTROL" != --control-late ] || break
    continue
  fi
  read -r JOURNAL_MOVED JOURNAL_TOTAL PENDING_INDEX <<< "$WITNESS"
  STOPPED=$(ps -o state= -p "$PID" 2>/dev/null)
  if [[ "$STOPPED" != *T* ]] || [ ! -f "$TARGET" ] || [ -e "$DEST" ] || [ -L "$DEST" ]; then
    kill -CONT "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true
    PID=""
    continue
  fi
  mkdir -p "$D/$GROUPNAME"
  # noclobber makes injection itself safe. Plain `>` could overwrite sweep's
  # output after it won the race, then manufacture the very corruption this
  # scenario is supposed to detect.
  if (set -C; printf '%s' "$SENTINEL" > "$BLOCKER") 2>/dev/null; then
    INJECTED=1
  else
    kill -CONT "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true
    PID=""
    continue
  fi
  pass "injection proven while apply was confirmed stopped ($JOURNAL_MOVED/$JOURNAL_TOTAL journalled moves; pending index $PENDING_INDEX)"
  if [ "$CONTROL" = --control-ignored ]; then rm "$BLOCKER"; fi
  kill -CONT "$PID" || { fail "could not resume proven injection"; exit 0; }
  wait "$PID"
  CODE=$?
  PID=""
  break
done

if [ "$INJECTED" != "1" ]; then
  unproven "mid-apply injection" "no confirmed stopped apply with pending target across four trials: $(cat "$W/watch.err")"
  exit 0
fi

pass "destination appears mid-apply: exclusively created the intruder while confirmed stopped, before the target destination existed (group position $IDX)"

# From this point onward, the injection is the witness. Product outcomes are
# assertions, never a condition for deciding whether the race happened.
assert_eq 1 "$([ "$CODE" != "0" ] && echo 1 || echo 0)" "apply refused rather than silently succeeding after the intruder appeared (exit $CODE)"

CONTENT="$(cat "$BLOCKER" 2>/dev/null)"
assert_eq "$SENTINEL" "$CONTENT" "the intruder's content at the destination was never overwritten"

# The tool's own source copy for this entry must not have been silently
# deleted while failing to land at the (blocked) destination. That would be
# data loss dressed up as a refusal.
if [ -f "$TARGET" ]; then
  pass "the source file for the blocked entry was left in place, not deleted"
else
  fail "the source file is gone and the destination still holds only the intruder's content. The original file was lost: $TARGET"
fi

if [ -s "$W/apply.err" ] && grep -qi "sweep:" "$W/apply.err"; then
  pass "the refusal was printed, not swallowed"
else
  fail "apply exited $CODE but printed nothing identifiable on stderr: $(cat "$W/apply.err")"
fi

# No file anywhere should have vanished. The intruder file we planted is new
# content, not a file we're tracking, so we exclude it from the before/after
# count by checking directly instead of a blind count comparison.
LOST=0
while IFS= read -r name; do
  p="$D/$name"
  if [ ! -e "$p" ] && ! find "$D" -mindepth 2 -name "$name" 2>/dev/null | grep -q .; then
    LOST=$((LOST + 1))
  fi
done < <(python3 -c "
def letters(i, width=5):
    s = ''
    for _ in range(width):
        s = chr(97 + i % 26) + s
        i //= 26
    return s
for i in range($N):
    print(f'batch_{letters(i)}.csv')
")
# The intruder occupies the blocked entry's destination name, so that one
# source file is expected to be exactly where it started (checked above);
# it must not count as lost by this sweep.
assert_eq 0 "$LOST" "no original file went missing anywhere in the tree"

[ -n "$CONTROL" ] || run_controls
true
