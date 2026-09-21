#!/usr/bin/env bash
# Shared harness for the stress scenarios.
#
# Three outcomes, never two. A scenario that cannot run on this host reports
# UNPROVEN and is counted separately. It must never be able to look like a
# pass. A harness that quietly skips is worse than no harness, because a green
# summary then means "nothing was checked" and reads identically to "everything
# was checked".
#
# No real file of yours is ever read. Every scenario builds its own tree under
# its own temporary directory and removes it on exit.

set -uo pipefail

# A scenario that dies before its first assertion used to be invisible: no ok,
# no FAIL, and the runner counted it as nothing at all. Worse, without `set -e`
# execution carried on past the error and a later assertion could report green,
# so a broken scenario looked like a passing one.
#
# This is the same defect the suite exists to find, wearing the harness as a
# costume. The trap makes a scenario say so when it dies, and run.sh treats a
# scenario that produced no assertions as a failure rather than as silence.
trap 'code=$?; if [ "$code" != "0" ]; then echo "    FAIL     scenario exited $code before finishing. It did not run to completion"; fi' EXIT
#
# The ERR trap is deliberately narrow. A first attempt fired on ANY non-zero
# command and produced 1250 false failures in one run, because scenarios run
# failing commands on purpose. That is how you test a refusal. A trap that
# cannot tell "failed as designed" from "is broken" is noise, and noise is
# how a real signal gets ignored.
#
# 127 is `command not found`: a typo or a missing tool, never a deliberate
# outcome. That is the shape of a scenario that is broken rather than failing.
set -E
trap 'c=$?; if [ "$c" = 127 ]; then fail "command not found at line $LINENO. The scenario is broken, not the tool"; fi' ERR

PASSED=0; FAILED=0; UNPROVEN=0
FAIL_LINES=(); UNPROVEN_LINES=()

# The binaries under test. Built by run.sh; never resolved from PATH, so a
# stale copy elsewhere cannot be tested by accident.
BIN="${BIN:?BIN must point at target/release}"
SWEEP="$BIN/sweep"; STASH="$BIN/stash"; UNPACK="$BIN/unpack"; MKFX="$BIN/mkfx"

# Most scenarios are about filesystem behaviour, not whether this machine's
# login keychain is unlocked. Exercise real sealed journals with one ephemeral
# supplied key by default. The dedicated journal/keychain scenario explicitly
# unsets or replaces this value for its own controls.
if [ -z "${ETUDE_JOURNAL_KEY:-}" ]; then
  export ETUDE_JOURNAL_KEY="$(python3 -c 'import secrets; print(secrets.token_hex(32))')"
fi

_stress_record() {
  # FD 199 is opened and immediately unlinked by the direct-run wrapper.
  # A scenario sees only the descriptor, never a replaceable record pathname.
  if [ -n "${STRESS_WRAP_DEPTH:-}" ]; then
    printf '%s\n' "$1" 2>/dev/null >&199 || true
  fi
}
pass()     { PASSED=$((PASSED+1)); _stress_record "ok"; printf '    ok       %s\n' "$1"; }
fail()     { FAILED=$((FAILED+1)); _stress_record "FAIL"; [ -n "${STRESS_WRAPPER_PID:-}" ] && kill -USR1 "$STRESS_WRAPPER_PID" 2>/dev/null || true; FAIL_LINES+=("$SCENARIO: $1"); printf '    FAIL     %s\n' "$1"; }
unproven() { UNPROVEN=$((UNPROVEN+1)); UNPROVEN_LINES+=("$SCENARIO: $1 ($2)"); _stress_record "unproven"; printf '    unproven %s (%s)\n' "$1" "$2"; }

# assert_eq EXPECTED ACTUAL LABEL
assert_eq() {
  if [ "$1" = "$2" ]; then pass "$3"; else fail "$3: expected '$1', got '$2'"; fi
}

