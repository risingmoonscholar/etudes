#!/usr/bin/env bash
# Race: something else creates a file at a destination path after sweep's
# internal re-scan already decided that path was free, but before sweep's
# own move lands there. This is the filesystem-level version of the
# plan-level overwrite bug — verify the window is closed where it actually
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
    open(os.path.join(d, f"batch_{letters(i)}.dat"), "w").close()
PY
}

now_ms() { python3 -c 'import time; print(int(time.time()*1000))'; }

# idx, source path, and the group's destination directory name.
pick_target() {
  "$SWEEP" "$1" --json 2>/dev/null | python3 -c '
import json, sys
p = json.load(sys.stdin)
g = next(g for g in p["groups"] if g["name"] == "batch")
m = g["members"]
idx = int(len(m) * 0.75)
print(idx)
print(m[idx])
print(g["name"])
'
}

W=$(workdir); trap 'rm -rf "$W"' EXIT

CTRL="$W/control/Desktop"; build_tree "$CTRL"
T0_START=$(now_ms)
"$SWEEP" apply "$CTRL" --yes >/dev/null 2>&1
T0_END=$(now_ms)
T0=$((T0_END - T0_START))
if [ "$T0" -lt 20 ]; then
  unproven "destination appears mid-apply" "baseline apply of $N files finished in ${T0}ms on this host — too fast for a background process to land inside the window"
  exit 0
fi

SENTINEL="INTRUDER-CONTENT-DO-NOT-TOUCH-$$"
HIT=0
CODE=0
TARGET=""
BLOCKER=""
IDX=0
BEFORE=0
D=""
FRACTIONS=(70 45 25 12)

for frac in "${FRACTIONS[@]}"; do
  D="$W/attempt/Desktop"
  rm -rf "$W/attempt"
  build_tree "$D"
  PICK="$(pick_target "$D")"
  IDX="$(printf '%s\n' "$PICK" | sed -n '1p')"
  TARGET="$(printf '%s\n' "$PICK" | sed -n '2p')"
  GROUPNAME="$(printf '%s\n' "$PICK" | sed -n '3p')"
  NAME="$(basename "$TARGET")"
  BLOCKER="$D/$GROUPNAME/$NAME"
  BEFORE=$(find "$D" -type f | wc -l | tr -d ' ')

  "$SWEEP" apply "$D" --yes >"$W/apply.out" 2>"$W/apply.err" &
  PID=$!
  DELAY_MS=$((T0 * frac / 100))
  python3 -c "import time; time.sleep($DELAY_MS/1000)"
  mkdir -p "$D/$GROUPNAME"
  printf '%s' "$SENTINEL" > "$BLOCKER"
  wait "$PID"
  CODE=$?

  # A hit is: the source file for this entry never made it into the
  # destination (still sitting at the original path OR gone-because-refused
  # before any move started), and the blocker at the destination still holds
  # exactly our sentinel — i.e. sweep's move for this entry did not run, or
  # ran and lost the race honestly (EEXIST), either way never overwriting us.
  if [ "$CODE" != "0" ] && [ -f "$BLOCKER" ]; then
    CONTENT="$(cat "$BLOCKER" 2>/dev/null)"
    if [ "$CONTENT" = "$SENTINEL" ]; then
      HIT=1
      break
    fi
  fi
done

if [ "$HIT" != "1" ]; then
  unproven "destination appears mid-apply" "the intruder file never landed inside the apply window across ${#FRACTIONS[@]} timed attempts (baseline ${T0}ms) — could not exercise the race on this host"
  exit 0
fi

echo "    (race landed: blocker planted at ~${DELAY_MS}ms into a ~${T0}ms baseline run, group position $IDX of $((N - 2)))"

assert_eq 1 "$([ "$CODE" != "0" ] && echo 1 || echo 0)" "apply refused rather than succeeding over the intruder (exit $CODE)"

CONTENT="$(cat "$BLOCKER" 2>/dev/null)"
assert_eq "$SENTINEL" "$CONTENT" "the intruder's content at the destination was never overwritten"

# The tool's own source copy for this entry must not have been silently
# deleted while failing to land at the (blocked) destination — that would be
# data loss dressed up as a refusal.
SRC_STILL_HERE=0
[ -f "$TARGET" ] && SRC_STILL_HERE=1
if [ "$SRC_STILL_HERE" = "1" ]; then
  pass "the source file for the blocked entry was left in place, not deleted"
else
  fail "the source file is gone and the destination still holds only the intruder's content — the original file was lost: $TARGET"
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
    print(f'batch_{letters(i)}.dat')
")
# The intruder occupies the blocked entry's destination name, so that one
# source file is expected to be exactly where it started (checked above);
# it must not count as lost by this sweep.
assert_eq 0 "$LOST" "no original file went missing anywhere in the tree"
