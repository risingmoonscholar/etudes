#!/usr/bin/env bash
# Deep nesting to the documented max --depth of 8, and past it.
#
# scan.rs's walk() is recursive: `if depth >= cfg.depth.min(8) { return }`
# gates whether a directory's contents are read at all. Root itself is
# depth 0. --depth N therefore reads directory levels 0..N-1 — N reads a
# directory that many hops deep, but never opens the (N)th hop's contents.
# This scenario builds a chain 10 levels deep (root + L1..L10), with a
# uniquely-tagged group of files at every level, and checks that number
# exactly: --depth N sees levels 0..N-1 and nothing past that, for every N
# from 1 to 8, and that 0 and 9 are refused outright rather than clamped.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir); trap 'rm -rf "$W"' EXIT
D="$W/root"; mkdir -p "$D"

# Level 0 is the root itself; levels 1..10 are L1/L2/.../L10 nested. Every
# level gets 3 files sharing a level-specific token, so "was this level's
# directory ever read" is directly observable in the plan's scanned count
# and group list — not just an entry count that could hide a different bug.
cur="$D"
for lvl in $(seq 0 10); do
  if [ "$lvl" -gt 0 ]; then cur="$cur/L$lvl"; mkdir -p "$cur"; fi
  for j in 1 2 3; do : > "$cur/depthmarker${lvl}_file${j}_tok.txt"; done
done

count_scanned() {  # count_scanned DEPTH
  local out
  out=$("$SWEEP" "$D" --depth "$1" --json 2>/dev/null)
  grep -o '"scanned":[0-9]*' <<<"$out" | head -1 | cut -d: -f2
}

# --- exact boundary for every legal depth -------------------------------
for depth in 1 2 3 4 5 6 7 8; do
  expected=$((depth * 3))
  got=$(count_scanned "$depth")
  assert_eq "$expected" "$got" "--depth $depth scans exactly levels 0..$((depth-1)) (3 files each = $expected)"
done

# --- files past the requested depth are never touched, at every level --
# Directly verify level 8, 9, 10 markers do not appear in the --depth 8
# scan, by checking the group list rather than trusting the count alone.
OUT8=$("$SWEEP" "$D" --depth 8 --json 2>/dev/null)
for lvl in 8 9 10; do
  if grep -q "depthmarker${lvl}_" <<<"$OUT8"; then
    fail "--depth 8 touched a file from level $lvl, which is past the requested depth"
  else
    pass "--depth 8 never sees level $lvl (correctly beyond the boundary)"
  fi
done

# --- explicit refusal outside 1..8, never a silent clamp ----------------
assert_exit 2 "--depth 9 is refused, not silently clamped to 8" -- "$SWEEP" "$D" --depth 9
assert_exit 2 "--depth 0 is refused, not silently clamped to 1" -- "$SWEEP" "$D" --depth 0
assert_exit 2 "--depth -1 is refused (not treated as unlimited)" -- "$SWEEP" "$D" --depth -1
assert_exit 2 "a non-numeric --depth is refused" -- "$SWEEP" "$D" --depth banana
assert_exit 2 "an absurdly large --depth is refused, not silently saturated" -- "$SWEEP" "$D" --depth 99999999999999999999

# --- apply at a middle depth actually moves the right files, and only ---
# those. --depth 3 should group and move levels 0,1,2 (9 files, one group of
# 9 sharing nothing — wait, each level's token differs, so 3 separate groups
# of 3 members won't clear MIN_TOKEN_GROUP (5). Rebuild a depth-scoped tree
# with one token shared across the first 3 levels to get a real apply case.
D2="$W/apply_root"; mkdir -p "$D2/L1/L2"
for f in a b c d e; do : > "$D2/shared_${f}_deeptok.txt"; done       # level 0, 5 files
for f in f g; do : > "$D2/L1/shared_${f}_deeptok.txt"; done          # level 1, 2 files
for f in h; do : > "$D2/L1/L2/shared_${f}_deeptok.txt"; done         # level 2, 1 file — 8 total across levels 0-2
mkdir -p "$D2/L1/L2/L3"
: > "$D2/L1/L2/L3/depthtoken_should_not_move.txt"                    # level 3: out of reach at --depth 3

BEFORE2=$(find "$D2" -type f | wc -l | tr -d ' ')
STATE_BEFORE=$(find "$D2/L1/L2/L3" -type f | wc -l | tr -d ' ')
assert_eq 1 "$STATE_BEFORE" "level-3 file exists before apply (sanity check on the fixture)"

"$SWEEP" apply "$D2" --depth 3 --yes >/tmp/deepapply_$$.txt 2>&1
APPLY_EC=$?
assert_eq 0 "$APPLY_EC" "apply --depth 3 succeeds"

AFTER2=$(find "$D2" -type f | wc -l | tr -d ' ')
assert_eq "$BEFORE2" "$AFTER2" "apply --depth 3 lost no files"

if [ -f "$D2/L1/L2/L3/depthtoken_should_not_move.txt" ]; then
  pass "the level-3 file (beyond --depth 3) was never touched — still in place"
else
  fail "the level-3 file vanished from its original location even though --depth 3 should never have read that directory"
fi

if [ -d "$D2/deeptok" ] && [ -f "$D2/deeptok/shared_h_deeptok.txt" ]; then
  pass "the level-2 group member (within --depth 3) was correctly moved into the group"
else
  fail "expected level-0..2 group 'deeptok' to exist at the root with the level-2 member inside it"
fi

rm -f /tmp/deepapply_$$.txt