# assert_exit WANT LABEL -- CMD...
assert_exit() {
  local want="$1" label="$2"; shift 3
  local out; out=$("$@" 2>&1); local got=$?
  if [ "$got" = "$want" ]; then pass "$label (exit $got)"
  else fail "$label: wanted exit $want, got $got (${out%%$'\n'*})"; fi
}

# assert_intact DIR N LABEL: nothing was destroyed. Count directory entries,
# never printed paths: a filename containing a newline is one file, not two.
assert_intact() {
  local n; n=$(python3 - "$1" <<'PY'
import os
import stat
import sys

count = 0
for root, dirs, files in os.walk(sys.argv[1], followlinks=False):
    for name in files:
        try:
            if stat.S_ISREG(os.lstat(os.path.join(root, name)).st_mode):
                count += 1
        except FileNotFoundError:
            pass
print(count)
PY
)
  if [ "$n" = "$2" ]; then pass "$3 ($n files intact)"
  else fail "$3: expected $2 files, found $n. FILES WERE LOST"; fi
}

# snapshot_tree DIR OUT: capture names, kinds, bytes and link targets without
# following links.  This is the default oracle for movement and refusal cases;
# a file count alone cannot distinguish a missing file from a duplicate.
snapshot_tree() {
  python3 "$(dirname "${BASH_SOURCE[0]}")/snapshot.py" "$1" > "$2"
  # A scenario normally removes its scratch tree on exit. Preserve the actual
  # manifests that informed a failed verdict before that cleanup can erase the
  # only useful explanation. The runner removes this directory for passes.
  if [ -n "${STRESS_FAILURE_ARTIFACTS:-}" ]; then
    SNAPSHOT_SEQUENCE=$(( ${SNAPSHOT_SEQUENCE:-0} + 1 ))
    mkdir -p "$STRESS_FAILURE_ARTIFACTS"
    cp "$2" "$STRESS_FAILURE_ARTIFACTS/snapshot-${SNAPSHOT_SEQUENCE}-$(basename "$2")"
  fi
}

# run_bounded MILLISECONDS EVIDENCE -- COMMAND ...: execute one command in an
# owned process group. The JSON evidence always records elapsed time, output,
# exit/signal and whether the deadline killed the group. Exit 124 means the
# deadline fired; callers must record that as failure or unproven explicitly.
run_bounded() {
  local timeout_ms="$1" evidence="$2"
  shift 2
  python3 "$(dirname "${BASH_SOURCE[0]}")/bounded.py" --timeout-ms "$timeout_ms" --evidence "$evidence" "$@"
}

# set_mtime_relative SECONDS PATH...: use the current clock rather than a
# calendar date in a fixture. Positive seconds mean the past; negative values
# mean the future. Python avoids platform-specific touch date syntax.
set_mtime_relative() {
  local seconds="$1"
  shift
  python3 - "$seconds" "$@" <<'PY'
import os
import sys
import time

when = time.time() - int(sys.argv[1])
for path in sys.argv[2:]:
    os.utime(path, (when, when))
PY
}

# assert_snapshot_eq BEFORE AFTER LABEL
assert_snapshot_eq() {
  if cmp -s "$1" "$2"; then
    pass "$3"
  else
    fail "$3: filesystem manifest changed (before=$1 after=$2)"
  fi
}

# assert_snapshot_ne BEFORE AFTER LABEL
assert_snapshot_ne() {
  if cmp -s "$1" "$2"; then
    fail "$3: filesystem manifest did not change"
  else
    pass "$3"
  fi
}

