#!/usr/bin/env bash
# The exact boundary around the tool's internal item cap
# (ScanConfig::default().max_entries = 20_000, in
# etude-core/src/scan.rs, not mentioned anywhere in README or --help).
#
# Sweep and stash both refuse a flat directory over 20,000 items before a
# journal is ever opened. This scenario verifies the narrower, useful fact:
# the refusal is exact at the boundary and does not silently truncate the walk
# (which would be far worse than refusing. It would mean acting on a
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
  fail "exactly 20,000 items was refused (exit $EC_AT): ${OUT_AT:0:200}. The cap comment says 'exceeds'. 20,000 itself should pass"
fi

OVER="$W/over_cap"
build_flat "$OVER" 20001
OVER_BEFORE="$W/over-before.json"
snapshot_tree "$OVER" "$OVER_BEFORE"
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
OVER_AFTER_SWEEP="$W/over-after-sweep.json"
snapshot_tree "$OVER" "$OVER_AFTER_SWEEP"
assert_snapshot_eq "$OVER_BEFORE" "$OVER_AFTER_SWEEP" "the refused sweep changed no path, byte, link, or directory"

# Stash shares the same ScanConfig default (whole_units=true, but a flat
# directory of loose files still counts one entry per file). Reuse the same
# cap-plus-one tree instead of allocating another 50,000 empty files.
t0=$(date +%s.%N)
OUT_STASH=$("$STASH" "$OVER" 2>&1)
EC_STASH=$?
t1=$(date +%s.%N)
S_STASH=$(echo "$t1-$t0"|bc)
printf '    stash at 20,001: exit=%s  %ss\n' "$EC_STASH" "$S_STASH" >&2
assert_eq 2 "$EC_STASH" "stash also refuses the cap-plus-one fixture outright"
OVER_AFTER="$W/over-after.json"
snapshot_tree "$OVER" "$OVER_AFTER"
assert_snapshot_eq "$OVER_AFTER_SWEEP" "$OVER_AFTER" "the refused stash changed no path, byte, link, or directory"

echo "" >&2
echo "    ── verdict ──" >&2
echo "    The relevant boundary is 20,000 items. Both tools reject one more" >&2
echo "    without mutation; flat-10k measures actual apply cost separately." >&2
