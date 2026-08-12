#!/usr/bin/env bash
# Race: the destination directory sweep is about to create already exists.
# But it is a plain FILE, not a directory. `mkdir -p` (fs::create_dir_all) on a
# path that exists and is not a directory must fail, not silently write into
# something that isn't a directory and not crash.
#
# Two variants:
#   A. deterministic: the blocking file is already there before sweep's
#      single scan+plan+apply invocation starts at all. This is the cleanest,
#      always-reproducible form of the collision and pins down the exact
#      behaviour.
#   B. genuinely mid-run: a large first group (processed before the target
#      group, by plan order) buys real wall-clock time, and the blocking file
#      is planted while that first group is still being moved, before the
#      second group's directory has been created.
#
# What must hold, in both: sweep refuses cleanly (no crash, no data loss, no
# writing into the file-that-should-have-been-a-directory), reports the
# failure honestly, and whatever it already moved before hitting the
# collision stays moved and correct.
#
# Nothing here is real data. Every tree is generated and removed on exit.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

require python3 "builds fixture trees fast" || exit 0

# --- Variant A: deterministic ---------------------------------------------

# Letter-suffixed names so exactly one token repeats across the set ("widget")
# and none of the per-file unique suffixes accidentally forms a second
# same-sized candidate group that could win the tie-break instead.
letters() {  # letters N -> deterministic short letter code, distinct per N
  python3 -c "
i=$1
s=''
for _ in range(5):
    s = chr(97 + i % 26) + s
    i //= 26
print(s)"
}

W=$(workdir); trap 'rm -rf "$W"' EXIT
DA="$W/a/Desktop"; mkdir -p "$DA"
for i in $(seq 1 8); do : > "$DA/widget_$(letters "$i").dat"; done
BLOCKER_CONTENT="I-AM-A-FILE-NOT-A-DIRECTORY-$$"
printf '%s' "$BLOCKER_CONTENT" > "$DA/widget"
BEFORE_A=$(find "$DA" -type f | wc -l | tr -d ' ')

CODE_A=0
OUT_A=$("$SWEEP" apply "$DA" --yes 2>&1) || CODE_A=$?

assert_eq 1 "$([ "$CODE_A" != "0" ] && echo 1 || echo 0)" "[A] apply refused rather than crashing or misbehaving (exit $CODE_A)"
assert_eq "$BLOCKER_CONTENT" "$(cat "$DA/widget" 2>/dev/null)" "[A] the blocking file's content is untouched. sweep did not write into it or replace it"
assert_eq "$BEFORE_A" "$(find "$DA" -type f | wc -l | tr -d ' ')" "[A] every source file is still present. Nothing was lost trying to reach a broken destination"
if echo "$OUT_A" | grep -qi "sweep:"; then
  pass "[A] the refusal was printed, not swallowed"
else
  fail "[A] apply exited $CODE_A but printed nothing identifiable: $OUT_A"
fi

# --- Variant B: mid-run, a real first group buys the window ---------------

now_ms() { python3 -c 'import time; print(int(time.time()*1000))'; }

build_two_groups() {  # $1 = Desktop dir
  mkdir -p "$1"
  python3 - "$1" <<'PY'
import sys, os
d = sys.argv[1]
def letters(i, width=5):
    s = ""
    for _ in range(width):
        s = chr(97 + i % 26) + s
        i //= 26
    return s
# Group 1: a camera burst, large, processed first by plan order. This is
# the group that buys real wall-clock time before group 2 is even reached.
for i in range(450):
    open(os.path.join(d, f"IMG_{i:04d}.jpg"), "w").close()
# Group 2: a small shared-token group ("widget"). This is the one whose
# destination directory we're about to block with a file. Letter suffixes
# keep each file's own suffix from forming a second same-sized candidate
# group that could out-rank "widget" in the tie-break.
for i in range(8):
    open(os.path.join(d, f"widget_{letters(i)}.dat"), "w").close()
PY
}

DB_CTRL="$W/b-control/Desktop"; build_two_groups "$DB_CTRL"
T0_START=$(now_ms)
"$SWEEP" apply "$DB_CTRL" --yes >/dev/null 2>&1
T0_END=$(now_ms)
T0=$((T0_END - T0_START))

if [ "$T0" -lt 30 ]; then
  unproven "[B] destination blocked mid-run by a same-named file" "baseline apply finished in ${T0}ms on this host. Too fast for a background process to land inside the window"
else
  DB="$W/b/Desktop"; build_two_groups "$DB"
  BEFORE_B=$(find "$DB" -type f | wc -l | tr -d ' ')

  "$SWEEP" apply "$DB" --yes >"$W/b.out" 2>"$W/b.err" &
  PID=$!
  DELAY_MS=$((T0 * 50 / 100))
  python3 -c "import time; time.sleep($DELAY_MS/1000)"
  BLOCKER_B="RACE-BLOCKER-$$"
  printf '%s' "$BLOCKER_B" > "$DB/widget"
  wait "$PID"
  CODE_B=$?

  if [ -f "$DB/widget" ] && [ "$(cat "$DB/widget" 2>/dev/null)" = "$BLOCKER_B" ] && [ ! -d "$DB/widget" ]; then
    echo "    (blocker planted at ~${DELAY_MS}ms into a ~${T0}ms baseline run)"
    assert_eq 1 "$([ "$CODE_B" != "0" ] && echo 1 || echo 0)" "[B] apply refused rather than succeeding over the blocked destination (exit $CODE_B)"
    assert_eq "$BLOCKER_B" "$(cat "$DB/widget" 2>/dev/null)" "[B] the mid-run blocking file's content is untouched"

    MOVED_PHOTOS=$(find "$DB" -mindepth 2 -name 'IMG_*.jpg' 2>/dev/null | wc -l | tr -d ' ')
    echo "    ($MOVED_PHOTOS of the first group's files had already landed before the collision)"
    # +1 for the blocker file itself, which we planted and which is expected
    # to still be there. It is not something the tool lost.
    assert_eq "$((BEFORE_B + 1))" "$(find "$DB" -type f | wc -l | tr -d ' ')" "[B] no file was lost anywhere in the tree. The earlier group's real moves stand, nothing else vanished"
  else
    unproven "[B] destination blocked mid-run by a same-named file" "the blocker either never survived to be checked or apply had already finished before it landed (baseline ${T0}ms, fired at ${DELAY_MS}ms)"
  fi
fi
