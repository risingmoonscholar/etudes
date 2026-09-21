#!/usr/bin/env bash
# A group whose members span two volumes: a real cross-device move. Nothing
# in etude-core checks mount boundaries while walking (see scan.rs's walk(),
# which recurses through fs::read_dir with no st_dev check), so scanning at
# --depth 2 over a tree with a disk image mounted as a subdirectory will pull
# in files that physically live on a different device than the group's
# destination -- forcing rename(2)'s EXDEV path: copy, verify the copy landed
# intact, then unlink the source (etude-core/src/apply.rs's move_one()).
#
# Two things are checked:
#   1. The ordinary case: apply moves everything correctly (verified by
#      content, not just presence), and undo puts every file back on its
#      original volume.
#   2. A real interruption: SIGKILL sent partway through a large cross-device
#      copy. The source must never be lost -- move_one only unlinks the
#      source after fs::copy + a size check succeed, so a kill mid-copy must
#      leave the source fully intact and a retry must refuse rather than
#      silently overwriting or silently succeeding.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

require hdiutil "cross-volume scenario needs a real disk image" || exit 0

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
# Named so its path sorts before plain filenames ('a' < the group's token
# letters) -- scan orders entries by full path, so this keeps the two halves
# of the group in a predictable relative order for the tests below.
INNER="$OUTER/aaa_innervol"
mkdir -p "$OUTER" "$INNER"

if ! hdiutil create -size 768m -fs "APFS" -volname ExdevStress "$IMG" >/dev/null 2>&1; then
  unproven "cross-volume: EXDEV move is correct and interruption-safe" "hdiutil create failed on this host"
  exit 0
fi
if ! hdiutil attach "$IMG" -mountpoint "$INNER" -nobrowse >/dev/null 2>&1; then
  unproven "cross-volume: EXDEV move is correct and interruption-safe" "hdiutil attach failed on this host"
  exit 0
fi
register_mount "$INNER"

DEV_OUTER=$(stat -f "%d" "$OUTER")
DEV_INNER=$(stat -f "%d" "$INNER")
if [ "$DEV_OUTER" = "$DEV_INNER" ]; then
  unproven "cross-volume: EXDEV move is correct and interruption-safe" \
    "the nested mount ended up on the same device id as the outer tree ($DEV_OUTER) on this host"
  exit 0
fi
pass "cross-volume: outer tree and nested mount are confirmed on different devices ($DEV_OUTER vs $DEV_INNER)"

# --- Part 1: ordinary correctness, content-verified -------------------------
for f in bravo charlie; do echo "outer-$f-content" > "$OUTER/cargo_$f.txt"; done
for f in alpha delta; do echo "inner-$f-content" > "$INNER/cargo_$f.txt"; done
echo "inner-echo-content" > "$INNER/cargo_echo.txt"

PLAN_OUT=$("$SWEEP" "$OUTER" --depth 2 2>&1)
if echo "$PLAN_OUT" | grep -q "Documents"; then
  pass "cross-volume: plan groups files from both volumes under one shared token"
else
  fail "cross-volume: plan did not group the cross-device files: $PLAN_OUT"
fi

assert_exit 0 "cross-volume: apply succeeds across the EXDEV boundary" -- "$SWEEP" apply "$OUTER" --yes --depth 2

CONTENT_OK=1
for pair in "cargo_bravo.txt:outer-bravo-content" "cargo_charlie.txt:outer-charlie-content" \
            "cargo_alpha.txt:inner-alpha-content" "cargo_delta.txt:inner-delta-content" \
            "cargo_echo.txt:inner-echo-content"; do
  name="${pair%%:*}"; want="${pair##*:}"
  got=$(cat "$OUTER/Documents/$name" 2>/dev/null || echo "<missing>")
  [ "$got" = "$want" ] || CONTENT_OK=0
done
if [ "$CONTENT_OK" = 1 ]; then
  pass "cross-volume: every file's content survived the cross-device copy intact"
else
  fail "cross-volume: content was corrupted or lost by the cross-device copy"
fi

assert_exit 0 "cross-volume: undo succeeds" -- "$SWEEP" undo
UNDO_OK=1
[ -f "$OUTER/cargo_bravo.txt" ] || UNDO_OK=0
[ -f "$OUTER/cargo_charlie.txt" ] || UNDO_OK=0
[ -f "$INNER/cargo_alpha.txt" ] || UNDO_OK=0
[ -f "$INNER/cargo_delta.txt" ] || UNDO_OK=0
[ -f "$INNER/cargo_echo.txt" ] || UNDO_OK=0
if [ "$UNDO_OK" = 1 ]; then
  pass "cross-volume: undo restored every file to its original volume"
else
  fail "cross-volume: undo did not restore all files to their original volumes"
fi

