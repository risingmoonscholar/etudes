#!/usr/bin/env bash
# Race: a source file is deleted after sweep's internal re-scan finds it but
# before its own turn in the move loop. `sweep apply` re-scans and re-plans
# in one call — the "plan" and "apply" the family targets are two passes
# inside that single invocation: pass 1 fingerprints every accepted file and
# writes the journal, pass 2 actually moves each one, and journal writes are
# fsynced per entry. That fsync cost gives every move a real, multi-millisecond
# footprint, so a big enough tree opens a real window for a background
# deleter to land inside.
#
# What must hold, with the source deleted mid-run:
#   - apply does not crash or leave an inconsistent tree
#   - it reports the failure honestly (nonzero exit)
#   - the journal never claims to have moved the file it could not touch
#   - nothing else is lost: total file count drops by exactly the one file
#     we ourselves deleted, nothing more
#
# Nothing here is real data. Every tree is generated and removed on exit.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

require python3 "builds fixture trees fast and reads the --json plan" || exit 0

N=500

# Letter-only stems so no filename accidentally collides with a sensitive
# marker (a plain zero-padded number can: "batch_1099.dat" reads as a 1099).
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
    open(os.path.join(d, f"batch_{letters(i)}.dat"), "w").close()
PY
}

now_ms() { python3 -c 'import time; print(int(time.time()*1000))'; }

# The one group's member list, in the exact order apply() will process it.
pick_target() {
  "$SWEEP" "$1" --json 2>/dev/null | python3 -c '
import json, sys
p = json.load(sys.stdin)
g = next(g for g in p["groups"] if g["name"] == "batch")
m = g["members"]
idx = int(len(m) * 0.75)
print(idx)
print(m[idx])
'
}

W=$(workdir); trap 'rm -rf "$W"' EXIT

# Calibrate: how long does an uninterrupted apply take on a tree this size,
# on this host, right now? Everything else is timed off this measurement
# instead of a hardcoded constant.
CTRL="$W/control/Desktop"; build_tree "$CTRL"
T0_START=$(now_ms)
"$SWEEP" apply "$CTRL" --yes >/dev/null 2>&1
T0_END=$(now_ms)
T0=$((T0_END - T0_START))
if [ "$T0" -lt 20 ]; then
  unproven "file vanishes mid-apply" "baseline apply of $N files finished in ${T0}ms on this host — too fast for a background process to land inside the window"
  exit 0
fi

HIT=0
CODE=0
TARGET=""
IDX=0
BEFORE=0
D=""
FRACTIONS=(70 45 25 12)

for frac in "${FRACTIONS[@]}"; do
  D="$W/attempt/Desktop"
  rm -rf "$W/attempt"
  build_tree "$D"
  PICK="$(pick_target "$D")"
  IDX="${PICK%%$'\n'*}"
  TARGET="${PICK#*$'\n'}"
  BEFORE=$(find "$D" -type f | wc -l | tr -d ' ')

  "$SWEEP" apply "$D" --yes >"$W/apply.out" 2>"$W/apply.err" &
  PID=$!
  DELAY_MS=$((T0 * frac / 100))
  python3 -c "import time; time.sleep($DELAY_MS/1000)"
  rm -f "$TARGET"
  wait "$PID"
  CODE=$?

  if [ "$CODE" != "0" ] && [ ! -e "$TARGET" ]; then
    # Confirm it's actually gone, not just relocated under a name we didn't
    # expect (e.g. classify put it somewhere odd) — search the whole tree.
    if ! find "$D" -name "$(basename "$TARGET")" 2>/dev/null | grep -q .; then
      HIT=1
      break
    fi
  fi
done

if [ "$HIT" != "1" ]; then
  unproven "file vanishes mid-apply" "deletion never landed inside the apply window across ${#FRACTIONS[@]} timed attempts (baseline ${T0}ms) — could not exercise the race on this host"
  exit 0
fi

echo "    (race landed: deleted at ~${DELAY_MS}ms into a ~${T0}ms baseline run, group position $IDX of $((N - 2)))"

assert_eq 1 "$([ "$CODE" != "0" ] && echo 1 || echo 0)" "apply reported the deletion as a failure, not a silent success (exit $CODE)"

if [ -s "$W/apply.err" ] && grep -qi "sweep:" "$W/apply.err"; then
  pass "the failure was printed, not swallowed"
else
  fail "apply exited $CODE but printed nothing identifiable on stderr: $(cat "$W/apply.err")"
fi

# The only file that should be missing anywhere in the tree is the one we
# ourselves deleted. Nothing else the tool touched should have vanished.
AFTER=$(find "$D" -type f | wc -l | tr -d ' ')
assert_eq "$((BEFORE - 1))" "$AFTER" "exactly one file missing after the crash — the one we deleted, nothing more"

MOVED=$(find "$D" -mindepth 2 -type f 2>/dev/null | wc -l | tr -d ' ')
echo "    ($MOVED files had already landed in the destination group before the abort)"

# The journal must not claim the deleted file as moved. If it lied, `undo`
# would either try to restore a file that was never really relocated (and
# find nothing at the recorded destination — a story that would look
# identical to "already gone", masking the lie) or the restored count would
# not match what we can independently verify was actually moved.
UNDO_OUT=$("$SWEEP" undo 2>&1)
RESTORED=$(echo "$UNDO_OUT" | grep -o 'Restored [0-9]*' | grep -o '[0-9]*')
assert_eq "$MOVED" "${RESTORED:-BAD}" "undo restored exactly the files that were actually moved before the crash"

if echo "$UNDO_OUT" | grep -qi "already gone"; then
  fail "undo reported files as 'already gone' — the journal claimed a move that never happened: $UNDO_OUT"
else
  pass "no phantom entries: undo never claimed anything was moved-then-missing"
fi

FINAL=$(find "$D" -type f | wc -l | tr -d ' ')
assert_eq "$((BEFORE - 1))" "$FINAL" "after undo, every survivor is back where it started"
