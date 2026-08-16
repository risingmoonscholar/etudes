#!/usr/bin/env bash
# A lowered RLIMIT_NOFILE against a large tree. The core scan/apply loop
# opens and closes one file at a time (see etude-core's fingerprint()), so it
# should tolerate an extremely low descriptor budget -- but the journal path
# also talks to the macOS keychain (a separate subsystem, over XPC, with its
# own descriptor needs), and that is the part actually worth stress-testing:
# if the keychain call fails under fd pressure, the tool must refuse to
# proceed rather than silently falling back to an unsealed journal or
# half-applying.
#
# bash's own `ulimit -n` cannot go arbitrarily low in-process (the shell
# needs descriptors too, and a value below what's already open is rejected),
# so each trial execs the binary directly from a fresh `bash -c`, minimising
# the shell's own descriptor footprint before the limit is applied.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

D=$(workdir)
# Kept under 1040 so no filename accidentally contains "1040" or "1099" --
# both are tax-document markers (etude-core/src/classify.rs) that would
# pull a file out of the "widget" group as a false personal-record match,
# which is correct sensitive-detector behaviour but would corrupt this
# scenario's move-count arithmetic.
N=1000
for i in $(seq 1 "$N"); do : > "$D/widget_$i.txt"; done
BEFORE=$(find "$D" -type f | wc -l | tr -d ' ')
assert_eq "$N" "$BEFORE" "rlimit: fixture has the expected file count"

# Find the lowest ulimit this shell can actually set and still exec a child
# with clean stdio redirection -- below this, failures are the shell's, not
# the tool's, and would misreport as the tool crashing.
LOWEST_WORKABLE=""
PROBE=$(mktemp "${TMPDIR:-/tmp}/etudes-rlimit-probe-XXXXXX")
for lim in 32 16 12 10 9 8 7 6 5 4; do
  # Must match the *shape* of the real trials below (a file redirect plus
  # exec), or a limit that can run `exec true` but not open a log file would
  # be misjudged as workable.
  if bash -c "ulimit -n $lim 2>/dev/null; exec true > '$PROBE' 2>&1" 2>/dev/null; then
    LOWEST_WORKABLE=$lim
  else
    break
  fi
done
rm -f "$PROBE"
if [ -z "$LOWEST_WORKABLE" ]; then
  unproven "rlimit: plan/apply degrade honestly under a low file-descriptor limit" \
    "could not get bash to exec a child process under any tested ulimit -n on this host"
  exit 0
fi

# --- plan under the lowest workable limit -----------------------------------
PLAN_LOG=$(mktemp "${TMPDIR:-/tmp}/etudes-rlimit-plan-XXXXXX")
bash -c "ulimit -n $LOWEST_WORKABLE 2>/dev/null; exec \"$SWEEP\" \"$D\" > \"$PLAN_LOG\" 2>&1" 2>/dev/null
PLAN_CODE=$?
PLAN_OUT=$(cat "$PLAN_LOG"); rm -f "$PLAN_LOG"

if [ "$PLAN_CODE" -ge 128 ]; then
  fail "rlimit: plan crashed under ulimit -n $LOWEST_WORKABLE (signal-death exit $PLAN_CODE): $PLAN_OUT"
else
  pass "rlimit: plan did not crash under ulimit -n $LOWEST_WORKABLE (exit $PLAN_CODE)"
fi

# --- apply (--no-journal) under the same limit: the pure move loop ---------
# --no-journal bypasses the keychain entirely, isolating the move loop's own
# descriptor discipline from the keychain subsystem's.
APPLY_NJ_LOG=$(mktemp "${TMPDIR:-/tmp}/etudes-rlimit-applynj-XXXXXX")
bash -c "ulimit -n $LOWEST_WORKABLE 2>/dev/null; exec \"$SWEEP\" apply \"$D\" --yes --no-journal > \"$APPLY_NJ_LOG\" 2>&1" 2>/dev/null
APPLY_NJ_CODE=$?
APPLY_NJ_OUT=$(cat "$APPLY_NJ_LOG"); rm -f "$APPLY_NJ_LOG"

if [ "$APPLY_NJ_CODE" -ge 128 ]; then
  fail "rlimit: apply --no-journal crashed under ulimit -n $LOWEST_WORKABLE (signal-death exit $APPLY_NJ_CODE): $APPLY_NJ_OUT"
else
  pass "rlimit: apply --no-journal did not crash under ulimit -n $LOWEST_WORKABLE (exit $APPLY_NJ_CODE)"
fi

MOVED=$(find "$D" -mindepth 2 -type f 2>/dev/null | wc -l | tr -d ' ')
REMAIN=$(find "$D" -maxdepth 1 -type f 2>/dev/null | wc -l | tr -d ' ')
TOTAL=$((MOVED + REMAIN))
assert_eq "$N" "$TOTAL" "rlimit: apply --no-journal under fd pressure lost no files (moved+remaining == original count)"

