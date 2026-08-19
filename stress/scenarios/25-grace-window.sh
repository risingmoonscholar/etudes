#!/usr/bin/env bash
# The grace window, tested against real clocks and a real filesystem.
#
# The rest of the suite runs with SWEEP_GRACE_SECS=0 -- every scenario builds a
# tree and sweeps it in the same second, so all of them would be inside the
# default window and none of them is about it. This one sets its own value and
# is the only place the window is exercised end to end.
#
# What it proves:
#   - a file changed inside the window is not moved
#   - the same file IS moved once the window is behind it
#   - the window is on mtime, not atime: reading a file does not re-protect it
#   - a download in flight is refused regardless of the window
#   - the summary says which files were held and why, rather than reporting a
#     folder as tidy while holding things back
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir); trap 'rm -rf "$W"' EXIT
D="$W/Desktop"; mkdir -p "$D"

# Four documents, all written now. With a one-hour window every one is inside.
for i in 1 2 3 4; do : > "$D/paper_$i.pdf"; done

OUT=$(SWEEP_GRACE_SECS=3600 "$SWEEP" "$D" 2>&1)
if grep -q "changed too recently" <<<"$OUT"; then
  pass "files inside the window are reported as held back, with the reason on the line"
else
  fail "four just-written files were not reported as too recent: $OUT"
fi

if grep -qE '^  Documents' <<<"$OUT"; then
  fail "a group formed from files written seconds ago, inside a one-hour window: $OUT"
else
  pass "no group formed from files inside the window"
fi

# The claim a summary must never make: "nothing here needs organising" while
# holding four files back. This is the shape of the misreport that started the
# taxonomy work, in a new place.
if grep -q "Nothing here needs organising" <<<"$OUT"; then
  fail "the summary called the folder tidy while holding 4 files inside the grace window: $OUT"
else
  pass "the summary does not claim the folder is tidy while holding files back"
fi

# --- the window is on mtime, not atime ----------------------------------
# Backdate the files past the window, then READ one. If the window used atime,
# reading would re-protect it -- which is how a Spotlight reindex would freeze
# a whole folder forever.
for i in 1 2 3 4; do touch -t 202601010900 "$D/paper_$i.pdf"; done
cat "$D/paper_1.pdf" > /dev/null

OUT2=$(SWEEP_GRACE_SECS=3600 "$SWEEP" "$D" 2>&1)
if grep -qE '^  Documents +4 files' <<<"$OUT2"; then
  pass "reading a file does not re-protect it: the window is mtime, not atime"
else
  fail "after backdating and reading one file, the group did not form. If reading re-protected it, the window is keyed on atime and a Spotlight reindex would freeze any folder: $OUT2"
fi

# --- in flight, regardless of age ---------------------------------------
: > "$D/movie.mp4.part"
touch -t 202601010900 "$D/movie.mp4.part"

OUT3=$(SWEEP_GRACE_SECS=0 "$SWEEP" "$D" 2>&1)
if grep -q "still in progress" <<<"$OUT3"; then
  pass "a download in flight is held back and says so, even with the window off and an old mtime"
else
  fail "an old .part file was not reported as in flight: $OUT3"
fi

MOVED=$(SWEEP_GRACE_SECS=0 "$SWEEP" "$D" --json 2>/dev/null | python3 -c "
import json,sys
d=json.load(sys.stdin)
print(any('.part' in f for g in d['groups'] for f in g.get('members',[])))
")
assert_eq "False" "$MOVED" "the in-flight download is in no group"
