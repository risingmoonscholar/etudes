#!/usr/bin/env bash
# Scale across every group sweep can produce, and the bound that makes the
# group namespace safe.
#
# This scenario used to build 300 distinct groups from 300 shared tokens, and
# its central worry was name collision: two coined groups landing on the same
# name would make one of them silently unreachable via `apply --only NAME`.
# That worry existed only because group names came from user text. Names now
# come from a fixed set -- a handful of structural detectors plus seven type
# families -- so collision is impossible by construction and 300 groups is a
# shape that can no longer occur.
#
# What is still worth proving, and is proven here:
#   - the group namespace is genuinely bounded and every name is distinct
#   - a folder holding every kind of file at once produces every group, with
#     no file counted twice and none lost between them
#   - apply and undo hold at thousands of files spread across those groups
#   - the listing stays one line per group and does not blow up
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir); trap 'rm -rf "$W"' EXIT
D="$W/many"; mkdir -p "$D"

PER_FAMILY=200

# One extension per family, so every family is populated and the totals are
# arithmetic rather than guesswork. dmg is the Installers detector's, not a
# type family's -- listing both proves they do not race for the same files.
make_family() {  # make_family EXT COUNT PREFIX
  local ext="$1" n="$2" pre="$3" i=0
  while [ "$i" -lt "$n" ]; do
    : > "$D/${pre}_$(printf '%04d' "$i").$ext"
    i=$((i+1))
  done
}

make_family png "$PER_FAMILY" shot_of
make_family pdf "$PER_FAMILY" paper
make_family sh  "$PER_FAMILY" runner
make_family zip "$PER_FAMILY" bundle
make_family mp4 "$PER_FAMILY" clip
make_family csv "$PER_FAMILY" table
make_family dmg "$PER_FAMILY" installer_for

# Deliberately unmapped: .dat is a generic container many apps use privately,
# and sweep leaves what it cannot identify alone. These must land in no group.
UNMAPPED=50
make_family dat "$UNMAPPED" opaque

TOTAL=$((PER_FAMILY * 7 + UNMAPPED))
BEFORE=$(find "$D" -type f | wc -l | tr -d ' ')
assert_eq "$TOTAL" "$BEFORE" "fixture has $TOTAL files: 7 families x $PER_FAMILY, plus $UNMAPPED unidentifiable"

t0=$(date +%s.%N)
PLAN_JSON=$("$SWEEP" "$D" --json 2>&1)
PLAN_EC=$?
t1=$(date +%s.%N)
printf '    plan over %d files: %ss\n' "$TOTAL" "$(echo "$t1-$t0"|bc)" >&2
assert_eq 0 "$PLAN_EC" "plan succeeds over a folder holding every kind of file at once"

GROUP_NAMES_FILE="$W/group_names.txt"
grep -o '"name":"[^"]*"' <<<"$PLAN_JSON" | sed 's/"name":"//;s/"$//' > "$GROUP_NAMES_FILE"
GROUP_COUNT=$(wc -l < "$GROUP_NAMES_FILE" | tr -d ' ')
UNIQUE_COUNT=$(sort -u "$GROUP_NAMES_FILE" | wc -l | tr -d ' ')

assert_eq "$GROUP_COUNT" "$UNIQUE_COUNT" "every proposed group name is distinct. Two groups sharing a name would make one unreachable via apply --only NAME"

# The bound. Three structural detectors plus seven families is the entire
# namespace; anything past that means a detector is coining names again.
if [ "$GROUP_COUNT" -le 10 ]; then
  pass "the group namespace is bounded: $GROUP_COUNT groups from $TOTAL files of every kind, never one per filename pattern"
else
  fail "sweep proposed $GROUP_COUNT groups. The namespace is meant to be a fixed set of at most 10 -- something is naming groups from user text again: $(sort -u "$GROUP_NAMES_FILE" | tr '\n' ' ')"
fi

GROUPED=$(grep -o '"count":[0-9]*' <<<"$PLAN_JSON" | cut -d: -f2 | awk '{s+=$1} END {print s+0}')
assert_eq "$((TOTAL - UNMAPPED))" "$GROUPED" "every identifiable file is counted in exactly one group, and the $UNMAPPED unidentifiable ones in none"

# --- human-readable output stays sane -----------------------------------
HUMAN_OUT=$("$SWEEP" "$D" 2>&1)
HUMAN_LINES=$(grep -cE '^  [A-Z][a-z]+ +[0-9]+ files' <<<"$HUMAN_OUT")
assert_eq "$GROUP_COUNT" "$HUMAN_LINES" "the listing has exactly one line per group"

LONGEST_LINE=$(awk '{ print length }' <<<"$HUMAN_OUT" | sort -rn | head -1)
printf '    longest output line: %s chars\n' "$LONGEST_LINE" >&2
if [ "$LONGEST_LINE" -lt 200 ]; then
  pass "no single line in the listing is unreasonably wide"
else
  fail "a line in the listing is $LONGEST_LINE characters wide. Likely unreadable in a normal terminal"
fi

# --- apply moves everything into distinctly-named directories -----------
t0=$(date +%s.%N)
APPLY_OUT=$("$SWEEP" apply "$D" --yes 2>&1)
APPLY_EC=$?
t1=$(date +%s.%N)
printf '    apply over %d files: %ss\n' "$TOTAL" "$(echo "$t1-$t0"|bc)" >&2
assert_eq 0 "$APPLY_EC" "apply succeeds across every group"

AFTER=$(find "$D" -type f | wc -l | tr -d ' ')
assert_eq "$TOTAL" "$AFTER" "apply lost no files"

DEST_DIRS=$(find "$D" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')
assert_eq "$GROUP_COUNT" "$DEST_DIRS" "exactly one destination directory per group, none merged"

STILL_FLAT=$(find "$D" -maxdepth 1 -type f -name '*.dat' | wc -l | tr -d ' ')
assert_eq "$UNMAPPED" "$STILL_FLAT" "the $UNMAPPED unidentifiable files were left exactly where they were"

# --- undo reverses everything -------------------------------------------
t0=$(date +%s.%N)
"$SWEEP" undo >/dev/null 2>&1
UNDO_EC=$?
t1=$(date +%s.%N)
printf '    undo across %d files: %ss\n' "$TOTAL" "$(echo "$t1-$t0"|bc)" >&2
assert_eq 0 "$UNDO_EC" "undo succeeds across every group"

RESTORED=$(find "$D" -maxdepth 1 -type f | wc -l | tr -d ' ')
assert_eq "$TOTAL" "$RESTORED" "undo returned every file to the flat directory"