# Journals go to a scratch state directory, never the real one.
#
# ETUDE_STATE_DIR redirects where journals are written while leaving HOME
# alone, so the keychain still works. Without this, a scenario would write
# into the user's own journal directory. `undo` picks the newest journal
# globally. Two scenarios could undo each other's work. A stress run would
# leave real state behind.
#
# Was XDG_STATE_HOME before issue #23 moved the product default off that
# Linux convention onto ~/Library/Application Support/etudes. ETUDE_STATE_DIR
# was already the primary override the rest of the suite uses; this just
# stops the harness being the one caller still depending on the removed
# fallback. Unlike the old XDG_STATE_HOME (which had "etudes" joined onto it
# by state_dir()), ETUDE_STATE_DIR is used exactly as given -- no /etudes
# suffix needed when a scenario builds a path under it.
# Every scenario builds a tree and sweeps it in the same second, so all of
# them sit inside sweep's default 24h grace window. None of them is about that
# window -- they are about collisions, interruptions, volumes and signals -- so
# the harness turns it off globally rather than making 35 scenarios remember a
# flag. A scenario that wants to TEST the window sets its own value.
# Unconditional, not ${VAR:-0}: a caller with the variable already exported
# would otherwise silently run the whole suite under a live grace window, and
# most scenarios would fail or go unproven for a reason nothing explains. A
# scenario that wants the window sets it per-invocation, as 25-grace-window
# does.
export SWEEP_GRACE_SECS=0

export ETUDE_STATE_DIR="${ETUDE_STATE_DIR_OVERRIDE:-$(mktemp -d "${TMPDIR:-/tmp}/etudes-stress-state-XXXXXX")}"
# Bind the chosen directory now: scenarios may later change the override.
printf -v _stress_state_cleanup 'rm -rf -- %q' "$ETUDE_STATE_DIR"
trap "$_stress_state_cleanup" EXIT

# A scratch tree, unique per scenario, removed on exit.
workdir() {
  local d; d=$(mktemp -d "${TMPDIR:-/tmp}/etudes-stress-${SCENARIO}-XXXXXX")
  echo "$d"
}

# Registers a mounted volume for cleanup, so every scenario that mounts one
# gets the ordering right by construction instead of each getting its own
# trap to write correctly. Issue #13.
#
# Nothing here defends against SIGKILL. A trap -- this one included -- never
# runs at all for a process killed with -9; that is a POSIX guarantee, not a
# bug in this harness. What a shared, correct helper DOES buy: every
# scenario that mounts something detaches it before removing its own
# directory on every path that CAN run a trap (normal exit, a caught signal
# like SIGTERM/SIGINT), and no future scenario can get that ordering wrong
# by copying an earlier one that already had it wrong. The actual defense
# against SIGKILL is sweep_orphaned_volumes below, run once before a batch
# starts, plus refusing to let two batches run at once -- see run.sh.
#
# Usage: after a successful `hdiutil attach ... -mountpoint "$MNT"`, call
# `register_mount "$MNT"`. This only appends to a list -- it does not wire
# anything into a trap by itself. The scenario's own `cleanup()` must call
# `detach_registered_mounts` before removing its workdir, the same way it
# already calls `rm -rf "$W"`; get that ordering right once, in cleanup(),
# and every mount registered here is detached in the right order regardless
# of how many times the scenario re-attaches under the same or different
# paths.
if ! declare -p REGISTERED_MOUNTS >/dev/null 2>&1; then
  declare -a REGISTERED_MOUNTS=()
fi
register_mount() {
  REGISTERED_MOUNTS+=("$1")
}
detach_registered_mounts() {
  local m
  for m in "${REGISTERED_MOUNTS[@]:-}"; do
    [ -n "$m" ] && [ -d "$m" ] && hdiutil detach "$m" -force >/dev/null 2>&1
  done
}

