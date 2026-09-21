#!/usr/bin/env bash
# 10,000 files in one flat directory (the "camera roll dumped straight onto
# Desktop" shape), at a size fixtures never reach. Two things get measured:
#
#   1. Real wall-clock for plan / apply / undo, so "usable at scale" has a
#      number instead of a guess. The journal fsyncs once per moved file
#      (see etude-core/src/journal.rs record_done) for crash-safety, which is
#      the right tradeoff for durability but means apply cost is dominated by
#      sync latency, not CPU. That should show up here as a roughly-linear
#      apply time far slower than plan or undo.
#
#   2. Whether classification holds up on real camera filenames at scale.
#      JEITA CP-3461B / CIPA DC-009-2010 section 4.3.1, read directly (not a
#      summary): a DCF file number is a 4-digit number between "0001" and
#      "9999"; "0000" is explicitly excluded. That is 9999 values, not
#      10,000. This generates exactly that range, IMG_0001..IMG_9999, one
#      file short of the round 10k this scenario is named for -- the extra
#      slot is IMG_0000, which is not DCF-conforming and would not exercise
#      what this scenario tests. It replaces the IMG_%05d (five digits) this
#      scenario used to write, which no camera produces (issue #11,
#      CONTRIBUTING.md). A guard in classify.rs now excludes a numeric marker
#      on a DCF-shaped name, so this is a regression witness for that guard,
#      not a live reproduction.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir); trap 'rm -rf "$W"' EXIT
D="$W/flat"; mkdir -p "$D"
# Measure organization and journaling throughput, not this runner's login
# keychain availability. The supplied ephemeral key still exercises sealed,
# fsync-backed journals and makes the load case portable.
export ETUDE_JOURNAL_KEY="$(python3 -c 'import secrets; print(secrets.token_hex(32))')"

N=9999
echo "    building $N DCF-conforming files (IMG_0001..IMG_9999)..." >&2
t0=$(date +%s.%N)
for i in $(seq 1 $N); do
  printf -v padded '%04d' "$i"
  : > "$D/IMG_${padded}.jpg"
done
t1=$(date +%s.%N)
BUILD_S=$(echo "$t1 - $t0" | bc)
printf '    (fixture build: %ss for %d files)\n' "$BUILD_S" "$N" >&2

BEFORE_COUNT=$(find "$D" -type f | wc -l | tr -d ' ')
BEFORE_MANIFEST="$W/before.json"
AFTER_PLAN_MANIFEST="$W/after-plan.json"
AFTER_UNDO_MANIFEST="$W/after-undo.json"
snapshot_tree "$D" "$BEFORE_MANIFEST"
assert_eq "$N" "$BEFORE_COUNT" "fixture tree has exactly $N files before anything runs"

# --- plan --------------------------------------------------------------
t0=$(date +%s.%N)
PLAN_JSON=$("$SWEEP" "$D" --json 2>/tmp/plan_err_$$.txt)
PLAN_EC=$?
t1=$(date +%s.%N)
PLAN_S=$(echo "$t1 - $t0" | bc)
rm -f /tmp/plan_err_$$.txt

assert_eq 0 "$PLAN_EC" "plan exits 0 on a ~10k-file flat directory"
snapshot_tree "$D" "$AFTER_PLAN_MANIFEST"
assert_snapshot_eq "$BEFORE_MANIFEST" "$AFTER_PLAN_MANIFEST" "planning changed no path or byte in the 10k fixture"

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
# actually did, and name it if so. A group member vanishing into "personal
# record: tax documents" because its index happened to be 1099 is a real,
# user-visible false positive. It is specific to scale: the reference
# desktop fixture (a few hundred files, hand-picked names) cannot produce it.
if [ "${PERSONAL:-0}" -gt 0 ]; then
  fail "a DCF-conforming camera file was misclassified as a personal record: $PERSONAL of $N were. IMG_1099.jpg or IMG_1040.jpg would collide with the tax-form markers \"1099\"/\"1040\" by coincidence of their index, and the camera-name guard in classify.rs (is_dcf_camera_stem, see issue #11) is meant to exclude exactly that. grep etude-core/src/classify.rs for is_dcf_camera_stem."
else
  pass "every DCF-conforming filename in this run (IMG_0001..IMG_9999, including IMG_1040 and IMG_1099) was correctly left unflagged -- deterministic, not probabilistic: the range is exhaustive, not sampled"
fi

# --- apply (real journal, real fsync-per-move) --------------------------
t0=$(date +%s.%N)
APPLY_OUT=$("$SWEEP" apply "$D" --yes 2>&1)
APPLY_EC=$?
t1=$(date +%s.%N)
APPLY_S=$(echo "$t1 - $t0" | bc)
assert_eq 0 "$APPLY_EC" "apply exits 0 on the ~10k-file plan"

[ ! -e "$D/IMG_0001.jpg" ] && find "$D" -mindepth 2 -type f -name IMG_0001.jpg | grep -q . \
  && pass "apply moved a representative camera file out of its origin" \
  || fail "apply did not move the representative camera file into a destination directory"

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

snapshot_tree "$D" "$AFTER_UNDO_MANIFEST"
assert_snapshot_eq "$BEFORE_MANIFEST" "$AFTER_UNDO_MANIFEST" "undo restored every original 10k path and byte"

printf '    undo:  %ss\n' "$UNDO_S" >&2

# --- the usability verdict, stated plainly ------------------------------
echo "" >&2
echo "    ── scale verdict ──" >&2
printf '    measured:      N=%-6s plan=%ss  apply=%ss  undo=%ss\n' "$N" "$PLAN_S" "$APPLY_S" "$UNDO_S" >&2
# This is one measured runner result, not a universal throughput claim.
# The 20k cap and 50k refusal have their own boundary scenario.
# Throughput is recorded above, but it is not a correctness exemption. A
# hardware-specific threshold belongs in the historical result table, where it
# cannot allowlist unrelated assertions in this file.
pass "apply timing recorded for this runner (${APPLY_S}s for $N files); integrity assertions remain independently gating"
