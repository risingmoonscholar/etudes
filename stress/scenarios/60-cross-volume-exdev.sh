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
LIVE_PID=""
WATCHER_PID=""
cleanup() {
  if [ -n "$LIVE_PID" ] && kill -0 "$LIVE_PID" 2>/dev/null; then
    kill -CONT "$LIVE_PID" 2>/dev/null || true
    kill -9 "$LIVE_PID" 2>/dev/null || true
    wait "$LIVE_PID" 2>/dev/null || true
  fi
  if [ -n "$WATCHER_PID" ]; then
    kill "$WATCHER_PID" 2>/dev/null || true
    wait "$WATCHER_PID" 2>/dev/null || true
  fi
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

if ! create_disk_image "$IMG" 768m APFS ExdevStress; then
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
INTERRUPT="$INNER/interrupt_zzzbig.txt"
DEST_PARTIAL="$OUTER/Documents/interrupt_zzzbig.txt"
printf 'selected-before-bulk-allocation\n' > "$INTERRUPT"
INTERRUPT_PLAN=$("$SWEEP" "$OUTER" --depth 2 --json 2>&1)
if python3 -c '
import json, sys
d = json.load(sys.stdin)["details"]
members = [m for g in d.get("groups", []) for m in g.get("members", [])]
sys.exit(0 if any(m.endswith("interrupt_zzzbig.txt") for m in members) else 1)
' <<<"$INTERRUPT_PLAN"; then
  pass "cross-volume: tiny interruption fixture is selected for the expected interrupt group before bulk allocation"
else
  fail "cross-volume: tiny interruption fixture was not selected; refusing to allocate a bulk copy fixture: $INTERRUPT_PLAN"
  exit 0
fi

# The same predicate is used by the real post-kill assertion and the negative
# control below, so the control exercises the scenario's actual oracle.
source_matches_snapshot() {
  local path=$1 expected_size=$2 expected_hash=$3 actual_size actual_hash
  [ -f "$path" ] || return 1
  actual_size=$(stat -f "%z" "$path" 2>/dev/null) || return 1
  [ "$actual_size" = "$expected_size" ] || return 1
  actual_hash=$(shasum -a 256 "$path" 2>/dev/null | awk '{print $1}') || return 1
  [ "$actual_hash" = "$expected_hash" ]
}

# Preload the watcher before apply starts: even Python startup can outlast a
# small cached copy. Only stat(2) and a 100us sleep separate size observations;
# SIGSTOP is sent directly, before launching ps to confirm the stopped state.
start_copy_probe() {
  local state_dir=$1 apply_log=$2
  rm -f "$W/copy-pid" "$W/watcher-ready" "$W/watcher-result"
  mkfifo "$W/copy-pid" || return 1
  python3 "$(dirname "${BASH_SOURCE[0]}")/../copy-window.py" \
    "$W/copy-pid" "$W/watcher-ready" "$DEST_PARTIAL" "$SRC_SIZE_BEFORE" \
    >"$W/watcher-result" &
  WATCHER_PID=$!
  for _ in $(seq 1 1000); do
    [ -f "$W/watcher-ready" ] && break
    kill -0 "$WATCHER_PID" 2>/dev/null || return 1
    sleep 0.001
  done
  [ -f "$W/watcher-ready" ] || return 1
  ETUDE_STATE_DIR="$state_dir" "$SWEEP" apply "$OUTER" --yes --depth 2 >"$apply_log" 2>&1 &
  PID=$!; LIVE_PID=$PID
  printf '%s\n' "$PID" > "$W/copy-pid"
}

pause_on_partial_copy() {
  PAUSE_CONFIRMED=0
  PAUSE_STATE=""
  if ! wait "$WATCHER_PID"; then
    WATCHER_PID=""
    fail "cross-volume: copy watcher failed; no interruption verdict is available"
    return 1
  fi
  WATCHER_PID=""
  read -r PAUSE_CONFIRMED PAUSE_STATE < "$W/watcher-result"
  tail -n +2 "$W/watcher-result"
  [ "$PAUSE_CONFIRMED" = 1 ]
}

# A completed move is a missed window, not a lost source. Resume/wait for it,
# preserve the completed-probe undo assertion, report this attempt unproven,
# and leave the fixture byte-identical for escalation.
restore_missed_copy() {
  local pid=$1 state_dir=$2 expected_size=$3 expected_hash=$4 apply_log=$5 payload_mib=$6 miss_reason=$7 dest_size state wait_status
  state=$(ps -p "$pid" -o state= 2>/dev/null | tr -d '[:space:]')
  case "$state" in T*) kill -CONT "$pid" 2>/dev/null || true ;; esac
  wait_status=0
  wait "$pid" 2>/dev/null || wait_status=$?
  LIVE_PID=""
  dest_size=0
  [ -f "$DEST_PARTIAL" ] && dest_size=$(stat -f "%z" "$DEST_PARTIAL" 2>/dev/null || echo 0)
  if [ "$wait_status" != 0 ]; then
    fail "cross-volume: apply exited $wait_status before a confirmed partial-copy kill; output: $(tail -n 12 "$apply_log" 2>/dev/null | tr '\n' ' ')"
    return 1
  fi
  if ! source_matches_snapshot "$INTERRUPT" "$expected_size" "$expected_hash" && [ "$dest_size" = "$expected_size" ]; then
    ETUDE_STATE_DIR="$state_dir" assert_exit 0 "cross-volume: completed ${payload_mib}MiB probe undoes before escalation" -- "$SWEEP" undo
    if source_matches_snapshot "$INTERRUPT" "$expected_size" "$expected_hash"; then
      rm -rf "$OUTER/Documents"
      unproven "cross-volume: SIGKILL during the ${payload_mib}MiB copy was not witnessed" "$miss_reason"
      return 0
    fi
  fi
  fail "cross-volume: apply exited 0 without a completed move after the pause window (source size=$(stat -f '%z' "$INTERRUPT" 2>/dev/null || echo MISSING), destination size=$dest_size); output: $(tail -n 12 "$apply_log" 2>/dev/null | tr '\n' ' ')"
  return 1
}

