#!/usr/bin/env bash
# A hard kill during undo must leave a journal that resumes to the exact
# original tree. This proves real setup, partial progress, termination, and
# recovery; a no-op binary cannot satisfy that chain.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir); trap 'rm -rf "$W"' EXIT
export ETUDE_JOURNAL_KEY="$(python3 -c 'import secrets; print(secrets.token_hex(32))')"

make_tree() {
  local d="$1" n="$2" i
  mkdir -p "$d"
  for i in $(seq 1 "$n"); do
    printf 'original screenshot payload %s\n' "$i" > "$d/Screenshot 2026-0$((i % 9 + 1))-$(printf %02d $((i % 28 + 1))) at $(printf %02d $((i % 12 + 1))).$(printf %02d $((i % 60))).$(printf %02d $((i % 60))) AM ($i).png"
  done
}

# The journal durably records each restored entry. The dev-only watcher reads
# that record stream and SIGSTOPs undo on the first partial observation at or
# beyond the target, so a fast runner cannot finish between observation and
# the test's SIGKILL.
wait_for_partial() {
  local target="$1" pid="$2" journal dump
  dump="$(dirname "$SWEEP")/journal-dump"
  [ -x "$dump" ] || return 1
  journal=$(ls -t "${ETUDE_STATE_DIR:-}"/sweep-*.journal 2>/dev/null | head -1)
  [ -n "$journal" ] || return 1
  "$dump" --watch-undo "$journal" "$target" "$pid"
}

TOTAL=500
TRIALS=0
for percent in 5 50 85; do
  TRIALS=$((TRIALS + 1))
  D="$W/trial-$percent"
  make_tree "$D" "$TOTAL"
  BEFORE="$W/before-$percent.json"
  AFTER="$W/after-$percent.json"
  snapshot_tree "$D" "$BEFORE"

  assert_exit 0 "undo interruption $percent%: setup apply succeeds" -- "$SWEEP" apply "$D" --yes
  moved=$(find "$D/Screenshots" -type f 2>/dev/null | wc -l | tr -d ' ')
  assert_eq "$TOTAL" "$moved" "undo interruption $percent%: apply moved all files into its holding directory"

  target=$((TOTAL * percent / 100))
  "$SWEEP" undo >/dev/null 2>&1 &
  pid=$!
  if ! witness=$(wait_for_partial "$target" "$pid"); then
    wait "$pid" 2>/dev/null || true
    fail "undo interruption $percent%: undo did not reach a witnessed partial state before exiting or the 10s deadline"
    continue
  fi
  read -r restored journal_total <<< "$witness"
  pass "undo interruption $percent%: journal durably records $restored of $TOTAL restored files while undo is stopped"

  if kill -9 "$pid" 2>/dev/null; then
    pass "undo interruption $percent%: SIGKILL was delivered to the journal-witnessed undo process"
  else
    fail "undo interruption $percent%: could not deliver SIGKILL after partial progress"
  fi
  wait "$pid" 2>/dev/null
  kill_status=$?
  [ "$kill_status" -ne 0 ] \
    && pass "undo interruption $percent%: killed undo exited non-zero ($kill_status)" \
    || fail "undo interruption $percent%: killed undo exited zero"

  restored_after_kill=$(find "$D" -maxdepth 1 -type f | wc -l | tr -d ' ')
  still_held=$(find "$D/Screenshots" -type f 2>/dev/null | wc -l | tr -d ' ')
  total_after_kill=$(find "$D" -type f | wc -l | tr -d ' ')
  if [ "$restored_after_kill" -gt 0 ] && [ "$still_held" -gt 0 ] && [ "$total_after_kill" -eq "$TOTAL" ]; then
    pass "undo interruption $percent%: interrupted tree has a real restored/held split with no missing files"
  else
    fail "undo interruption $percent%: interrupted tree is not a valid partial split (restored=$restored_after_kill held=$still_held total=$total_after_kill expected=$TOTAL)"
  fi

  assert_exit 0 "undo interruption $percent%: a second undo resumes successfully" -- "$SWEEP" undo
  snapshot_tree "$D" "$AFTER"
  assert_snapshot_eq "$BEFORE" "$AFTER" "undo interruption $percent%: resume restored every original path and byte"

  COMPLETE="$W/complete-$percent.json"
  TERMINAL="$W/terminal-$percent.json"
  snapshot_tree "$D" "$COMPLETE"
  final_out=$("$SWEEP" undo 2>&1); final_status=$?
  snapshot_tree "$D" "$TERMINAL"
  if [ "$final_status" -eq 1 ]; then
    pass "undo interruption $percent%: a completed journal returns the documented no-work exit"
  else
    fail "undo interruption $percent%: completed journal returned unexpected exit=$final_status output=${final_out%%$'\n'*}"
  fi
  assert_snapshot_eq "$COMPLETE" "$TERMINAL" "undo interruption $percent%: terminal no-work call changed no path or byte"
done

pass "SIGKILL undo recovery covered $TRIALS witnessed progress points (early, middle, and late)"
