#!/usr/bin/env bash
# The normal same-volume macOS move is atomic renamex_np(RENAME_EXCL).
# Revoke source-directory write permission while apply is confirmed stopped
# with a pending journal target. The create control must fail before resume.
# No timing calibration establishes injection. The link/unlink fallback is
# not exercised by this fixture and remains explicitly unproven.
#
# What must hold: sweep reports the failure honestly (nonzero exit), and the
# journal reflects reality: every entry it claims is "done" actually
# finished, and undo can act on exactly those.
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

  chmod 555 "$D"
  # Mode bits alone do not prove the current user cannot write (e.g. root).
  CREATE_RESULT=$(python3 - "$D" <<'PY_CREATE'
import os, sys
try:
    fd = os.open(os.path.join(sys.argv[1], 'write-control'), os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
except PermissionError:
    print('denied')
else:
    os.close(fd)
    os.unlink(os.path.join(sys.argv[1], 'write-control'))
    print('allowed')
PY_CREATE
  )
  if [ "$(stat -f '%Lp' "$D" 2>/dev/null)" != 555 ] || [ "$CREATE_RESULT" != denied ]; then
    chmod 755 "$D"
    kill -CONT "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true
    PID=""
    continue
  fi
  INJECTED=1
  pass "permission change mid-apply: pending target present, create failed while apply was confirmed stopped ($JOURNAL_MOVED/$N journalled moves)"
  if [ "$CONTROL" = --control-ignored ]; then
    # Mask the proven fault while still stopped to simulate an apply that
    # ignores it and returns success. The full witness must reject that.
    chmod 755 "$D"
  fi
  if ! kill -CONT "$PID"; then
    fail "could not resume apply after a proven permission injection"
    exit 0
  fi
  wait "$PID"; CODE=$?
  PID=""
  chmod 755 "$D"
  break
done
if [ "$INJECTED" != 1 ]; then
  unproven "permission change mid-apply injection" "no confirmed stopped apply with a pending target and failed create across four trials: $(cat "$W/watch.err")"
  exit 0
fi
assert_eq 3 "$CODE" "apply reported the permission failure rather than silently succeeding (exit $CODE)"
if [ -s "$W/apply.err" ] && grep -qi "sweep:" "$W/apply.err"; then
  pass "the failure was printed, not swallowed"
else
  fail "apply exited $CODE but printed nothing identifiable on stderr: $(cat "$W/apply.err")"
fi

MOVED=$(find "$D" -mindepth 2 -type f | wc -l | tr -d ' ')
UNDO_DONE=0
# The signature we're looking for: a name that exists BOTH at its original
# source path and at its destination. link(2) succeeded, unlink(2) failed,
# and nothing rolled the link back. If this exists, no file's content was
# lost (same inode, two names) but the journal's own "done" bookkeeping and
# the tree's real state have diverged.
DUP_NAMES=()
while IFS= read -r destpath; do
  name="$(basename "$destpath")"
  srcpath="$D/$name"
  if [ -f "$srcpath" ]; then
    DUP_NAMES+=("$name")
  fi
done < <(find "$D" -mindepth 2 -type f 2>/dev/null)

if [ "${#DUP_NAMES[@]}" -eq 0 ]; then
  pass "the witnessed permission failure left no source/destination duplicate"
else
  NAME="${DUP_NAMES[0]}"
  SRC_INODE=$(stat -f '%i' "$D/$NAME")
  DEST_PATH=$(find "$D" -mindepth 2 -name "$NAME" 2>/dev/null | head -1)
  DEST_INODE=$(stat -f '%i' "$DEST_PATH")

  echo "    (found: $NAME exists at both $D/$NAME and $DEST_PATH)"

  assert_eq "$SRC_INODE" "$DEST_INODE" "the duplicate is the SAME inode (link succeeded, no content was copied or lost). Just two names now"

  fail "REAL DEFECT: a permission failure between link(2) and unlink(2) inside move_one leaves a file with two live names (the original at \$D/$NAME and a hard-linked copy at $DEST_PATH), and apply.rs never marks that journal entry done (the \`done = true\` assignment sits after move_one returns Ok, so a mid-move_one failure skips it). \`sweep undo\` will therefore never offer to clean up the duplicate: it only reverses entries the journal marked done, and this one is not. The duplicate is permanent and untracked until removed by hand."

  # Confirm the prediction directly: undo must not touch this duplicate.
  UNDO_OUT=$("$SWEEP" undo 2>&1); UNDO_CODE=$?
  UNDO_DONE=1
  if [ -f "$D/$NAME" ] && [ -f "$DEST_PATH" ]; then
    fail "confirmed: after 'sweep undo', the duplicate at $DEST_PATH is still there and still untracked. undo output: $UNDO_OUT"
  else
    pass "undo actually cleaned up the duplicate after all (better than expected from reading the source)"
  fi
fi

# Whatever else happened, no file should be missing entirely.
MISSING=0
while IFS= read -r name; do
  if [ ! -e "$D/$name" ] && ! find "$D" -mindepth 2 -name "$name" 2>/dev/null | grep -q .; then
    MISSING=$((MISSING + 1))
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
assert_eq 0 "$MISSING" "no file went missing outright after the permission failure"

if [ "$UNDO_DONE" = 0 ]; then
  UNDO_OUT=$("$SWEEP" undo 2>&1); UNDO_CODE=$?
fi
assert_eq 0 "$UNDO_CODE" "undo succeeds after the permission failure"
RESTORED=$(echo "$UNDO_OUT" | grep -o 'Restored [0-9]*' | grep -o '[0-9]*')
assert_eq "$MOVED" "${RESTORED:-BAD}" "journal restores exactly the files actually moved before permission failure"
if echo "$UNDO_OUT" | grep -qi "already gone"; then
  fail "permission failure left phantom journal entries: $UNDO_OUT"
else
  pass "permission failure left no phantom journal entries"
fi
snapshot_tree "$D" "$W/recovered.json"
assert_snapshot_eq "$W/original.json" "$W/recovered.json" "permission failure undo restored exact original paths and bytes"
unproven "link/unlink fallback seam" "normal same-volume macOS rename is atomic; this fixture does not exercise fallback"
[ -n "$CONTROL" ] || run_controls
true
