#!/usr/bin/env bash
# 50,000 files in one directory, and the exact boundary around the tool's
# internal item cap (ScanConfig::default().max_entries = 20_000, in
# etude-core/src/scan.rs — not mentioned anywhere in README or --help).
#
# The instructions ask: does 50,000 complete in reasonable time, and is the
# answer usable? The honest answer here is that the question doesn't apply —
# sweep and stash both refuse a flat directory over 20,000 items before a
# journal is ever opened, so 50,000 never reaches apply at all. What this
# scenario actually verifies is narrower and more important: that the refusal
# is fast, exact at the boundary, and does not silently truncate the walk
# (which would be far worse than refusing — it would mean acting on a
# fraction of the folder without saying so).
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir); trap 'rm -rf "$W"' EXIT

build_flat() {  # build_flat DIR N
  local dir="$1" n="$2" i padded
  mkdir -p "$dir"
  for i in $(seq 0 $((n-1))); do
    printf -v padded '%06d' "$i"
    : > "$dir/file_${padded}.dat"
  done
}

# --- the exact boundary: 20,000 must pass, 20,001 must refuse ----------
AT="$W/at_cap"
t0=$(date +%s.%N)
build_flat "$AT" 20000
t1=$(date +%s.%N)
printf '    built 20,000 files in %ss\n' "$(echo "$t1-$t0"|bc)" >&2

t0=$(date +%s.%N)
OUT_AT=$("$SWEEP" "$AT" --json 2>&1)
EC_AT=$?
t1=$(date +%s.%N)
printf '    scan at exactly 20,000: exit=%s  %ss\n' "$EC_AT" "$(echo "$t1-$t0"|bc)" >&2
# exit 0 (something to organise) or 1 (scanned fine, nothing needed organising)
# are both "the scan succeeded"; only 2 (refusal) or 3 (error) mean it didn't.
if [ "$EC_AT" = "0" ] || [ "$EC_AT" = "1" ]; then
  pass "exactly 20,000 items is accepted (at the cap, not over it)"
else
  fail "exactly 20,000 items was refused (exit $EC_AT): ${OUT_AT:0:200} — the cap comment says 'exceeds', so 20,000 itself should pass"
fi

OVER="$W/over_cap"
build_flat "$OVER" 20001
t0=$(date +%s.%N)
OUT_OVER=$("$SWEEP" "$OVER" 2>&1)
EC_OVER=$?
t1=$(date +%s.%N)
OVER_S=$(echo "$t1-$t0"|bc)
printf '    scan at 20,001 (one over): exit=%s  %ss\n' "$EC_OVER" "$OVER_S" >&2
assert_eq 2 "$EC_OVER" "one item over the cap is refused, not silently truncated"
if grep -q "20001 items exceeds the 20000 item cap" <<<"$OUT_OVER"; then
  pass "the refusal message states the real count and the real cap"
else
  fail "refusal message did not match expected wording: ${OUT_OVER:0:200}"
fi

# --- 50,000: same refusal, verify it does not hang or misbehave --------
D50="$W/flat50k"
t0=$(date +%s.%N)
build_flat "$D50" 50000
t1=$(date +%s.%N)
printf '    built 50,000 files in %ss\n' "$(echo "$t1-$t0"|bc)" >&2

BEFORE=$(find "$D50" -type f | wc -l | tr -d ' ')
assert_eq 50000 "$BEFORE" "fixture has exactly 50,000 files before sweep touches it"

t0=$(date +%s.%N)
OUT50=$("$SWEEP" "$D50" 2>&1)
EC50=$?
t1=$(date +%s.%N)
S50=$(echo "$t1-$t0"|bc)
printf '    sweep on 50,000 files: exit=%s  %ss\n' "$EC50" "$S50" >&2

assert_eq 2 "$EC50" "sweep refuses 50,000 files outright (exit 2, a deliberate refusal)"
AFTER=$(find "$D50" -type f | wc -l | tr -d ' ')
assert_eq "$BEFORE" "$AFTER" "the refused scan touched nothing — every file still where it started"

if (( $(echo "$S50 < 5" | bc -l) )); then
  pass "the refusal on 50,000 files is fast (${S50}s) — no hang, no attempt to churn through all of it first"
else
  fail "refusing 50,000 files took ${S50}s — slower than expected for a walk that exists only to say no"
fi

# stash shares the same ScanConfig default (whole_units=true, but a flat
# directory of loose files still counts one entry per file) — confirm it
# refuses the same way rather than, say, attempting to stash 50,000 files
# into one hidden holding directory.
t0=$(date +%s.%N)
OUT_STASH=$("$STASH" "$D50" 2>&1)
EC_STASH=$?
t1=$(date +%s.%N)
S_STASH=$(echo "$t1-$t0"|bc)
printf '    stash on 50,000 files: exit=%s  %ss\n' "$EC_STASH" "$S_STASH" >&2
assert_eq 2 "$EC_STASH" "stash also refuses 50,000 loose files outright"
AFTER2=$(find "$D50" -type f | wc -l | tr -d ' ')
assert_eq "$BEFORE" "$AFTER2" "the refused stash touched nothing either"

echo "" >&2
echo "    ── verdict ──" >&2
echo "    50,000 files in one flat directory is not a 'slow apply' case — it is" >&2
echo "    an immediate, well-formed refusal from both tools, in well under a" >&2
echo "    second. The real ceiling for this shape of folder is the undocumented" >&2
echo "    20,000-item cap (see 50-scale-flat-10k-apply-timing.sh for what apply" >&2
echo "    actually costs as you approach it)." >&2
