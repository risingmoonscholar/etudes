#!/usr/bin/env bash
# A read-only volume. `plan` only reads metadata, so it must work exactly as
# on a writable volume. `apply` must refuse cleanly -- not crash, not claim
# success, not leave anything half-moved (there is nowhere for a half-move to
# happen: the OS refuses the very first write).
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

require hdiutil "read-only-volume scenario needs a real disk image" || exit 0

IMG=""
MNT=""
cleanup() {
  detach_registered_mounts
  [ -n "${W:-}" ] && rm -rf "$W"
}
trap cleanup EXIT

W=$(workdir)
IMG="$W/ro.dmg"
MNT="$W/mnt"
mkdir -p "$MNT"

if ! hdiutil create -size 20m -fs "APFS" -volname ROVolStress "$IMG" >/dev/null 2>&1; then
  unproven "read-only volume: apply refuses cleanly" "hdiutil create failed on this host"
  exit 0
fi
if ! hdiutil attach "$IMG" -mountpoint "$MNT" -nobrowse >/dev/null 2>&1; then
  unproven "read-only volume: apply refuses cleanly" "hdiutil attach failed on this host"
  exit 0
fi
register_mount "$MNT"

for f in alpha beta gamma delta epsilon; do : > "$MNT/widget_$f.txt"; done
BEFORE=$(find "$MNT" -maxdepth 1 -type f | wc -l | tr -d ' ')

# Flip to read-only: detach, reattach with -readonly. register_mount was
# called once, after the first attach, for this same path -- the reattach
# here does not need a second call. At exit, detach_registered_mounts tries
# that one registered path once, which by then is the read-only mount.
hdiutil detach "$MNT" -force >/dev/null 2>&1
if ! hdiutil attach "$IMG" -mountpoint "$MNT" -nobrowse -readonly >/dev/null 2>&1; then
  unproven "read-only volume: apply refuses cleanly" "could not reattach the image read-only on this host"
  exit 0
fi

# plan must work identically on a read-only volume -- it never writes.
PLAN_OUT=$("$SWEEP" "$MNT" 2>&1)
PLAN_CODE=$?
assert_eq 0 "$PLAN_CODE" "read-only volume: plan succeeds (it only reads)"
if echo "$PLAN_OUT" | grep -q "widget"; then
  pass "read-only volume: plan still finds the group"
else
  fail "read-only volume: plan did not find the expected group on a read-only volume: $PLAN_OUT"
fi

# apply must refuse cleanly: no crash (no signal-death exit code >= 128),
# no exit 0, and nothing moved.
APPLY_OUT=$("$SWEEP" apply "$MNT" --yes 2>&1)
APPLY_CODE=$?

if [ "$APPLY_CODE" -ge 128 ]; then
  fail "read-only volume: apply crashed (signal-death exit code $APPLY_CODE): $APPLY_OUT"
else
  pass "read-only volume: apply did not crash (exit $APPLY_CODE)"
fi

if [ "$APPLY_CODE" = 0 ]; then
  fail "read-only volume: apply reported success (exit 0) on a read-only volume"
else
  pass "read-only volume: apply did not report success"
fi

if echo "$APPLY_OUT" | grep -qi "moved [1-9]"; then
  fail "read-only volume: apply's output claims files were moved: $APPLY_OUT"
else
  pass "read-only volume: apply's output makes no claim of files moved"
fi

AFTER=$(find "$MNT" -maxdepth 1 -type f | wc -l | tr -d ' ')
assert_eq "$BEFORE" "$AFTER" "read-only volume: no file vanished or was left half-moved"

exit 0
