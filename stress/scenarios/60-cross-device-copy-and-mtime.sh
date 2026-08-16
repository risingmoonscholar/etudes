#!/usr/bin/env bash
# Issue #20. A genuinely cross-device move, mirroring 60-cross-volume-exdev.sh's
# structure exactly -- outer tree, inner mount, confirmed different st_dev --
# with the inner volume formatted exFAT instead of APFS.
#
# Two prior versions of this scenario made the same mistake in two different
# shapes, caught in review both times: first, source and destination on the
# SAME exFAT volume (never crossed a device at all). Second, source files
# created directly under $OUTER with $OUTER as the apply root -- the exFAT
# mount was attached and confirmed to have a different st_dev, but nothing
# was ever read from or written to it, because apply always destinations
# under plan.root (etude-core/src/apply.rs, dest_dir = plan.root.join(name)),
# so a same-device move is what actually ran regardless of what else was
# mounted nearby.
#
# What actually forces EXDEV, matching the sibling exactly: the SOURCE files
# live inside the inner mount (a subdirectory of OUTER), the apply ROOT stays
# OUTER, so the group's destination lands at $OUTER/<name>/ -- a different
# device than where the source files actually are. That is move_one's EXDEV
# path into copy_data_and_stat, for real.
#
# Three things checked against the real copy, not assumed:
#   1. Content survives.
#   2. mtime survives. Undo's fingerprint check compares the destination's
#      mtime against what was recorded at plan time (etude-core/src/apply.rs,
#      fingerprint()); a copy that reset it would make the file look changed
#      and undo would refuse to restore it. Checked by asserting the number
#      directly AND by running undo and confirming it actually restores.
#   3. An AppleDouble sidecar file, if the source exFAT volume produces one
#      as a side effect of files having been read from it, never appears in
#      sweep's own next scan of that volume.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

require hdiutil "cross-device copy scenario needs a real disk image" || exit 0

IMG=""
INNER=""
cleanup() {
  detach_registered_mounts
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
register_mount "$INNER"

DEV_OUTER=$(stat -f "%d" "$OUTER")
DEV_INNER=$(stat -f "%d" "$INNER")
if [ "$DEV_OUTER" = "$DEV_INNER" ]; then
  unproven "cross-device copy: content, mtime and no sidecar in-plan" \
    "the nested mount ended up on the same device id as the outer tree ($DEV_OUTER) on this host"
  exit 0
fi
pass "cross-device copy: outer tree and exFAT mount are confirmed on different devices ($DEV_OUTER vs $DEV_INNER)"

# The source files live ON the exFAT mount. The apply root is OUTER, so the
# group's destination is $OUTER/Photos/ -- a different device than where
# these files physically are right now. This is the part the earlier two
# versions got backwards.
for n in 1 2 3; do
  printf 'photo %s' "$n" > "$INNER/IMG_104$n.jpg"
done
touch -t 202001010000 "$INNER"/IMG_104*.jpg
MTIME_BEFORE=$(stat -f %m "$INNER/IMG_1041.jpg")

PLAN_OUT=$("$SWEEP" "$OUTER" --depth 2 2>&1)
if echo "$PLAN_OUT" | grep -q "Photos"; then
  pass "cross-device copy: plan groups the camera-named files"
else
  fail "cross-device copy: plan did not group the files: $PLAN_OUT"
fi

assert_exit 0 "cross-device copy: apply exits 0, moving the exFAT files onto OUTER" \
  -- "$SWEEP" apply "$OUTER" --only Photos --yes --depth 2

# The destination is $OUTER/Photos/, on the OUTER device -- not still inside
# aaa_innervol. Confirming that directly is part of proving this is a real
# cross-device move: if these files were still under aaa_innervol, apply
# would have done a same-device no-op, not the copy this scenario exists to
# check.
CONTENT_OK=1
MTIME_OK=1
STILL_ON_SOURCE_VOLUME=0
for n in 1 2 3; do
  f="$OUTER/Photos/IMG_104$n.jpg"
  if [ ! -f "$f" ]; then
    CONTENT_OK=0
    MTIME_OK=0
    continue
  fi
  [ "$(cat "$f")" = "photo $n" ] || CONTENT_OK=0
  [ "$(stat -f %m "$f")" = "$MTIME_BEFORE" ] || MTIME_OK=0
  [ -f "$INNER/IMG_104$n.jpg" ] && STILL_ON_SOURCE_VOLUME=1
done
if [ "$STILL_ON_SOURCE_VOLUME" = 1 ]; then
  fail "cross-device copy: a source file is still present on the exFAT volume after apply -- this did not exercise a real move"
fi
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
# matches the destination. Undo copies back onto the exFAT volume, so this
# also exercises copy_data_and_stat's EXDEV path a second time, in reverse.
assert_exit 0 "cross-device copy: undo succeeds, proving the mtime fingerprint still matched" \
  -- "$SWEEP" undo
RESTORED=0
for n in 1 2 3; do
  [ -f "$INNER/IMG_104$n.jpg" ] && RESTORED=$((RESTORED + 1))
done
assert_eq 3 "$RESTORED" "cross-device copy: undo restored every file to the exFAT volume"

# The AppleDouble check, against the exFAT volume undo just wrote back to.
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
  pass "cross-device copy: no AppleDouble sidecar appears in sweep's own plan of the exFAT volume"
fi

hdiutil detach "$INNER" -force >/dev/null 2>&1
INNER=""