# Escalate only when the copy completes before a confirmed stop. The first
# stopped attempt is also a negative control: move the source path while the
# copy is frozen, kill it, and prove the same integrity predicate rejects it.
KILLED=0
CONTROL_PASSED=0
PAYLOAD_MIB=0
MISS_REASON="copies through 32MiB, 96MiB, 256MiB, and 512MiB completed before a stopped partial copy could be witnessed"
# One payload at a time, at most 512MiB on the 768MiB source image.
for mib in 32 96 256 512; do
  rm -f "$INTERRUPT" "$DEST_PARTIAL"
  dd if=/dev/urandom of="$INTERRUPT" bs=1m count="$mib" >/dev/null 2>&1
  SRC_SIZE_BEFORE=$(stat -f "%z" "$INTERRUPT")
  SRC_HASH_BEFORE=$(shasum -a 256 "$INTERRUPT" | awk '{print $1}')

  if [ "$CONTROL_PASSED" = 0 ]; then
    CONTROL_STATE="$W/control-state-$mib"
    mkdir -p "$CONTROL_STATE"
    start_copy_probe "$CONTROL_STATE" "$W/control-apply.log" || { fail "cross-volume: watcher could not start"; exit 0; }
    if pause_on_partial_copy; then
      CONTROL_DEST_SIZE=$(stat -f "%z" "$DEST_PARTIAL" 2>/dev/null || echo 0)
      if ! source_matches_snapshot "$INTERRUPT" "$SRC_SIZE_BEFORE" "$SRC_HASH_BEFORE"; then
        if [ "$CONTROL_DEST_SIZE" -gt 0 ] && [ "$CONTROL_DEST_SIZE" -lt "$SRC_SIZE_BEFORE" ]; then
          kill -9 "$PID" 2>/dev/null || true; wait "$PID" 2>/dev/null || true; LIVE_PID=""
          fail "REAL DEFECT: source changed or disappeared while a confirmed-stopped cross-volume copy still had a partial destination"
          exit 0
        fi
        MISS_REASON="the control copy finished before source and destination could be rechecked while stopped"
        restore_missed_copy "$PID" "$CONTROL_STATE" "$SRC_SIZE_BEFORE" "$SRC_HASH_BEFORE" "$W/control-apply.log" "$mib" "$MISS_REASON" || exit 0
        continue
      fi
      if [ "$CONTROL_DEST_SIZE" -le 0 ] || [ "$CONTROL_DEST_SIZE" -ge "$SRC_SIZE_BEFORE" ]; then
        MISS_REASON="the control copy completed before the stopped-state size recheck"
        restore_missed_copy "$PID" "$CONTROL_STATE" "$SRC_SIZE_BEFORE" "$SRC_HASH_BEFORE" "$W/control-apply.log" "$mib" "$MISS_REASON" || exit 0
        continue
      fi
      CONTROL_RESCUE="$INNER/interrupt-control-rescue.txt"
      if ! mv "$INTERRUPT" "$CONTROL_RESCUE"; then
        kill -CONT "$PID" 2>/dev/null || true
        wait "$PID" 2>/dev/null || true; LIVE_PID=""
        fail "cross-volume: negative control could not remove the source path while the copy was stopped"
        exit 0
      fi
      if ! kill -9 "$PID" 2>/dev/null; then
        fail "cross-volume: SIGKILL could not be delivered to confirmed-stopped negative-control process pid=$PID state=$PAUSE_STATE"
        kill -CONT "$PID" 2>/dev/null || true
        wait "$PID" 2>/dev/null || true; LIVE_PID=""
        mv "$CONTROL_RESCUE" "$INTERRUPT" || true
        exit 0
      fi
      CONTROL_WAIT=0
      wait "$PID" 2>/dev/null || CONTROL_WAIT=$?
      LIVE_PID=""
      if [ "$CONTROL_WAIT" != 137 ]; then
        fail "cross-volume: negative-control process did not exit from SIGKILL (pid=$PID wait=$CONTROL_WAIT)"
        mv "$CONTROL_RESCUE" "$INTERRUPT" || true
        exit 0
      fi
      if source_matches_snapshot "$INTERRUPT" "$SRC_SIZE_BEFORE" "$SRC_HASH_BEFORE"; then
        fail "cross-volume: negative control failed because the source-integrity predicate accepted a removed source"
      else
        pass "cross-volume: negative control rejects removed source after stopped-copy SIGKILL pid=$PID state=$PAUSE_STATE destination=$CONTROL_DEST_SIZE/$SRC_SIZE_BEFORE bytes wait=$CONTROL_WAIT"
        CONTROL_PASSED=1
      fi
      mv "$CONTROL_RESCUE" "$INTERRUPT" || { fail "cross-volume: negative control could not restore its saved source"; exit 0; }
      # The interrupted control may have moved earlier members of the same
      # group before reaching the large source. Replay its own journal, then
      # clear only this scenario's generated destination tree before the real
      # positive probe starts.
      ETUDE_STATE_DIR="$CONTROL_STATE" assert_exit 0 "cross-volume: negative-control cleanup restores earlier group members" -- "$SWEEP" undo
      if ! source_matches_snapshot "$INTERRUPT" "$SRC_SIZE_BEFORE" "$SRC_HASH_BEFORE"; then
        fail "cross-volume: negative-control cleanup did not restore the original byte-identical source"
        exit 0
      fi
      rm -rf "$OUTER/Documents"
    else
      MISS_REASON="control copy completed before the partial destination could be frozen"
      restore_missed_copy "$PID" "$CONTROL_STATE" "$SRC_SIZE_BEFORE" "$SRC_HASH_BEFORE" "$W/control-apply.log" "$mib" "$MISS_REASON" || exit 0
      continue
    fi
  fi

  POSITIVE_STATE="$W/positive-state-$mib"
  mkdir -p "$POSITIVE_STATE"
  POSITIVE_PLAN=$(ETUDE_STATE_DIR="$POSITIVE_STATE" "$SWEEP" "$OUTER" --depth 2 --json 2>&1)
  if ! python3 -c '
