#!/usr/bin/env bash
# 10,000 files in one flat directory — the "camera roll dumped straight onto
# Desktop" shape, at a size fixtures never reach. Two things get measured:
#
#   1. Real wall-clock for plan / apply / undo, so "usable at scale" has a
#      number instead of a guess. The journal fsyncs once per moved file
#      (see etude-core/src/journal.rs record_done) for crash-safety, which is
#      the right tradeoff for durability but means apply cost is dominated by
#      sync latency, not CPU. That should show up here as a roughly-linear
#      apply time far slower than plan or undo.
#
#   2. Whether classification holds up when filenames are sequential numbers
#      instead of hand-picked fixture names. The sensitive-marker detector
#      does plain substring matching (etude-core/src/classify.rs,
#      SENSITIVE_MARKERS) against markers like "1099" and "1040" — both of
#      which are also plausible 4-digit substrings of an ordinary sequential
#      camera index. At fixture scale (tens of files) that collision almost
#      never fires. At 10,000 sequential files it is close to guaranteed.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir); trap 'rm -rf "$W"' EXIT
D="$W/flat"; mkdir -p "$D"

N=10000
echo "    building $N files..." >&2
t0=$(date +%s.%N)
for i in $(seq 0 $((N-1))); do
  printf -v padded '%05d' "$i"
  : > "$D/IMG_${padded}.jpg"
done
t1=$(date +%s.%N)
BUILD_S=$(echo "$t1 - $t0" | bc)
printf '    (fixture build: %ss for %d files)\n' "$BUILD_S" "$N" >&2

BEFORE=$(find "$D" -type f | wc -l | tr -d ' ')
assert_eq "$N" "$BEFORE" "fixture tree has exactly $N files before anything runs"

# --- plan --------------------------------------------------------------
t0=$(date +%s.%N)
PLAN_JSON=$("$SWEEP" "$D" --json 2>/tmp/plan_err_$$.txt)
PLAN_EC=$?
t1=$(date +%s.%N)
PLAN_S=$(echo "$t1 - $t0" | bc)
rm -f /tmp/plan_err_$$.txt

assert_eq 0 "$PLAN_EC" "plan exits 0 on a 10,000-file flat directory"
assert_intact "$D" "$BEFORE" "planning moved nothing"

SCANNED=$(grep -o '"scanned":[0-9]*' <<<"$PLAN_JSON" | head -1 | cut -d: -f2)
assert_eq "$N" "$SCANNED" "plan scanned all $N files (none silently dropped)"

GROUP_COUNT=$(grep -o '"count":[0-9]*' <<<"$PLAN_JSON" | head -1 | cut -d: -f2)
PERSONAL=$(grep -o '"looks_personal":[0-9]*' <<<"$PLAN_JSON" | head -1 | cut -d: -f2)

printf '    plan: %ss  (scanned=%s  grouped=%s  looks_personal=%s)\n' \
  "$PLAN_S" "$SCANNED" "$GROUP_COUNT" "$PERSONAL" >&2

# --- the substring false-positive, made almost inevitable by scale -----
# "1099" and "1040" are tax-form markers (classify.rs SENSITIVE_MARKERS) and
# also ordinary 4-digit substrings. Among 10,000 sequential zero-padded
# indices, at least one is expected to collide with each. Confirm whether it
# actually did, and name it if so — a group member vanishing into "personal
# record: tax documents" because its index happened to be 1099 is a real,
# user-visible false positive, and it is specific to scale: the reference
# desktop fixture (a few hundred files, hand-picked names) cannot produce it.
if [ "${PERSONAL:-0}" -gt 0 ]; then
  fail "no ordinary sequential camera file is misclassified as a personal record: $PERSONAL of $N were — likely IMG_01099.jpg / IMG_01040.jpg colliding with the tax-form substring markers \"1099\"/\"1040\" purely by coincidence of their index. Repro: touch 10000 sequentially-numbered IMG_NNNNN.jpg files and run \`sweep DIR --json\`; left_alone.by_category reports tax documents even though nothing tax-related exists in the tree. This is a real false positive that only shows up at this scale — grep etude-core/src/classify.rs for SENSITIVE_MARKERS, entries (\"1099\", Category::Tax) and (\"1040\", Category::Tax) are unqualified substrings with no word-boundary check."
else
  pass "no sequential-index/tax-marker substring collision occurred in this run (probabilistic — see comment; a 0-personal outcome here does not mean the detector is safe, only that this particular seed of indices didn't collide)"
fi

# --- apply (real journal, real fsync-per-move) --------------------------
t0=$(date +%s.%N)
APPLY_OUT=$("$SWEEP" apply "$D" --yes 2>&1)
APPLY_EC=$?
t1=$(date +%s.%N)
APPLY_S=$(echo "$t1 - $t0" | bc)
assert_eq 0 "$APPLY_EC" "apply exits 0 on the 10,000-file plan"

AFTER=$(find "$D" -type f | wc -l | tr -d ' ')
assert_eq "$BEFORE" "$AFTER" "apply lost no files (count identical, wherever they now live)"

: "${GROUP_COUNT:=0}"
if [ "$GROUP_COUNT" -gt 0 ]; then
  MS_PER_FILE=$(echo "$APPLY_S * 1000 / $GROUP_COUNT" | bc -l)
else
  MS_PER_FILE="n/a"
fi
printf '    apply: %ss for ~%s moves (%sms/file)  journal: yes\n' \
  "$APPLY_S" "$GROUP_COUNT" "$MS_PER_FILE" >&2

# --- undo -----------------------------------------------------------
t0=$(date +%s.%N)
UNDO_OUT=$("$SWEEP" undo 2>&1)
UNDO_EC=$?
t1=$(date +%s.%N)
UNDO_S=$(echo "$t1 - $t0" | bc)
assert_eq 0 "$UNDO_EC" "undo exits 0"

RESTORED=$(find "$D" -maxdepth 1 -type f | wc -l | tr -d ' ')
assert_eq "$BEFORE" "$RESTORED" "undo returned every file to the flat directory (exact baseline count)"

printf '    undo:  %ss\n' "$UNDO_S" >&2

# --- the usability verdict, stated plainly ------------------------------
# Linear extrapolation from THIS measured apply time (not a guess): the
# journal's per-move fsync makes apply cost ~proportional to file count.
EXTRAP_20K=$(echo "$APPLY_S * 2" | bc)
EXTRAP_50K=$(echo "$APPLY_S * 5" | bc)
echo "" >&2
echo "    ── scale verdict ──" >&2
printf '    measured:      N=%-6s plan=%ss  apply=%ss  undo=%ss\n' "$N" "$PLAN_S" "$APPLY_S" "$UNDO_S" >&2
printf '    extrapolated:  N=20000 (the scan cap) apply ≈ %ss\n' "$EXTRAP_20K" >&2
printf '    extrapolated:  N=50000 apply ≈ %ss  — but N=50000 can never reach apply: see 50-scale-cap-boundary-50k.sh, sweep refuses at scan time (20,000-item cap) before a journal is ever opened.\n' "$EXTRAP_50K" >&2
if (( $(echo "$APPLY_S > 30" | bc -l) )); then
  fail "apply --yes on a 10,000-file Desktop-shaped folder took ${APPLY_S}s (>30s) with the journal on. A user who dumps a 10k-photo camera roll onto Desktop and runs sweep apply will sit and wait roughly a minute doing nothing else with that terminal. This is a genuine usability ceiling worth having a number for, not a pass/fail bug — reported here because 'slow is a finding, not a failure of the test.'"
else
  pass "apply on 10,000 files completed in a plainly usable time (${APPLY_S}s)"
fi