# Best-effort cleanup of volumes orphaned by a killed run, run once before a
# batch starts (see run.sh), not per scenario. Issue #13: a scenario killed
# with SIGKILL leaves its mount attached forever as far as this harness is
# concerned, because nothing inside that process ever runs again to detach
# it. This is the actual mitigation -- not preventing the leak (impossible
# from inside a process that received SIGKILL) but sweeping it up before the
# next batch compounds it.
#
# Only ever touches a device whose reported mount point contains
# "etudes-stress-", this harness's own naming convention (workdir(), above).
# Never touches anything else mounted on the machine.
#
# Some entries are unrecoverable from here: `diskutil list "$dev"` failing
# after `hdiutil info` still lists the device means the kernel-level disk is
# already gone and only diskimagesiod's own bookkeeping still thinks it is
# attached (reproduced directly while building this: a process that died
# via SIGKILL left exactly this state, `hdiutil detach` failing with "No
# such file or directory" against a device diskutil could not find at all).
# Fixing that needs restarting diskimagesiod, a system daemon whose restart
# could affect disk images this harness has nothing to do with -- out of
# proportion for a test suite to do on its own. Reported and skipped, not
# silently ignored.
sweep_orphaned_volumes() {
  # Skipping silently is correct here -- failing the whole batch because an
  # opportunistic cleaner lacks a tool would be worse than leaving an orphan
  # for a human to find later, and every scenario that actually needs a
  # volume already checks for hdiutil itself and goes unproven if it is
  # missing. A one-line trace is still worth having so a silent skip is
  # findable rather than invisible when someone is debugging why a leak
  # from a prior run wasn't swept.
  command -v hdiutil >/dev/null 2>&1 || {
    echo "  orphan sweep: skipped, no hdiutil on this host" >&2
    return 0
  }
  local plist json dev mnt
  plist=$(hdiutil info -plist 2>/dev/null) || {
    echo "  orphan sweep: skipped, hdiutil info failed" >&2
    return 0
  }
  json=$(plutil -convert json -o - - <<<"$plist" 2>/dev/null) || {
    echo "  orphan sweep: skipped, could not parse hdiutil's output" >&2
    return 0
  }
  local pairs
  pairs=$(python3 -c '
import json, sys
try:
    d = json.load(sys.stdin)
except Exception:
    sys.exit(0)
for img in d.get("images", []):
    for e in img.get("system-entities", []):
        dev = e.get("dev-entry")
        mnt = e.get("mount-point")
        if dev and mnt and "etudes-stress-" in mnt:
            print(f"{dev}\t{mnt}")
' <<<"$json" 2>/dev/null)
  [ -z "$pairs" ] && return 0
  local swept=0 stale=0 refused=0
  while IFS=$'\t' read -r dev mnt; do
    [ -z "$dev" ] && continue
    if ! diskutil list "$dev" >/dev/null 2>&1; then
      stale=$((stale + 1))
      echo "  orphan sweep: $dev ($mnt) is already gone at the kernel level; diskimagesiod's own bookkeeping is stale. Not fixable from here -- see sweep_orphaned_volumes' comment in stress/lib.sh." >&2
      continue
    fi
    if hdiutil detach "$dev" -force >/dev/null 2>&1; then
      swept=$((swept + 1))
      echo "  orphan sweep: detached $dev, left mounted at $mnt by a previous killed run" >&2
    else
      # A review caught this branch missing: a live, diskutil-confirmed
      # device whose detach itself fails (e.g. genuinely busy) was silently
      # dropped, with no line distinguishing it from a clean sweep or the
      # kernel-gone stale case above.
      refused=$((refused + 1))
      echo "  orphan sweep: $dev ($mnt) is live but refused to detach (e.g. busy); left as-is" >&2
    fi
  done <<<"$pairs"
  if [ "$swept" -gt 0 ] || [ "$stale" -gt 0 ] || [ "$refused" -gt 0 ]; then
    echo "  orphan sweep: $swept detached, $stale unrecoverable stale, $refused live-but-refused (all reported above)" >&2
  fi
}

require() {  # require CMD REASON: mark unproven and return 1 if missing
  command -v "$1" >/dev/null 2>&1 && return 0
  unproven "$2" "$1 not available on this host"
  return 1
}

# Read the assertion record exactly once and derive both the displayed counts
# and the status from it.  run.sh calls this same function after each child.
stress_outcome() {
  local record="$1" child_status="$2" line
  STRESS_PASSED=0; STRESS_FAILED=0; STRESS_UNPROVEN=0; STRESS_COMPLETED=0
  while IFS= read -r line; do
    case "$line" in
      ok) STRESS_PASSED=$((STRESS_PASSED + 1));;
      FAIL) STRESS_FAILED=$((STRESS_FAILED + 1));;
      unproven) STRESS_UNPROVEN=$((STRESS_UNPROVEN + 1));;
      complete) STRESS_COMPLETED=1;;
    esac
    if [ -n "${STRESS_RESULT_FD:-}" ]; then printf '%s\n' "$line" >&"$STRESS_RESULT_FD"; fi
  done < "$record"
  # A failed conditional at the end of a scenario is not a failed assertion.
  # Preserve signal deaths, and use USR1 only as a fallback for a lost record.
  # Exit 2 is the wrapper's explicit all-unproven verdict.  It is incomplete
  # coverage, not an assertion failure; preserve it so the runner can report
  # that distinction. Any other nonzero child status is an unexpected abort.
  if [ "$STRESS_FAILED" -eq 0 ] && { [ "${STRESS_WRAPPER_FAILED:-0}" -ne 0 ] || { [ "$child_status" -ne 0 ] && [ "$child_status" -ne 2 ]; } || [ "$STRESS_COMPLETED" -ne 1 ] || [ $((STRESS_PASSED + STRESS_UNPROVEN)) -eq 0 ]; }; then
    STRESS_FAILED=1
    [ -z "${STRESS_RESULT_FD:-}" ] || printf 'FAIL\n' >&"$STRESS_RESULT_FD"
  fi
  [ "$STRESS_FAILED" -gt 0 ] && return 1
  [ "$STRESS_PASSED" -eq 0 ] && return 2
  return 0
}

