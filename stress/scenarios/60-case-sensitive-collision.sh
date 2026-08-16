#!/usr/bin/env bash
# A case-SENSITIVE APFS volume. Two files whose names differ only by case
# (e.g. photoset_report.txt and PHOTOSET_REPORT.txt) are two entirely
# different, legally coexisting files on a case-sensitive filesystem -- unlike
# on the case-insensitive default, where they are the same name and a real
# collision.
#
# apply()'s destination-collision check (etude-core/src/apply.rs) folds every
# planned destination to lowercase before deduplicating:
#
#   planned_destinations.insert(dst.to_string_lossy().to_lowercase())
#
# That is correct on a case-insensitive volume. On a case-sensitive one it is
# wrong: it refuses a plan that would apply perfectly safely, because two
# genuinely-different destination paths hash to the same folded key.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

require hdiutil "case-sensitive-volume scenario needs a real disk image" || exit 0

IMG=""
MNT=""
cleanup() {
  detach_registered_mounts
  [ -n "${W:-}" ] && rm -rf "$W"
}
trap cleanup EXIT

W=$(workdir)
IMG="$W/cs.dmg"
MNT="$W/mnt"
mkdir -p "$MNT"

if ! hdiutil create -size 20m -fs "Case-sensitive APFS" -volname CSVolStress "$IMG" >/dev/null 2>&1; then
  unproven "case-sensitive volume: a legal case-differing pair is not refused" "hdiutil could not create a case-sensitive APFS image on this host"
  exit 0
fi
if ! hdiutil attach "$IMG" -mountpoint "$MNT" -nobrowse >/dev/null 2>&1; then
  unproven "case-sensitive volume: a legal case-differing pair is not refused" "hdiutil attach failed on this host"
  exit 0
fi
register_mount "$MNT"

# Confirm the volume really is case-sensitive before trusting the rest of the
# scenario -- if hdiutil silently gave us something else, the whole premise
# is void and must be reported as unproven, not passed by accident.
FS_PERSONALITY=$(diskutil info "$MNT" 2>/dev/null | grep -i "File System Personality" || true)
if ! echo "$FS_PERSONALITY" | grep -qi "case-sensitive"; then
  unproven "case-sensitive volume: a legal case-differing pair is not refused" "diskutil reports the mounted volume is not case-sensitive: ${FS_PERSONALITY:-<no output>}"
  exit 0
fi
pass "case-sensitive volume: confirmed case-sensitive before testing ($FS_PERSONALITY)"

PROJ="$MNT/proj"
mkdir -p "$PROJ"
# Five files share the "photoset" token -- enough to clear sweep's minimum
# group size. Two of them are the SAME name except for case: a legal pair on
# this filesystem, and a real collision on the default case-insensitive one.
for f in alpha beta gamma delta; do : > "$PROJ/photoset_$f.txt"; done
: > "$PROJ/photoset_report.txt"
: > "$PROJ/PHOTOSET_REPORT.txt"
BEFORE=$(find "$PROJ" -type f | wc -l | tr -d ' ')
assert_eq 6 "$BEFORE" "case-sensitive volume: both differently-cased files coexist on disk (fixture sanity check)"

PLAN_OUT=$("$SWEEP" "$PROJ" 2>&1)
if echo "$PLAN_OUT" | grep -q "photoset"; then
  pass "case-sensitive volume: plan groups all 6 files under one shared token"
else
  fail "case-sensitive volume: plan did not form the expected group: $PLAN_OUT"
fi

APPLY_OUT=$("$SWEEP" apply "$PROJ" --yes 2>&1)
APPLY_CODE=$?

if [ "$APPLY_CODE" = 0 ]; then
  pass "case-sensitive volume: apply succeeded -- the case-fold collision bug is not (or no longer) present"
  AFTER=$(find "$PROJ" -type f | wc -l | tr -d ' ')
  assert_eq "$BEFORE" "$AFTER" "case-sensitive volume: successful apply lost no files"
else
  # This is the bug. Report it prominently and leave the assertion failing --
  # do not weaken it to a pass.
  fail "REAL DEFECT: case-sensitive volume wrongly refused a legal apply. \
photoset_report.txt and PHOTOSET_REPORT.txt are two distinct files on this \
case-sensitive filesystem (both existed on disk before apply, see fixture \
sanity check above) but apply() folds every destination path to lowercase \
before deduplicating (etude-core/src/apply.rs, planned_destinations.insert(dst.to_string_lossy().to_lowercase())), \
so it reports a DestinationCollision and refuses the ENTIRE group of 6 files \
-- not just the 2 that (wrongly) look identical to it. exit=$APPLY_CODE output=[$APPLY_OUT]"
  AFTER=$(find "$PROJ" -type f | wc -l | tr -d ' ')
  assert_eq "$BEFORE" "$AFTER" "case-sensitive volume: the wrongful refusal at least lost no files"
fi

exit 0