# Whatever happened (full success, or an honest mid-loop refusal), it must
# not be a half-truth: either everything not yet moved is still exactly
# where it started (verifiable), or the tool said so.
if [ "$MOVED" -gt 0 ] && [ "$MOVED" -lt "$N" ] && ! echo "$APPLY_NJ_OUT" | grep -qi "io error\|refus\|error"; then
  fail "rlimit: apply --no-journal moved only $MOVED of $N files but printed no error/refusal explaining why: $APPLY_NJ_OUT"
else
  pass "rlimit: apply --no-journal's outcome ($MOVED/$N moved) is consistent with its own reported message"
fi

# Move everything back to a flat layout regardless of outcome, for the next
# part of the scenario.
find "$D" -mindepth 2 -type f -exec mv {} "$D/" \; 2>/dev/null
find "$D" -mindepth 1 -type d -empty -delete 2>/dev/null
RESET_COUNT=$(find "$D" -maxdepth 1 -type f | wc -l | tr -d ' ')
assert_eq "$N" "$RESET_COUNT" "rlimit: fixture reset cleanly between trials"

# --- apply (with journal) at a limit low enough to strain the keychain -----
# Search downward from the lowest workable shell limit for the point where
# the keychain (a separate OS subsystem with its own fd needs) starts to
# fail. Either point reachable is fine to report on; the property under test
# is that whichever happens, it happens honestly.
KEYCHAIN_FAIL_LIM=""
KEYCHAIN_OK_LIM=""
for lim in $(seq "$LOWEST_WORKABLE" -1 4); do
  bash -c "ulimit -n $lim 2>/dev/null" >/dev/null 2>&1 || continue
  LOG=$(mktemp "${TMPDIR:-/tmp}/etudes-rlimit-kc-XXXXXX")
  bash -c "export ETUDE_STATE_DIR='$ETUDE_STATE_DIR'; ulimit -n $lim 2>/dev/null; exec \"$SWEEP\" apply \"$D\" --yes > \"$LOG\" 2>&1" 2>/dev/null
  code=$?
  out=$(cat "$LOG"); rm -f "$LOG"

  if [ "$code" -ge 128 ]; then
    fail "rlimit: apply (with journal) crashed under ulimit -n $lim (signal-death exit $code): $out"
    break
  fi

  moved=$(find "$D" -mindepth 2 -type f 2>/dev/null | wc -l | tr -d ' ')
  if [ "$moved" -gt 0 ]; then
    KEYCHAIN_OK_LIM=$lim
    # Undo to reset for the next iteration.
    bash -c "export ETUDE_STATE_DIR='$ETUDE_STATE_DIR'; \"$SWEEP\" undo" >/dev/null 2>&1
    find "$D" -mindepth 2 -type f -exec mv {} "$D/" \; 2>/dev/null
    find "$D" -mindepth 1 -type d -empty -delete 2>/dev/null
  else
    if [ "$code" = 0 ]; then
      fail "rlimit: apply (with journal) reported success (exit 0) under ulimit -n $lim but moved nothing: $out"
    else
      KEYCHAIN_FAIL_LIM=$lim
      KEYCHAIN_FAIL_OUT=$out
      break
    fi
  fi
done

RESET_COUNT2=$(find "$D" -maxdepth 1 -type f | wc -l | tr -d ' ')
assert_eq "$N" "$RESET_COUNT2" "rlimit: fixture intact after the keychain-limit search"

if [ -n "$KEYCHAIN_FAIL_LIM" ]; then
  pass "rlimit: found the fd limit where the keychain-backed journal path starts to fail (ulimit -n $KEYCHAIN_FAIL_LIM)"
  if echo "$KEYCHAIN_FAIL_OUT" | grep -qi "moved [1-9]"; then
    fail "rlimit: apply claimed files were moved despite failing to get a keychain key: $KEYCHAIN_FAIL_OUT"
  else
    pass "rlimit: apply moved nothing when the keychain-backed sealer could not be acquired"
  fi
  if echo "$KEYCHAIN_FAIL_OUT" | grep -qi "refus"; then
    pass "rlimit: apply's failure message is an honest refusal, not a bare crash trace"
  else
    fail "rlimit: apply failed without an honest refusal message: $KEYCHAIN_FAIL_OUT"
  fi
else
  unproven "rlimit: apply (with journal) fails honestly when the keychain cannot be reached under fd pressure" \
    "every fd limit from $LOWEST_WORKABLE down to 4 that this shell could set still let the keychain call succeed on this host"
fi

exit 0
