#!/usr/bin/env bash
# Race: a source file is replaced by a symlink pointing outside the scanned
# root, after sweep's internal re-scan already recorded it as a plain file,
# but before its own turn to move.
#
# What must hold: sweep must not follow the link to its target. It must not
# read the external file's content. It must not write to it. It must not
# delete it. It must not write anything outside the root it was given.
#
# This scenario proves the property for the path every real sweep run takes:
# source and destination on the same device. macOS uses an exclusive atomic
# rename; filesystems without that operation use the hard-link fallback for
# regular files and rename for a symlink. None dereferences the source link.
# Cross-device copies use a separate path and are not assessed by this probe.
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

OUTSIDE="$W/outside"
mkdir -p "$OUTSIDE"
SENTINEL="SAME-DEVICE-OUTSIDE-SECRET-$$-$(python3 -c 'import random;print(random.randint(100000,999999))')"
printf '%s' "$SENTINEL" > "$OUTSIDE/secret.txt"
SOURCE_SENTINEL="SAME-DEVICE-INSIDE-SOURCE-$$-$(python3 -c 'import random;print(random.randint(100000,999999))')"

INJECTED=0
CODE=0
TARGET=""
GROUP=""
D=""

for trial in 1 2 3 4; do
  D="$W/attempt-$trial/Desktop"
  export ETUDE_STATE_DIR="$W/state-$trial"
  mkdir -p "$ETUDE_STATE_DIR"
  build_tree "$D"
  PICK="$(pick_target "$D")"
  IDX="$(printf '%s\n' "$PICK" | sed -n '1p')"
  TARGET="$(printf '%s\n' "$PICK" | sed -n '2p')"
  GROUP="$(printf '%s\n' "$PICK" | sed -n '3p')"
  NAME="$(basename "$TARGET")"
  DEST="$D/$GROUP/$NAME"
  # A regular destination with these bytes proves this member moved before
  # the replacement; outside bytes prove that the replacement was followed.
  printf '%s' "$SOURCE_SENTINEL" > "$TARGET"

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
  rm -f "$TARGET"
  if ln -s "$OUTSIDE/secret.txt" "$TARGET" && [ -L "$TARGET" ] && [ "$(readlink "$TARGET")" = "$OUTSIDE/secret.txt" ]; then
    INJECTED=1
  else
    kill -CONT "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true
    PID=""
    continue
  fi
  pass "injection proven while apply was confirmed stopped ($JOURNAL_MOVED/$JOURNAL_TOTAL journalled moves; pending index $PENDING_INDEX)"
  if [ "$CONTROL" = --control-ignored ]; then cat "$OUTSIDE/secret.txt" > "$D/simulated-unsafe-copy"; fi
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

pass "symlink swap mid-apply: replaced the planned source with a verified external link while confirmed stopped (group position $IDX)"

# The witnessed swap decides whether this test ran. A regular destination with
# the outside bytes is unsafe; a relocated link or a refusal that leaves the
# link at source are both defined safe outcomes for same-device movement.
if [ -L "$DEST" ]; then
  LINK_TARGET="$(readlink "$DEST" 2>/dev/null)"
  assert_eq "$OUTSIDE/secret.txt" "$LINK_TARGET" "the relocated link still points at the same outside path, unchanged"
elif [ -L "$TARGET" ] && [ "$CODE" -ne 0 ]; then
  pass "apply refused after the swap and left the external link at its source"
else
  fail "after a witnessed symlink swap, apply left neither a safe destination link nor a refused source link (exit=$CODE)"
fi

OUTSIDE_CONTENT="$(cat "$OUTSIDE/secret.txt" 2>/dev/null)"
assert_eq "$SENTINEL" "$OUTSIDE_CONTENT" "the outside file's content was never read into a copy or modified"

COPIES=$(find "$D" -type f -exec grep -lF "$SENTINEL" {} + 2>/dev/null || true)
if [ -n "$COPIES" ]; then
  fail "the outside file's content leaked into the tree as a real copy somewhere under \$D"
else
  pass "no copy of the outside content exists anywhere in the tree. Only the link moved"
fi

if [ -e "$OUTSIDE/secret.txt" ]; then
  pass "the outside file itself still exists. It was never deleted through the link"
else
  fail "the outside file was deleted. sweep followed the link to remove it"
fi

[ -n "$CONTROL" ] || run_controls
true