# --- Part 2: real interruption mid-copy -------------------------------------
rm -rf "$OUTER/cargo" 2>/dev/null
for f in bravo charlie delta echo; do : > "$OUTER/interrupt_$f.txt"; done
# 'z' sorts after the group's other members, and files inside aaa_innervol/
# sort before plain top-level filenames -- so this file is scanned FIRST.
# Prove that selection on a tiny payload before allocating anything expensive.
INTERRUPT="$INNER/interrupt_zzzbig.bin"
DEST_PARTIAL="$OUTER/interrupt/interrupt_zzzbig.bin"
printf 'selected-before-bulk-allocation\n' > "$INTERRUPT"
INTERRUPT_PLAN=$("$SWEEP" "$OUTER" --depth 2 2>&1)
if grep -Fq "interrupt_zzzbig.bin" <<<"$INTERRUPT_PLAN" && grep -Fq "interrupt" <<<"$INTERRUPT_PLAN"; then
  pass "cross-volume: tiny interruption fixture is selected for the expected interrupt group before bulk allocation"
else
  fail "cross-volume: tiny interruption fixture was not selected for its expected group; refusing to allocate a bulk copy fixture: $INTERRUPT_PLAN"
  exit 0
fi

# Escalate only until this host actually exposes an incomplete destination.
# A completed attempt is undone before the next size, so every repetition
# starts from the same source path. This replaces a fixed 1.5 GiB allocation
# that was often far larger than the copy window needed for the observation.
KILLED=0
PAYLOAD_MIB=0
for mib in 32 96 256; do
  rm -f "$INTERRUPT" "$DEST_PARTIAL"
  dd if=/dev/urandom of="$INTERRUPT" bs=1m count="$mib" >/dev/null 2>&1
  SRC_SIZE_BEFORE=$(stat -f "%z" "$INTERRUPT")
  SRC_HASH_BEFORE=$(shasum -a 256 "$INTERRUPT" | awk '{print $1}')

  "$SWEEP" apply "$OUTER" --yes --depth 2 >/tmp/etudes-exdev-kill.log 2>&1 &
  PID=$!
  # Poll only for the independent witness: a destination that is nonempty but
  # still shorter than its known source. Never infer that from the exit code.
  for _ in $(seq 1 300); do
    kill -0 "$PID" 2>/dev/null || break
    if [ -f "$DEST_PARTIAL" ]; then
      partial_size=$(stat -f "%z" "$DEST_PARTIAL" 2>/dev/null || echo 0)
      if [ "$partial_size" -gt 0 ] && [ "$partial_size" -lt "$SRC_SIZE_BEFORE" ]; then
        kill -9 "$PID" 2>/dev/null
        KILLED=1
        PAYLOAD_MIB=$mib
        break
      fi
    fi
    sleep 0.01
  done
  wait "$PID" 2>/dev/null
  [ "$KILLED" = 1 ] && break

  # The copy finished before the bounded observation window. Restore it and
  # try the next measured size; failure to restore is a scenario failure.
  if [ ! -f "$INTERRUPT" ] && [ -f "$DEST_PARTIAL" ]; then
    assert_exit 0 "cross-volume: completed ${mib}MiB probe undoes before escalation" -- "$SWEEP" undo
  else
    fail "cross-volume: ${mib}MiB probe ended without either a source or a completed destination"
    exit 0
  fi
done

if [ "$KILLED" = 0 ]; then
  unproven "cross-volume: a real SIGKILL mid-copy leaves the source intact" \
    "copies through 32MiB, 96MiB, and 256MiB completed before an incomplete destination could be witnessed on this host"
else
  SRC_SIZE_AFTER=$(stat -f "%z" "$INTERRUPT" 2>/dev/null || echo "MISSING")
  SRC_HASH_AFTER=$(shasum -a 256 "$INTERRUPT" 2>/dev/null | awk '{print $1}')
  if [ "$SRC_SIZE_AFTER" = "$SRC_SIZE_BEFORE" ] && [ "$SRC_HASH_AFTER" = "$SRC_HASH_BEFORE" ]; then
    pass "cross-volume: source is byte-identical after a witnessed SIGKILL during a ${PAYLOAD_MIB}MiB copy"
  else
    fail "REAL DEFECT: a SIGKILL mid cross-device-copy left the source file altered or gone \
(was $SRC_SIZE_BEFORE bytes, now $SRC_SIZE_AFTER) -- move_one's copy-verify-unlink ordering \
did not protect the source"
  fi

  # A retry must not silently clobber or silently "succeed" over the partial
  # artifact the kill left behind at the destination.
  RETRY_OUT=$("$SWEEP" apply "$OUTER" --yes --depth 2 2>&1)
  RETRY_CODE=$?
  if [ "$RETRY_CODE" = 0 ]; then
    fail "cross-volume: retrying apply after an interrupted copy reported success (exit 0) without checking for the stray partial destination file: $RETRY_OUT"
  else
    pass "cross-volume: retrying apply after an interrupted copy refused rather than silently overwriting/succeeding (exit $RETRY_CODE)"
  fi
  SRC_HASH_FINAL=$(shasum -a 256 "$INTERRUPT" 2>/dev/null | awk '{print $1}')
  assert_eq "$SRC_HASH_BEFORE" "$SRC_HASH_FINAL" "cross-volume: source is still byte-identical after the refused retry"
fi
rm -f /tmp/etudes-exdev-kill.log

exit 0
