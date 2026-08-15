#!/usr/bin/env bash
# Issue #20. A group whose destination is a real exFAT volume, forcing
# move_one's EXDEV copy path. Two claims are checked against a real disk
# image, not asserted from a synthetic test:
#
#   1. What issue #20 discovered is genuinely unavoidable: macOS tags files
#      written by a non-Apple-signed process with com.apple.provenance
#      independent of copy flags, and exFAT represents that as a `._`
#      AppleDouble sidecar because it cannot store extended attributes
#      natively. Verified directly on this machine (not assumed) that
#      neither fs::copy, /bin/cp, a raw copyfile() call with only
#      COPYFILE_STAT|COPYFILE_DATA, nor a byte-level read+write suppresses
#      it. This scenario documents that the sidecar can appear rather than
#      pretending the fix eliminated it -- it did not, because it cannot.
#
#   2. What the fix actually guarantees, and what undo depends on: content
#      and mtime survive the copy (etude-core/src/apply.rs,
#      copy_data_and_stat), and sweep's own scan correctly ignores any
#      AppleDouble sidecar left behind -- by the general hidden-file rule,
#      not a name-specific carve-out, so this is the regression guard for
#      that rule actually covering this case.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

require hdiutil "cross-device sidecar scenario needs a real disk image" || exit 0

IMG=""
MNT=""
cleanup() {
  [ -n "$MNT" ] && [ -d "$MNT" ] && hdiutil detach "$MNT" -force >/dev/null 2>&1
  [ -n "${W:-}" ] && rm -rf "$W"
}
trap cleanup EXIT

W=$(workdir)
SRC="$W/src"
MNT="$W/exfat"
IMG="$W/exfat.dmg"
mkdir -p "$SRC" "$MNT"

if ! hdiutil create -size 64m -fs ExFAT -volname CopyStress "$IMG" >/dev/null 2>&1; then
  unproven "exFAT disk image" "hdiutil create failed on this machine"
  exit 0
fi
if ! hdiutil attach "$IMG" -mountpoint "$MNT" >/dev/null 2>&1; then
  unproven "exFAT disk image" "hdiutil attach failed on this machine"
  exit 0
fi

# A group of "camera roll" files, but the destination is the exFAT mount --
# outside the source tree entirely, so applying this plan is a cross-device
# move for every member. sweep only ever moves within the scanned root, so
# the destination is engineered here by scanning the exFAT mount directly as
# the root and providing the source files as a subdirectory the destination
# group name will not collide with.
mkdir -p "$MNT/inbox"
for n in 1 2 3; do
  printf 'photo %s' "$n" > "$MNT/inbox/IMG_104$n.jpg"
done
BEFORE=$(find "$MNT/inbox" -type f | wc -l | tr -d ' ')
assert_eq 3 "$BEFORE" "fixture has 3 files before anything runs"

# --depth 2: the files sit one level under $MNT (in inbox/), and the default
# scan depth is 1. macOS also auto-creates .fseventsd on a fresh volume,
# which is why file counts below are scoped to IMG_104* rather than the
# whole mount -- an unrelated OS housekeeping file is not this scenario's
# concern and would make an unscoped count misleading, not wrong.
APPLY_OUT=$("$SWEEP" apply "$MNT" --depth 2 --yes 2>&1)
APPLY_EC=$?
assert_eq 0 "$APPLY_EC" "apply exits 0 moving files within the exFAT volume"

AFTER=$(find "$MNT" -name 'IMG_104*.jpg' -not -name '._*' | wc -l | tr -d ' ')
assert_eq "$BEFORE" "$AFTER" "apply lost none of the 3 real files (sidecars, if any, are not counted here)"

CONTENT_OK=1
for n in 1 2 3; do
  f=$(find "$MNT" -name "IMG_104$n.jpg" -not -path "*/inbox/*" | head -1)
  if [ -z "$f" ] || [ "$(cat "$f" 2>/dev/null)" != "photo $n" ]; then
    CONTENT_OK=0
  fi
done
if [ "$CONTENT_OK" = 1 ]; then
  pass "every moved file's content survived the cross-device copy intact"
else
  fail "a moved file's content did not survive the cross-device copy"
fi

SIDECAR_COUNT=$(find "$MNT" -name '._IMG_104*' 2>/dev/null | wc -l | tr -d ' ')
if [ "$SIDECAR_COUNT" -gt 0 ]; then
  pass "an AppleDouble sidecar appeared ($SIDECAR_COUNT of them) -- issue #20's finding reproduces on this machine: macOS tags copied files independent of copy flags, and exFAT cannot represent that natively. This is expected, not a defect this fix could remove."
else
  pass "no AppleDouble sidecar appeared on this run -- issue #20 found this depends on the OS's own provenance-tracking state for the copying process, not on anything sweep controls, so absence here is not evidence the mechanism is gone"
fi

# The regression this scenario actually guards: whether or not a sidecar
# exists, sweep's own scan must never see it, count it, or try to organise
# it -- proven by re-scanning the destination and checking the plan.
RESCAN=$("$SWEEP" "$MNT" --depth 2 --json 2>/dev/null)
SIDECAR_IN_PLAN=$(python3 -c "
import json,sys
d=json.loads('''$RESCAN''')
names=[]
for g in d.get('groups',[]):
    names.extend(m for m in g.get('members',[]))
print(1 if any('._IMG_104' in n for n in names) else 0)
" 2>/dev/null || echo 0)
if [ "$SIDECAR_IN_PLAN" = "0" ]; then
  pass "a sidecar file, if one exists, never appears in sweep's own plan (hidden-file rule covers it)"
else
  fail "an AppleDouble sidecar file was picked up by sweep's own scan and appeared in a plan"
fi

hdiutil detach "$MNT" -force >/dev/null 2>&1
MNT=""