import json, sys
d=json.loads(sys.stdin.read())
members=[m for g in d.get("details", {}).get("groups", []) for m in g.get("members", [])]
sys.exit(0 if any(str(m).endswith("interrupt_zzzbig.txt") for m in members) else 1)
' <<<"$POSITIVE_PLAN"; then
    fail "cross-volume: positive interruption probe no longer selects the expected source after the control: $POSITIVE_PLAN"
    exit 0
  fi
  start_copy_probe "$POSITIVE_STATE" "$W/positive-apply.log" || { fail "cross-volume: watcher could not start"; exit 0; }
  if pause_on_partial_copy; then
    STOPPED_DEST_SIZE=$(stat -f "%z" "$DEST_PARTIAL" 2>/dev/null || echo 0)
    if ! source_matches_snapshot "$INTERRUPT" "$SRC_SIZE_BEFORE" "$SRC_HASH_BEFORE"; then
      if [ "$STOPPED_DEST_SIZE" -gt 0 ] && [ "$STOPPED_DEST_SIZE" -lt "$SRC_SIZE_BEFORE" ]; then
        kill -9 "$PID" 2>/dev/null || true; wait "$PID" 2>/dev/null || true; LIVE_PID=""
        fail "REAL DEFECT: source changed or disappeared while a confirmed-stopped cross-volume copy still had a partial destination"
        exit 0
      fi
      MISS_REASON="the copy completed before source and destination could be rechecked while stopped"
      restore_missed_copy "$PID" "$POSITIVE_STATE" "$SRC_SIZE_BEFORE" "$SRC_HASH_BEFORE" "$W/positive-apply.log" "$mib" "$MISS_REASON" || exit 0
      continue
    fi
    if [ "$STOPPED_DEST_SIZE" -gt 0 ] && [ "$STOPPED_DEST_SIZE" -lt "$SRC_SIZE_BEFORE" ]; then
      if ! kill -9 "$PID" 2>/dev/null; then
        fail "cross-volume: SIGKILL could not be delivered to confirmed-stopped copy pid=$PID state=$PAUSE_STATE"
        kill -CONT "$PID" 2>/dev/null || true
        wait "$PID" 2>/dev/null || true; LIVE_PID=""
        exit 0
      fi
      KILL_WAIT=0
      wait "$PID" 2>/dev/null || KILL_WAIT=$?
      LIVE_PID=""
      if [ "$KILL_WAIT" != 137 ]; then
        fail "cross-volume: confirmed-stopped copy did not exit from SIGKILL (pid=$PID wait=$KILL_WAIT)"
        exit 0
      fi
      KILLED=1
      PAYLOAD_MIB=$mib
      pass "cross-volume: SIGKILL delivered to confirmed-stopped copy pid=$PID state=$PAUSE_STATE destination=$STOPPED_DEST_SIZE/$SRC_SIZE_BEFORE bytes wait=$KILL_WAIT"
      break
    fi
    MISS_REASON="the destination completed before the stopped-state size recheck"
    restore_missed_copy "$PID" "$POSITIVE_STATE" "$SRC_SIZE_BEFORE" "$SRC_HASH_BEFORE" "$W/positive-apply.log" "$mib" "$MISS_REASON" || exit 0
    continue
  fi
  MISS_REASON="copy completed before a partial destination could be frozen"
  restore_missed_copy "$PID" "$POSITIVE_STATE" "$SRC_SIZE_BEFORE" "$SRC_HASH_BEFORE" "$W/positive-apply.log" "$mib" "$MISS_REASON" || exit 0
