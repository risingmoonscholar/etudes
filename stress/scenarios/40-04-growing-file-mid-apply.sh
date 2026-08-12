#!/usr/bin/env bash
# Race: a file is still being appended to, by a process holding it open,
# while sweep scans it, fingerprints it, and moves it.
#
# What must hold: the move (hard_link → unlink on the common same-device
# path) operates on directory entries, never file content, so a concurrent
# writer's open file descriptor keeps writing to the same inode no matter
# which name currently points at it or which directory it lives in. The file
# must come out the other side with every byte the writer actually wrote
# (nothing dropped, nothing truncated), regardless of when sweep's move lands
# relative to the writes.
#
# Nothing here is real data. Every tree is generated and removed on exit.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

require python3 "builds fixture trees, runs the writer, and verifies content" || exit 0

N=500

build_filler() {
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

W=$(workdir); trap 'rm -rf "$W"' EXIT
D="$W/Desktop"
build_filler "$D"

TARGET="$D/batch_growing.dat"
: > "$TARGET"
RESULT="$W/writer-result.txt"

# Appends a deterministic, verifiable byte stream to $TARGET for a fixed wall
#-clock duration, independent of how long sweep takes. It is writing before
# sweep starts, during the scan, during fingerprinting, and during (or
# around) the move, whichever the OS scheduler lands it on.
python3 - "$TARGET" "$RESULT" <<'PY' &
import sys, time, hashlib
path, result_path = sys.argv[1], sys.argv[2]
duration = 6.0
f = open(path, "ab", buffering=0)
h = hashlib.sha256()
total = 0
i = 0
start = time.time()
while time.time() - start < duration:
    chunk = (f"CHUNK-{i:08d}-".encode() * 64)[:1024]
    f.write(chunk)
    h.update(chunk)
    total += len(chunk)
    i += 1
    time.sleep(0.003)
f.close()
with open(result_path, "w") as r:
    r.write(f"{total}\t{h.hexdigest()}\n")
PY
WRITER_PID=$!

# Give the writer a head start so there's real content and an open fd before
# sweep ever looks at the path, then run sweep while it's still appending.
python3 -c "import time; time.sleep(0.3)"
"$SWEEP" apply "$D" --yes >"$W/apply.out" 2>"$W/apply.err"
APPLY_CODE=$?

wait "$WRITER_PID"

if [ ! -f "$RESULT" ]; then
  unproven "growing file survives a concurrent move" "the writer never finished. Could not establish an expected byte count"
  exit 0
fi
EXPECT_SIZE="$(cut -f1 "$RESULT")"
EXPECT_HASH="$(cut -f2 "$RESULT")"

if [ "$APPLY_CODE" != "0" ]; then
  echo "    (apply exited $APPLY_CODE: $(cat "$W/apply.err")). Checking the file survived regardless"
fi

FOUND="$(find "$D" -name "batch_growing.dat" 2>/dev/null | head -1)"
if [ -z "$FOUND" ]; then
  fail "the growing file cannot be found anywhere in the tree after apply. It was lost"
  exit 0
fi

ACTUAL_SIZE=$(stat -f '%z' "$FOUND")
ACTUAL_HASH=$(shasum -a 256 "$FOUND" | awk '{print $1}')

assert_eq "$EXPECT_SIZE" "$ACTUAL_SIZE" "final file size matches every byte the writer actually wrote (no truncation)"
assert_eq "$EXPECT_HASH" "$ACTUAL_HASH" "final file content is byte-identical to what the writer wrote (no corruption, no interleaving damage)"

if [ "$FOUND" != "$TARGET" ]; then
  echo "    (moved to $FOUND while $((EXPECT_SIZE)) bytes were being written)"
else
  echo "    (still at the original path, apply did not reach it before this check; content checked in place)"
fi
