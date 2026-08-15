#!/usr/bin/env bash
# Issue #20. A genuinely cross-device move, mirroring 60-cross-volume-exdev.sh's
# structure exactly (outer tree, inner mount, confirmed different st_dev) but
# with the inner volume formatted exFAT instead of APFS, so this actually
# exercises move_one's EXDEV path into copy_data_and_stat -- the earlier
# version of this scenario put both halves on the same exFAT volume by
# mistake and only ever exercised issue #21's same-device fallback. Caught in
# review; this replaces it.
#
# Three things checked against the real copy, not assumed:
#   1. Content survives.
#   2. mtime survives. Undo's fingerprint check compares the destination's
#      mtime against what was recorded at plan time (etude-core/src/apply.rs,
#      fingerprint()); a copy that reset it would make the file look changed
#      and undo would refuse to restore it. Checked directly by running undo
#      and confirming it actually restores, not just by comparing timestamps.
#   3. An AppleDouble sidecar file, if the destination volume produces one,
#      never appears in sweep's own next scan of that folder -- the general
#      hidden-file rule, now checked by name rather than by accident. This
#      check does not require a sidecar to appear (issue #20 found that
#      depends on macOS's own provenance-tracking state, not on anything
#      sweep controls) -- it fails only if one exists AND sweep counted it.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

require hdiutil "cross-device copy scenario needs a real disk image" || exit 0

IMG=""
INNER=""
cleanup() {
  [ -n "$INNER" ] && [ -d "$INNER" ] && hdiutil detach "$INNER" -force >/dev/null 2>&1
  [ -n "${W:-}" ] && rm -rf "$W"
}
trap cleanup EXIT

W=$(workdir)
OUTER="$W/outer"
IMG="$W/inner.dmg"
INNER="$OUTER/aaa_innervol"
mkdir -p "$OUTER" "$INNER"

if ! hdiutil create -size 64m -fs ExFAT -volname CopyMtimeStress "$IMG" >/dev/null 2>&1; then
  unproven "cross-device copy: content, mtime and no sidecar in-plan" "hdiutil create failed on this host"
  exit 0
fi
if ! hdiutil attach "$IMG" -mountpoint "$INNER" -nobrowse >/dev/null 2>&1; then
  unproven "cross-device copy: content, mtime and no sidecar in-plan" "hdiutil attach failed on this host"
  exit 0
fi

DEV_OUTER=$(stat -f "%d" "$OUTER")
DEV_INNER=$(stat -f "%d" "$INNER")
if [ "$DEV_OUTER" = "$DEV_INNER" ]; then
  unproven "cross-device copy: content, mtime and no sidecar in-plan" \
    "the nested mount ended up on the same device id as the outer tree ($DEV_OUTER) on this host"
  exit 0
fi
pass "cross-device copy: outer tree and exFAT mount are confirmed on different devices ($DEV_OUTER vs $DEV_INNER)"

for n in 1 2 3; do
  printf 'photo %s' "$n" > "$OUTER/IMG_104$n.jpg"
done
# Backdate every source file so "the copy just happened to land on the
# current time" cannot pass the mtime check by accident.
touch -t 202001010000 "$OUTER"/IMG_104*.jpg
MTIME_BEFORE=$(stat -f %m "$OUTER/IMG_1041.jpg")

PLAN_OUT=$("$SWEEP" "$OUTER" --depth 2 2>&1)
if echo "$PLAN_OUT" | grep -q "Photos"; then
  pass "cross-device copy: plan groups the camera-named files"
else
  fail "cross-device copy: plan did not group the files: $PLAN_OUT"
fi

assert_exit 0 "cross-device copy: apply exits 0, moving the outer files onto the exFAT mount" \
  -- "$SWEEP" apply "$OUTER" --only Photos --yes --depth 2

# The group's destination is created under $INNER since --only Photos moves
# only that group and INNER is where scan found the group's shared folder
# context to live once it is nested in OUTER at depth 2 -- confirm by
# locating the moved files rather than assuming a fixed path.
CONTENT_OK=1
MTIME_OK=1
for n in 1 2 3; do
  f=$(find "$OUTER" -name "IMG_104$n.jpg" -path "*aaa_innervol*" | head -1)
  if [ -z "$f" ]; then
    CONTENT_OK=0
    MTIME_OK=0
    continue
  fi
  [ "$(cat "$f")" = "photo $n" ] || CONTENT_OK=0
  [ "$(stat -f %m "$f")" = "$MTIME_BEFORE" ] || MTIME_OK=0
done
if [ "$CONTENT_OK" = 1 ]; then
  pass "cross-device copy: every file's content survived the copy intact"
else
  fail "cross-device copy: content was lost or corrupted crossing the device boundary"
fi
if [ "$MTIME_OK" = 1 ]; then
  pass "cross-device copy: mtime survived the copy (copy_data_and_stat's COPYFILE_STAT)"
else
  fail "cross-device copy: mtime did not survive -- undo's fingerprint check would refuse these files"
fi

# The claim that matters more than the mtime number itself: undo actually
# works, which it only can if the fingerprint recorded at plan time still
# matches the destination.
assert_exit 0 "cross-device copy: undo succeeds, proving the mtime fingerprint still matched" \
  -- "$SWEEP" undo
RESTORED=0
for n in 1 2 3; do
  [ -f "$OUTER/IMG_104$n.jpg" ] && RESTORED=$((RESTORED + 1))
done
assert_eq 3 "$RESTORED" "cross-device copy: undo restored every file to the outer tree"

# The AppleDouble check. Re-apply so there is something on the exFAT volume
# to re-scan, whether or not a sidecar appeared alongside it.
"$SWEEP" apply "$OUTER" --only Photos --yes --depth 2 >/dev/null 2>&1
RESCAN_FILE="$W/rescan.json"
"$SWEEP" "$INNER" --json >"$RESCAN_FILE" 2>/dev/null
if ! python3 -c "
import json, sys
with open('$RESCAN_FILE') as f:
    d = json.load(f)
members = [m for g in d.get('groups', []) for m in g.get('members', [])]
sidecar = [m for m in members if m.split('/')[-1].startswith('._')]
sys.exit(1 if sidecar else 0)
"; then
  fail "cross-device copy: an AppleDouble sidecar file was picked up by sweep's own scan"
else
  pass "cross-device copy: no AppleDouble sidecar appears in sweep's own plan of the exFAT destination"
fi

hdiutil detach "$INNER" -force >/dev/null 2>&1
INNER=""
