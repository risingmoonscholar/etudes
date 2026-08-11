#!/usr/bin/env bash
# Hundreds of distinct groups in one folder — the shape of a shared drive or
# an old "Projects" folder with a client subfolder's worth of naming per
# client, never a handful of screenshots. Checks:
#   - every group sweep proposes gets a genuinely distinct name (no two
#     groups collide, which would make one of them silently unreachable via
#     `apply --only NAME`)
#   - the human-readable output stays one-line-per-group and doesn't wrap,
#     truncate, or otherwise become unreadable at 300 groups
#   - apply moves everything into the right, distinctly-named directories
#   - undo reverses all 300 groups back to flat, exact baseline count
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir); trap 'rm -rf "$W"' EXIT
D="$W/many"; mkdir -p "$D"

NGROUPS=300
MEMBERS_PER_GROUP=5
# Member suffixes are single letters (a, b, c, ...) on purpose: classify.rs's
# tokens() drops any token shorter than 3 characters, so these never become
# tokens themselves. The first attempt at this fixture used "_member0".."_4"
# as the differentiator and got exactly 5 groups back, not 300 — "member0"
# is itself a valid >=3-char token that recurs across all 300 filenames
# (300 members, tied for the biggest group), so the largest-group-first
# grouping rule (plan.rs: sort candidates by size desc) picked "member0"
# through "member4" as five giant groups before "grp003tag" and friends ever
# got a turn. That was a bug in this fixture, not in sweep — real distinct
# groups need a differentiator that cannot itself qualify as a token.
LETTERS="abcde"
i=0
while [ "$i" -lt "$NGROUPS" ]; do
  printf -v tok 'grp%03dtag' "$i"
  j=0
  while [ "$j" -lt "$MEMBERS_PER_GROUP" ]; do
    letter=${LETTERS:$j:1}
    : > "$D/${tok}_${letter}.dat"
    j=$((j+1))
  done
  i=$((i+1))
done

TOTAL=$((NGROUPS * MEMBERS_PER_GROUP))
BEFORE=$(find "$D" -type f | wc -l | tr -d ' ')
assert_eq "$TOTAL" "$BEFORE" "fixture has exactly $NGROUPS groups x $MEMBERS_PER_GROUP members = $TOTAL files"

t0=$(date +%s.%N)
PLAN_JSON=$("$SWEEP" "$D" --json 2>&1)
PLAN_EC=$?
t1=$(date +%s.%N)
printf '    plan over %d groups: %ss\n' "$NGROUPS" "$(echo "$t1-$t0"|bc)" >&2
assert_eq 0 "$PLAN_EC" "plan succeeds over a folder that produces $NGROUPS distinct groups"

GROUP_NAMES_FILE="$W/group_names.txt"
grep -o '"name":"[^"]*"' <<<"$PLAN_JSON" | sed 's/"name":"//;s/"$//' > "$GROUP_NAMES_FILE"
GROUP_COUNT=$(wc -l < "$GROUP_NAMES_FILE" | tr -d ' ')
assert_eq "$NGROUPS" "$GROUP_COUNT" "sweep proposed exactly $NGROUPS groups (one per shared token, none merged, none split)"

UNIQUE_COUNT=$(sort -u "$GROUP_NAMES_FILE" | wc -l | tr -d ' ')
assert_eq "$NGROUPS" "$UNIQUE_COUNT" "every group name is unique — no two groups collide"

MEMBER_TOTAL=$(grep -o '"count":[0-9]*' <<<"$PLAN_JSON" | cut -d: -f2 | awk '{s+=$1} END {print s}')
assert_eq "$TOTAL" "$MEMBER_TOTAL" "every one of the $TOTAL files ended up counted in exactly one group"

# --- human-readable output stays sane at this scale ---------------------
HUMAN_OUT=$("$SWEEP" "$D" 2>&1)
HUMAN_LINES=$(grep -c "files.*filenames contain" <<<"$HUMAN_OUT")
assert_eq "$NGROUPS" "$HUMAN_LINES" "the human-readable listing has exactly one line per group, for all $NGROUPS groups (no collapsing, no truncation of the list itself)"

LONGEST_LINE=$(awk '{ print length }' <<<"$HUMAN_OUT" | sort -rn | head -1)
printf '    longest output line: %s chars (300-group listing did not blow up)\n' "$LONGEST_LINE" >&2
if [ "$LONGEST_LINE" -lt 200 ]; then
  pass "no single line in the 300-group listing is unreasonably wide"
else
  fail "a line in the 300-group listing is $LONGEST_LINE characters wide — likely unreadable in a normal terminal"
fi

# --- apply moves everything into distinctly-named directories -----------
t0=$(date +%s.%N)
APPLY_OUT=$("$SWEEP" apply "$D" --yes 2>&1)
APPLY_EC=$?
t1=$(date +%s.%N)
printf '    apply over %d groups (%d files): %ss\n' "$NGROUPS" "$TOTAL" "$(echo "$t1-$t0"|bc)" >&2
assert_eq 0 "$APPLY_EC" "apply succeeds across $NGROUPS groups"

AFTER=$(find "$D" -type f | wc -l | tr -d ' ')
assert_eq "$TOTAL" "$AFTER" "apply lost no files across $NGROUPS groups"

DEST_DIRS=$(find "$D" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')
assert_eq "$NGROUPS" "$DEST_DIRS" "exactly $NGROUPS destination directories were created — one per group, none merged"

# Spot-check a handful across the range land in the right, uniquely-named
# place (not just that counts add up).
MISPLACED=0
for idx in 0 1 149 298 299; do
  printf -v tok 'grp%03dtag' "$idx"
  if [ ! -d "$D/$tok" ] || [ "$(find "$D/$tok" -type f | wc -l | tr -d ' ')" != "$MEMBERS_PER_GROUP" ]; then
    MISPLACED=$((MISPLACED+1))
  fi
done
assert_eq 0 "$MISPLACED" "spot-checked group directories (first, middle, last) each contain exactly their $MEMBERS_PER_GROUP members"

# --- undo reverses all 300 groups -----------------------------
t0=$(date +%s.%N)
"$SWEEP" undo >/dev/null 2>&1
UNDO_EC=$?
t1=$(date +%s.%N)
printf '    undo across %d groups: %ss\n' "$NGROUPS" "$(echo "$t1-$t0"|bc)" >&2
assert_eq 0 "$UNDO_EC" "undo succeeds across $NGROUPS groups"

RESTORED=$(find "$D" -maxdepth 1 -type f | wc -l | tr -d ' ')
assert_eq "$TOTAL" "$RESTORED" "undo returned all $TOTAL files across $NGROUPS groups to the flat directory"

REMAINING_DIRS=$(find "$D" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')
assert_eq 0 "$REMAINING_DIRS" "undo removed every one of the $NGROUPS now-empty group directories it created"
