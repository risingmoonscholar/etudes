#!/usr/bin/env bash
# A crash during stash or pop must preserve every payload and let the next
# invocation resume to the exact original tree.  Progress is witnessed from
# the root's visible files, never guessed from elapsed time.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir); trap 'rm -rf "$W"' EXIT
D="$W/Desktop"; TOTAL=1000
make_tree() {
  rm -rf "$D"; mkdir -p "$D"
  for i in $(seq 1 "$TOTAL"); do printf 'stash payload %s\n' "$i" > "$D/report_$i.pdf"; done
}
top_count() { find "$D" -maxdepth 1 -type f | wc -l | tr -d ' '; }
wait_for_between() {
  local lower="$1" upper="$2" pid="$3" n
  for _ in $(seq 1 1000); do
    n=$(top_count)
    if [ "$n" -gt "$lower" ] && [ "$n" -lt "$upper" ] && kill -0 "$pid" 2>/dev/null; then
      printf '%s\n' "$n"; return 0
    fi
    kill -0 "$pid" 2>/dev/null || return 1
    sleep 0.01
  done
  return 1
}
assert_partial_integrity() {
  local label="$1"
  assert_intact "$D" "$TOTAL" "$label leaves every payload at its root or holding location"
}

make_tree
BEFORE="$W/before.json"; snapshot_tree "$D" "$BEFORE"

# Kill a real stash after it has moved some but not all top-level files.
"$STASH" "$D" >"$W/stash.out" 2>&1 & PID=$!
if moved=$(wait_for_between 0 "$TOTAL" "$PID"); then
  pass "stash interruption witnessed $((TOTAL - moved)) of $TOTAL files moved while the process remained live"
  kill -9 "$PID" 2>/dev/null && pass "SIGKILL reached the active stash process" || fail "could not deliver SIGKILL to the active stash process"
  wait "$PID" 2>/dev/null; STATUS=$?
  [ "$STATUS" -ne 0 ] && pass "killed stash exited non-zero ($STATUS)" || fail "killed stash exited zero"
else
  wait "$PID" 2>/dev/null || true
  fail "stash did not reach a witnessed partial state within 10 seconds"
fi
assert_partial_integrity "interrupted stash"
assert_exit 0 "stash pop resumes the interrupted stash" -- "$STASH" pop "$D"
AFTER_STASH_RESUME="$W/after-stash-resume.json"; snapshot_tree "$D" "$AFTER_STASH_RESUME"
assert_snapshot_eq "$BEFORE" "$AFTER_STASH_RESUME" "stash recovery restored every original path and byte"

# Start from a completed stash, then kill pop after it has restored a strict
# subset. A second pop must account for the already-restored files and finish.
assert_exit 0 "setup stash before interrupted pop succeeds" -- "$STASH" "$D"
"$STASH" pop "$D" >"$W/pop.out" 2>&1 & PID=$!
if restored=$(wait_for_between 0 "$TOTAL" "$PID"); then
  pass "pop interruption witnessed $restored of $TOTAL files restored while the process remained live"
  kill -9 "$PID" 2>/dev/null && pass "SIGKILL reached the active pop process" || fail "could not deliver SIGKILL to the active pop process"
  wait "$PID" 2>/dev/null; STATUS=$?
  [ "$STATUS" -ne 0 ] && pass "killed pop exited non-zero ($STATUS)" || fail "killed pop exited zero"
else
  wait "$PID" 2>/dev/null || true
  fail "pop did not reach a witnessed partial state within 10 seconds"
fi
assert_partial_integrity "interrupted pop"
assert_exit 0 "stash pop resumes the interrupted pop" -- "$STASH" pop "$D"
AFTER_POP_RESUME="$W/after-pop-resume.json"; snapshot_tree "$D" "$AFTER_POP_RESUME"
assert_snapshot_eq "$BEFORE" "$AFTER_POP_RESUME" "pop recovery restored every original path and byte"
TERMINAL="$W/terminal.json"; snapshot_tree "$D" "$TERMINAL"
assert_exit 1 "completed stash pop reports no remaining work" -- "$STASH" pop "$D"
AFTER_TERMINAL="$W/after-terminal.json"; snapshot_tree "$D" "$AFTER_TERMINAL"
assert_snapshot_eq "$TERMINAL" "$AFTER_TERMINAL" "terminal stash pop changes no path or payload"
