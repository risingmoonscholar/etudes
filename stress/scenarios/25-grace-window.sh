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

# --- a file dated in the future, with the window off -------------------------
# elapsed() returns Err for a timestamp ahead of now, and the old code turned
# that error into "too recent" via unwrap_or(true). So `--since 0`, which means
# hold nothing back, held back every future-dated file. Future mtimes are
# ordinary: clock skew, restored backups, unpacked archives, network volumes.
FUT="$W/future"; mkdir -p "$FUT"
for n in 1 2 3; do : > "$FUT/report_$n.pdf"; done
touch -t 203001010900 "$FUT"/report_*.pdf
FUT_OUT=$("$SWEEP" "$FUT" --since 0 2>&1)
if grep -qE '^  Documents' <<<"$FUT_OUT"; then
  pass "--since 0 means zero for a file dated in the future too"
else
  fail "three PDFs dated 2030 were held back by a scan told to hold nothing back: $FUT_OUT"
fi

# With a real window, a future mtime IS held back. That is the conservative
# reading of a clock that disagrees with this one, and it is a decision now
# rather than the fallback of an unwrap.
FUT_OUT2=$("$SWEEP" "$FUT" --since 1d 2>&1)
if grep -q "changed too recently" <<<"$FUT_OUT2"; then
  pass "with a real window, a future-dated file is still held back deliberately"
else
  fail "a future-dated file was swept with a 1d window: $FUT_OUT2"
fi

# --- stash's clock becomes a contract ----------------------------------------
# pop --if-due is the automation gate: early is exit 2 (refused by the clock,
# distinct from 1 = nothing stashed, so a launchd job can tell "fire again"
# from "clean yourself up"), due is a normal pop, and a typo of the flag is
# refused rather than popping early -- the typo case is the one that would
# have silently voided the contract.
SC="$W/stashclock"; mkdir -p "$SC"; for n in 1 2 3; do : > "$SC/f_$n.txt"; done
"$STASH" "$SC" --for 1w >/dev/null 2>&1 || fail "stash --for failed"
CODE=0; "$STASH" pop "$SC" --if-due >/dev/null 2>&1 || CODE=$?
assert_eq 2 "$CODE" "pop --if-due a week early is refused by the clock"
[ -f "$SC/f_1.txt" ] && fail "an early --if-due pop moved files" || pass "and nothing came back early"
CODE=0; "$STASH" pop "$SC" --ifdue >/dev/null 2>&1 || CODE=$?
assert_eq 2 "$CODE" "a typo of --if-due is refused, not treated as a plain pop"
[ ! -f "$SC/f_1.txt" ] && pass "the typo moved nothing" || fail "a typo'd flag popped the stash"
# a plain early pop is a human right, and says it is early
EARLY_OUT=$("$STASH" pop "$SC" 2>&1); CODE=$?
assert_eq 0 "$CODE" "a plain early pop still works"
grep -q "popping .* early" <<<"$EARLY_OUT" \
  && pass "and says in one line that it is early" \
  || fail "the early pop was silent about the clock: $EARLY_OUT"
[ -f "$SC/f_1.txt" ] && pass "files restored" || fail "pop lost files"

# nothing stashed: exit 1, distinct from the refusal
CODE=0; "$STASH" pop "$SC" --if-due >/dev/null 2>&1 || CODE=$?
assert_eq 1 "$CODE" "--if-due with nothing stashed is exit 1, so a job knows to clean itself up"

# --if-due with a DEADLINE-LESS stash pops, exit 0: nothing to be early
# against. This is the deterministic integration proof of the exit-0 path.
# The "past a real deadline pops" arithmetic is proven exhaustively in the
# pop_clock unit test -- deterministically, without time control, because the
# minimum --for unit is a minute and a real overdue stash cannot be forged
# (its journal records paths inside the original holding-dir name, which a
# backdating rename would orphan; that attempt is what caught this).
NODL="$W/stashnodl"; mkdir -p "$NODL"; for n in 1 2 3; do : > "$NODL/g_$n.txt"; done
"$STASH" "$NODL" >/dev/null 2>&1 || fail "stash without a deadline failed"
CODE=0; "$STASH" pop "$NODL" --if-due >/dev/null 2>&1 || CODE=$?
assert_eq 0 "$CODE" "pop --if-due on a deadline-less stash pops, exit 0"
[ -f "$NODL/g_1.txt" ] && pass "and the files came back" || fail "the pop lost files"

# status --all on a POPULATED machine: redact by default. Hold a stash for
# the duration of this assertion, then release it -- the earlier version ran
# after its only stash was popped, so "Nothing stashed anywhere" passed the
# redaction check without a populated response to redact.
HELD="$W/stashheld"; mkdir -p "$HELD"; for n in 1 2 3; do : > "$HELD/h_$n.txt"; done
"$STASH" "$HELD" --for 1w >/dev/null 2>&1 || fail "stash --for held failed"
ALL_OUT=$("$STASH" status --all 2>&1); true
grep -q "stashheld" <<<"$ALL_OUT" && fail "status --all showed a full path with no --paths: $ALL_OUT" \
  || pass "status --all redacts a real held stash's path"
grep -q "redacted" <<<"$ALL_OUT" && pass "and says paths are redacted" \
  || fail "status --all did not disclose that it redacts: $ALL_OUT"
"$STASH" pop "$HELD" >/dev/null 2>&1 || true
# The harness is not a terminal, which makes it the exact caller the --paths
# gate exists for: the refusal must fire here, state its reasoning, and leak
# nothing. journal-dump answers to the same rule.
CODE=0; PATHS_OUT=$("$STASH" status --all --paths 2>&1) || CODE=$?
assert_eq 2 "$CODE" "--paths without a terminal is refused"
grep -q "person at a terminal" <<<"$PATHS_OUT" \
  && pass "and the refusal states its reasoning" \
  || fail "the --paths refusal gave no reason: $PATHS_OUT"
grep -q "$W" <<<"$PATHS_OUT" && fail "the refusal itself leaked a path" || pass "and leaked nothing"