# Direct execution needs an owner outside the scenario shell: command
# substitutions and subshells cannot change the parent's counters.  Re-source
# the scenario as a background job in its own process group, keeping only an
# unlinked high-fd assertion record in common.
# Depth is the recursion stop; fd 199 is only the record channel. Trusted
# scenarios must not forge the guard, truncate the record or escape the group.
# SIGKILL of this wrapper cannot be forwarded or cleaned up.
if [ -n "${STRESS_WRAP_DEPTH:-}" ]; then
  if ! { true >&199; } 2>/dev/null; then
    echo 'stress/lib.sh: wrapper record fd 199 is absent; running unwrapped' >&2
  fi
elif [ "${SCENARIO:-}" != run ]; then
  _stress_record_file=$(mktemp "${TMPDIR:-/tmp}/etudes-stress-record-XXXXXX") || exit 1
  exec 198<&- 199>&-
  exec 198<"$_stress_record_file"
  exec 199>>"$_stress_record_file"
  rm -f "$_stress_record_file"
  export STRESS_WRAP_DEPTH=1 STRESS_WRAPPER_PID=$$
  STRESS_WRAPPER_FAILED=0
  trap 'STRESS_WRAPPER_FAILED=1; _stress_wait_interrupted=1' USR1
  _stress_forward() {
    _stress_wait_interrupted=1
    kill -"$1" -- "-$_stress_child" 2>/dev/null || true
  }
  trap '_stress_forward INT' INT
  trap '_stress_forward TERM' TERM
  trap '_stress_forward HUP' HUP
  trap '_stress_forward QUIT' QUIT
  # The batch runner already owns a process group for its case. Job control
  # would put this child in a different group and let it escape that reaper.
  [ "${STRESS_NO_JOB_CONTROL:-0}" = 1 ] || set -m
  /bin/bash "$0" "$@" <&0 &
  _stress_child=$!
  # USR1 interrupts wait (158 on macOS); wait again until the job is reaped.
  while :; do
    _stress_wait_interrupted=0
    wait "$_stress_child"; _stress_child_status=$?
    [ "$_stress_wait_interrupted" -eq 0 ] && break
  done
  if [ "$_stress_child_status" -eq 0 ]; then
    printf 'complete\n' >&199
  fi
  kill -TERM -- "-$_stress_child" 2>/dev/null || true
  kill -KILL -- "-$_stress_child" 2>/dev/null || true
  stress_outcome /dev/fd/198 "$_stress_child_status"
  exit $?
fi
