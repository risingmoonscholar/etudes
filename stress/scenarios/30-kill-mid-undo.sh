#!/usr/bin/env bash
# A hard kill during undo must leave a journal that resumes to the exact
# original tree. This proves real setup, partial progress, termination, and
# recovery; a no-op binary cannot satisfy that chain.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

CONTROL="${1:-}"
case "$CONTROL" in ''|--control-late|--control-missing) ;; *) fail "unknown undo control"; exit 0;; esac
W=$(workdir)
pid=""
cleanup() {
  if [ -n "$pid" ]; then
    kill -CONT "$pid" 2>/dev/null || true
    kill -TERM "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  fi
  rm -rf "$W"
  eval "$_stress_state_cleanup"
}
trap cleanup EXIT
export ETUDE_JOURNAL_KEY="$(python3 -c 'import secrets; print(secrets.token_hex(32))')"

make_tree() {
  python3 - "$1" "$2" <<'PY_TREE'
import pathlib, sys
d, n = pathlib.Path(sys.argv[1]), int(sys.argv[2])
d.mkdir(parents=True)
for i in range(1, n + 1):
    # Preserve the original 500 names; added names avoid 1040/1099 markers.
    suffix = str(i) if i <= 500 else f"{i:x}"
    name = f"Screenshot 2026-0{i % 9 + 1}-{i % 28 + 1:02d} at {i % 12 + 1:02d}.{i % 60:02d}.{i % 60:02d} AM ({suffix}).png"
    (d / name).write_text(f"original screenshot payload {i}\n")
PY_TREE
}

# The journal durably records each restored entry. The dev-only watcher reads
# that record stream, confirms SIGSTOP, then reloads before reporting a real
# partial state. A stale observation is an unproven attempt, never a pass.
wait_for_partial() {
  local target="$1" pid="$2" journal dump
  dump="$(dirname "$SWEEP")/journal-dump"
  [ -x "$dump" ] || return 1
  journal=$(ls -t "${ETUDE_STATE_DIR:-}"/sweep-*.journal 2>/dev/null | head -1)
  [ -n "$journal" ] || return 1
  "$dump" --watch-undo "$journal" "$target" "$pid"
}

TRIALS=0
PERCENTAGES="5 50 85"
[ -z "$CONTROL" ] || PERCENTAGES="50"
for percent in $PERCENTAGES; do
  TOTAL=500
  # The late window has four times as many durable file restores remaining.
  [ "$percent" != 85 ] || TOTAL=2000
  PROVEN=0
  for attempt in 1 2 3 4; do
    D="$W/trial-$percent-attempt-$attempt"
    export ETUDE_STATE_DIR="$W/state-$percent-attempt-$attempt"
    mkdir -p "$ETUDE_STATE_DIR"
    make_tree "$D" "$TOTAL"
    BEFORE="$W/before-$percent.json"
    AFTER="$W/after-$percent.json"
    snapshot_tree "$D" "$BEFORE"

    setup_out=$("$SWEEP" apply "$D" --yes 2>&1); setup_status=$?
    if [ "$setup_status" -ne 0 ]; then
      fail "undo interruption $percent%: setup apply succeeds: wanted exit 0, got $setup_status ($setup_out)"
      break
    fi
    moved=$(find "$D/Screenshots" -type f 2>/dev/null | wc -l | tr -d ' ')
    if [ "$moved" != "$TOTAL" ]; then
      fail "undo interruption $percent%: apply did not move all files (wanted $TOTAL, got $moved)"
      break
    fi

    target=$((TOTAL * percent / 100))
    "$SWEEP" undo >"$W/undo.out" 2>"$W/undo.err" &
    pid=$!
    if [ "$CONTROL" = --control-late ]; then
      wait "$pid" 2>/dev/null || true
    fi
    if witness=$(wait_for_partial "$target" "$pid" 2>"$W/watch.err"); then
      cat "$W/watch.err" >&2
      stopped=$(ps -o state= -p "$pid" 2>/dev/null)
      if [[ "$stopped" != *T* ]]; then
        fail "undo interruption $percent%: watcher returned success without a confirmed stopped undo"
        kill -CONT "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
        pid=""
        break
      fi
      PROVEN=1
      TRIALS=$((TRIALS + 1))
      break
    fi
    cat "$W/watch.err" >&2
    kill -CONT "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null; undo_status=$?
    pid=""
    if [ "$undo_status" -ne 0 ]; then
      fail "undo interruption $percent%: unwitnessed undo exited $undo_status: $(cat "$W/undo.err")"
      break
    fi
    echo "    attempt  undo interruption $percent%: attempt $attempt unproven"
    [ "$CONTROL" != --control-late ] || break
  done
  if [ "$PROVEN" != 1 ]; then
    unproven "undo interruption $percent%" "no real stopped partial state across at most four attempts"
    continue
  fi
  pass "undo interruption $percent%: setup apply succeeds (exit $setup_status)"
  assert_eq "$TOTAL" "$moved" "undo interruption $percent%: apply moved all files into its holding directory"
  read -r restored journal_total <<< "$witness"
  assert_eq "$TOTAL" "$journal_total" "undo interruption $percent%: witness belongs to the complete fixture"
  pass "undo interruption $percent%: journal durably records $restored of $TOTAL restored files while undo is stopped"

  if [ "$CONTROL" = --control-missing ]; then
    victim=$(find "$D" -maxdepth 1 -type f | head -1)
    rm -- "$victim"
    pass "missing-file control: removed a restored file while undo was confirmed stopped"
  fi
  if kill -9 "$pid" 2>/dev/null; then
    pass "undo interruption $percent%: SIGKILL was delivered to the journal-witnessed undo process"
  else
    fail "undo interruption $percent%: could not deliver SIGKILL after partial progress"
  fi
  wait "$pid" 2>/dev/null
  kill_status=$?
  pid=""
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

if [ -z "$CONTROL" ]; then
  echo "    coverage SIGKILL undo recovery: $TRIALS of 3 progress points proven"
  for control in late missing; do
    output="$W/control-$control.out"
    env -u STRESS_WRAP_DEPTH -u STRESS_WRAPPER_PID -u STRESS_RESULT_FD \
      -u STRESS_FAILURE_ARTIFACTS /bin/bash "${BASH_SOURCE[0]}" "--control-$control" \
      >"$output" 2>&1 198<&- 199>&-
    result=$?
    if [ "$control" = late ]; then
      if [ "$result" -eq 2 ] && grep -q 'unproven.*undo interruption' "$output" && \
          ! grep -q 'journal durably records' "$output"; then
        pass "finished-undo control is unproven, never a witnessed pass or product failure"
      else
        fail "finished-undo control wanted unproven exit 2, got $result: $(cat "$output")"
      fi
    elif [ "$result" -eq 2 ] && grep -q 'unproven.*undo interruption' "$output"; then
      unproven "missing-file control" "no stopped partial undo for the deletion"
    elif [ "$result" -eq 1 ] && grep -q 'confirmed stopped' "$output" && \
        grep -q 'FAIL.*resume restored every original path and byte' "$output"; then
      pass "missing-file control fails byte-exact recovery after deletion while stopped"
    else
      fail "missing-file control did not reject recovery (exit $result): $(cat "$output")"
    fi
  done
fi
true