done

if [ "$KILLED" = 0 ]; then
  unproven "cross-volume: a real SIGKILL mid-copy leaves the source intact" \
    "$MISS_REASON"
else
  SRC_SIZE_AFTER=$(stat -f "%z" "$INTERRUPT" 2>/dev/null || echo "MISSING")
  SRC_HASH_AFTER=$(shasum -a 256 "$INTERRUPT" 2>/dev/null | awk '{print $1}')
  if source_matches_snapshot "$INTERRUPT" "$SRC_SIZE_BEFORE" "$SRC_HASH_BEFORE"; then
    pass "cross-volume: source is byte-identical after a witnessed SIGKILL during a ${PAYLOAD_MIB}MiB copy"
  else
    fail "REAL DEFECT: a SIGKILL mid cross-device-copy left the source file altered or gone \
(was $SRC_SIZE_BEFORE bytes, now $SRC_SIZE_AFTER) -- move_one's copy-verify-unlink ordering \
did not protect the source"
  fi

  # A retry must not silently clobber or silently "succeed" over the partial
  # artifact the kill left behind at the destination.
  DEST_SIZE_BEFORE_RETRY=$(stat -f "%z" "$DEST_PARTIAL" 2>/dev/null || echo MISSING)
  DEST_HASH_BEFORE_RETRY=$(shasum -a 256 "$DEST_PARTIAL" 2>/dev/null | awk '{print $1}')
  RETRY_OUT=$("$SWEEP" apply "$OUTER" --yes --depth 2 2>&1)
  RETRY_CODE=$?
  if [ "$RETRY_CODE" = 0 ]; then
    fail "cross-volume: retrying apply after an interrupted copy reported success (exit 0) without checking for the stray partial destination file: $RETRY_OUT"
  else
    pass "cross-volume: retrying apply after an interrupted copy refused rather than silently overwriting/succeeding (exit $RETRY_CODE)"
  fi
  DEST_SIZE_AFTER_RETRY=$(stat -f "%z" "$DEST_PARTIAL" 2>/dev/null || echo MISSING)
  DEST_HASH_AFTER_RETRY=$(shasum -a 256 "$DEST_PARTIAL" 2>/dev/null | awk '{print $1}')
  assert_eq "$DEST_SIZE_BEFORE_RETRY" "$DEST_SIZE_AFTER_RETRY" "cross-volume: refused retry leaves the partial destination size unchanged"
  assert_eq "$DEST_HASH_BEFORE_RETRY" "$DEST_HASH_AFTER_RETRY" "cross-volume: refused retry leaves the partial destination byte-identical"
  SRC_HASH_FINAL=$(shasum -a 256 "$INTERRUPT" 2>/dev/null | awk '{print $1}')
  assert_eq "$SRC_HASH_BEFORE" "$SRC_HASH_FINAL" "cross-volume: source is still byte-identical after the refused retry"
fi
rm -f /tmp/etudes-exdev-kill.log

exit 0
