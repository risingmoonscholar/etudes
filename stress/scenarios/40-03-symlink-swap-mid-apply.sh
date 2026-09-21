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
# source and destination on the same device, so move_one uses hard_link (with
# an explicit is_file() guard against exactly this) and, when that's not
# available, rename(2), neither of which dereferences a symlink source.
#
# A sibling condition exists. A *cross-device* move (source and destination
# on different filesystems, e.g. an external volume mounted inside the
# scanned folder) falls back to a different code path, `fs::copy`, which
# Rust's stdlib documents as following symlinks. That was reproduced by hand
# during authoring (a mounted ram disk as the source device, a source file
# swapped for a symlink to an outside secret, timed into the same window this
# scenario exercises): the destination ended up holding the outside file's
# exact bytes, and `apply` reported ordinary success. It is not automated
# here. Every attempt to mount and then cleanly detach a scratch volume on
# this host left it wedged ("Resource busy") for minutes at a time even after
# killing sweep and the mount's own helper process, which is not a
# side effect a shared test suite should risk leaving behind. See the
# session report for the exact repro commands.
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

now_ms() { python3 -c 'import time; print(int(time.time()*1000))'; }

pick_target() {
  "$SWEEP" "$1" --json 2>/dev/null | python3 -c '
import json, sys
p = json.load(sys.stdin)
g = next(g for g in p["groups"] if g["name"] == "Data")
m = g["members"]
idx = int(len(m) * 0.75)
print(idx)
print(m[idx])
print(g["name"])
'
}

W=$(workdir); trap 'rm -rf "$W"' EXIT

OUTSIDE="$W/outside"
mkdir -p "$OUTSIDE"
SENTINEL="SAME-DEVICE-OUTSIDE-SECRET-$$-$(python3 -c 'import random;print(random.randint(100000,999999))')"
printf '%s' "$SENTINEL" > "$OUTSIDE/secret.txt"

CTRL="$W/control/Desktop"; build_tree "$CTRL"
T0_START=$(now_ms)
"$SWEEP" apply "$CTRL" --yes >/dev/null 2>&1
T0_END=$(now_ms)
T0=$((T0_END - T0_START))
if [ "$T0" -lt 20 ]; then
  unproven "symlink swap mid-apply (same device)" "baseline apply of $N files finished in ${T0}ms on this host. Too fast for a background process to land inside the window"
  exit 0
fi

INJECTED=0
CODE=0
TARGET=""
GROUP=""
D=""
FRACTIONS=(70 45 25 12)

for frac in "${FRACTIONS[@]}"; do
  D="$W/attempt/Desktop"
  rm -rf "$W/attempt"
  build_tree "$D"
  PICK="$(pick_target "$D")"
  IDX="$(printf '%s\n' "$PICK" | sed -n '1p')"
  TARGET="$(printf '%s\n' "$PICK" | sed -n '2p')"
  GROUP="$(printf '%s\n' "$PICK" | sed -n '3p')"
  NAME="$(basename "$TARGET")"
  DEST="$D/$GROUP/$NAME"

  "$SWEEP" apply "$D" --yes >"$W/apply.out" 2>"$W/apply.err" &
  PID=$!
  DELAY_MS=$((T0 * frac / 100))
  python3 -c "import time; time.sleep($DELAY_MS/1000)"
  # A completed move leaves the old source path free again.  Replacing it at
  # that point would prove nothing about the mover, and a normal destination
  # file would be a safe (but misleading) result.  Only inject while this
  # member is still waiting to be moved.
  if [ -e "$DEST" ] || [ -L "$DEST" ]; then
    wait "$PID" 2>/dev/null || true
    continue
  fi
  rm -f "$TARGET"
  if ln -s "$OUTSIDE/secret.txt" "$TARGET" && [ -L "$TARGET" ] && [ "$(readlink "$TARGET")" = "$OUTSIDE/secret.txt" ]; then
    # Re-check after the replacement.  The mover can finish between the
    # pre-injection check and `ln`; that leaves a regular destination and a
    # new but irrelevant source link.  It is not a witnessed race, so discard
    # this attempt rather than judging its regular destination as unsafe.
    if [ -e "$DEST" ] && [ ! -L "$DEST" ]; then
      wait "$PID" 2>/dev/null || true
      continue
    fi
    INJECTED=1
  else
    wait "$PID" 2>/dev/null || true
    continue
  fi
  wait "$PID"
  CODE=$?
  break
done

if [ "$INJECTED" != "1" ]; then
  unproven "symlink swap mid-apply (same device)" "could not replace the planned source with the external symlink across ${#FRACTIONS[@]} timed attempts (baseline ${T0}ms)"
  exit 0
fi

pass "symlink swap mid-apply: replaced the planned source with a verified external link at ~${DELAY_MS}ms (group position $IDX)"

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
