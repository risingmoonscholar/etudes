#!/usr/bin/env bash
# Issue #21. A same-device move on a real exFAT volume: renamex_np fails
# ENOTSUP (exFAT does not support it), move_one falls back to
# move_one_link_unlink, whose hard_link attempt ALSO fails -- exFAT has no
# hard-link concept at all, verified directly with `ln` against a real
# image -- with ENOTSUP, not EXDEV. Before the fix, only EXDEV was caught
# there, so this exact shape returned a hard error and apply failed
# outright, not sidecar junk.
#
# This scenario never crosses a device boundary -- source and destination
# are both on the one exFAT mount -- so it does NOT exercise move_one's
# EXDEV-to-copy_data_and_stat path. That is issue #20's shape, tested
# separately in 60-cross-device-copy-and-mtime.sh, which puts the source
# and destination on genuinely different volumes the way
# 60-cross-volume-exdev.sh already does for APFS.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

require hdiutil "same-device exFAT scenario needs a real disk image" || exit 0

IMG=""
MNT=""
cleanup() {
  detach_registered_mounts
  [ -n "${W:-}" ] && rm -rf "$W"
}
trap cleanup EXIT

W=$(workdir)
MNT="$W/exfat"
IMG="$W/exfat.dmg"
mkdir -p "$MNT"

if ! hdiutil create -size 64m -fs ExFAT -volname RenameFallback "$IMG" >/dev/null 2>&1; then
  unproven "exFAT disk image" "hdiutil create failed on this machine"
  exit 0
fi
if ! hdiutil attach "$IMG" -mountpoint "$MNT" >/dev/null 2>&1; then
  unproven "exFAT disk image" "hdiutil attach failed on this machine"
  exit 0
fi
register_mount "$MNT"

mkdir -p "$MNT/inbox"
for n in 1 2 3; do
  printf 'photo %s' "$n" > "$MNT/inbox/IMG_104$n.jpg"
done
BEFORE=$(find "$MNT/inbox" -type f | wc -l | tr -d ' ')
assert_eq 3 "$BEFORE" "fixture has 3 files before anything runs"

APPLY_OUT=$("$SWEEP" apply "$MNT" --depth 2 --yes 2>&1)
APPLY_EC=$?
assert_eq 0 "$APPLY_EC" "same-device apply on exFAT exits 0 (was a hard error before #21's fix)"

AFTER=$(find "$MNT" -name 'IMG_104*.jpg' | wc -l | tr -d ' ')
assert_eq "$BEFORE" "$AFTER" "apply lost none of the 3 real files"

CONTENT_OK=1
for n in 1 2 3; do
  f=$(find "$MNT" -name "IMG_104$n.jpg" -not -path "*/inbox/*" | head -1)
  if [ -z "$f" ] || [ "$(cat "$f" 2>/dev/null)" != "photo $n" ]; then
    CONTENT_OK=0
  fi
done
if [ "$CONTENT_OK" = 1 ]; then
  pass "every moved file's content survived the same-device fallback"
else
  fail "a moved file's content did not survive the same-device fallback"
fi

hdiutil detach "$MNT" -force >/dev/null 2>&1
MNT=""
