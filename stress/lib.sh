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
# A scenario run by hand has no SCENARIO; name it after the script so messages
# and scratch folders still work under set -u.
SCENARIO="${SCENARIO:-$(basename "$0" .sh)}"

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
# supplied key by default.
#
# No scenario may run a tool without a key except through keychain_fenced below.
# With no key the tools use the login keychain of whoever runs the suite, and can
# add a key to it. A fake HOME is not proof against that, and a `security` stub on
# PATH does nothing, because the tools call /usr/bin/security by its full path.
# scripts/check-stress-catalog.py refuses a scenario that drops the key another way.
if [ -z "${ETUDE_JOURNAL_KEY:-}" ]; then
  export ETUDE_JOURNAL_KEY="$(python3 -c 'import secrets; print(secrets.token_hex(32))')"
fi

KEYCHAIN_FENCE='(version 1)(allow default)(deny process-exec (literal "/usr/bin/security"))'

# True only if sandbox-exec runs a harmless control and enforces an added
# process-exec denial. No security process is launched, even as a control.
keychain_fence_works() {
  command -v sandbox-exec >/dev/null 2>&1 || return 1
  sandbox-exec -p "$KEYCHAIN_FENCE" /usr/bin/true >/dev/null 2>&1 || return 1
  # Prove process-exec denial with a harmless command, never security itself.
  ! sandbox-exec -p "$KEYCHAIN_FENCE"'(deny process-exec (literal "/usr/bin/true"))' /usr/bin/true >/dev/null 2>&1
}

# keychain_fenced COMMAND...: run a command with no journal key, in a sandbox
# where /usr/bin/security cannot start, so the keychain looks unavailable and is
# never reached. If the sandbox cannot be applied, the command does not run.
keychain_fenced() {
  sandbox-exec -p "$KEYCHAIN_FENCE" env -u ETUDE_JOURNAL_KEY "$@"
}

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

# assert_no_panic LABEL OUTPUT: a crash is never a message, whatever the exit code.
assert_no_panic() {
  case "$2" in
    *panicked*) fail "$1 panicked: ${2%%$'\n'*}" ;;
    *) pass "$1 did not panic" ;;
  esac
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

# stop_confirmed PID: a successful SIGSTOP delivery is only a request. Wait
# for ps(1)'s documented T state before the caller inspects and injects.
# On failure the child is resumed; it must never be left stopped by a miss.
stop_confirmed() {
  local pid="$1" state deadline=$((SECONDS + 3))
  kill -STOP "$pid" 2>/dev/null || return 1
  while [ "$SECONDS" -lt "$deadline" ]; do
    state=$(ps -o state= -p "$pid" 2>/dev/null) || break
    [[ "$state" != *T* ]] || return 0
    [[ "$state" != *Z* ]] || break
  done
  kill -CONT "$pid" 2>/dev/null || true
  return 1
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
# ETUDE_STATE_DIR redirects journals and HOME isolates configuration. Both
# directories are disposable; keychain access is separately fenced. Without
# this, a scenario would write
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

# The batch runner owns this parent even when SIGKILL prevents EXIT cleanup.
# Direct runs retain their independent temporary directories.
export HOME="$(mktemp -d "${STRESS_CASE_ROOT:-${TMPDIR:-/tmp}}/etudes-stress-home-XXXXXX")"

export ETUDE_STATE_DIR="${ETUDE_STATE_DIR_OVERRIDE:-$(mktemp -d "${STRESS_CASE_ROOT:-${TMPDIR:-/tmp}}/etudes-stress-state-XXXXXX")}"
# Bind the chosen directory now: scenarios may later change the override.
# Sourcing this file again (a scenario that reloads it) makes a second state
# folder. Keep the first one's removal in the list, or it is left behind.
_stress_earlier_state_cleanup=${_stress_state_cleanup:-}
printf -v _stress_state_cleanup 'rm -rf -- %q %q' "$ETUDE_STATE_DIR" "$HOME"
[ -z "$_stress_earlier_state_cleanup" ] || _stress_state_cleanup="$_stress_earlier_state_cleanup; $_stress_state_cleanup"
_stress_exit_notice='code=$?; if [ "$code" != 0 ] && [ "$BASH_SUBSHELL" = 0 ]; then echo "    FAIL     scenario exited $code before finishing. It did not run to completion"; fi'
builtin trap "$_stress_exit_notice
$_stress_state_cleanup" EXIT

# Almost every scenario sets its own EXIT trap to remove its work folder, and
# that replaces the line above. 46 of 77 scenarios then left their state folder
# in the temp folder on every run, because the trap they wrote never mentioned
# it. Rather than ask each scenario (and every future one) to remember, any
# EXIT trap set from here on also removes the state folder, after the
# scenario's own commands. A trap set in a `( ... )` subshell is left alone:
# the subshell ending must not take the state folder away from the scenario
# still running. This guard does not cover `$( ... )`: under /bin/bash 3.2,
# $BASH_SUBSHELL is 0 in an EXIT trap fired there, so never set an EXIT trap
# inside a command substitution.
# Only the `trap CMD EXIT` and `trap - EXIT` forms are rewritten. Every other
# use passes straight through to the builtin, including `trap -p EXIT`, so
# `trap -- CMD EXIT` and `trap CMD EXIT INT` do not get the state cleanup.
#
# A scenario whose trap ends in `exit` must run "$_stress_state_cleanup" itself
# before that `exit`; a later command in the same trap never runs.
trap() {
  if [ "$#" -eq 2 ] && { [ "$2" = EXIT ] || [ "$2" = 0 ]; } \
    && { [ "$1" = - ] || [ "${1#-}" = "$1" ]; }; then
    local _stress_user_trap=$1
    [ "$_stress_user_trap" != - ] || _stress_user_trap=
    builtin trap "$_stress_exit_notice
$_stress_user_trap
[ \"\$BASH_SUBSHELL\" != 0 ] || { $_stress_state_cleanup; }" EXIT
    return 0
  fi
  builtin trap "$@"
}

# A scratch tree, unique per scenario, removed on exit. A scenario run by hand
# has no SCENARIO set; name the tree after the script instead of failing, and
# never return an empty path: a caller that resolves an empty path with cd lands
# in its current folder, and its exit trap would then remove that folder.
workdir() {
  local d; d=$(mktemp -d "${TMPDIR:-/tmp}/etudes-stress-${SCENARIO:-$(basename "$0" .sh)}-XXXXXX") || d=""
  if [ -z "$d" ] || [ ! -d "$d" ]; then
    echo "stress: could not make a scratch folder; stopping before any cleanup can touch the current folder" >&2
    return 1
  fi
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

# Create a blank filesystem image using the current macOS API when it is
# available. `hdiutil create -fs` still works for APFS on older systems, but
# recent macOS releases direct callers to `diskutil image create blank`; the
# latter is also the path that can create exFAT images on this host.
#
# Callers continue to use hdiutil for attach/detach, so this is deliberately a
# narrow creation helper rather than another mount lifecycle abstraction.
create_disk_image() {
  local image=$1 size=$2 filesystem=$3 volume_name=$4
  if command -v diskutil >/dev/null 2>&1; then
    if diskutil image create blank -size "$size" -fs "$filesystem" \
      --volumeName "$volume_name" "$image" >/dev/null 2>&1; then
      return 0
    fi
  fi
  hdiutil create -size "$size" -fs "$filesystem" -volname "$volume_name" \
    "$image" >/dev/null 2>&1
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
  _stress_final_status=$?
  # This is the wrapper's completed verdict, not an early scenario abort.
  builtin trap "$_stress_state_cleanup" EXIT
  exit "$_stress_final_status"
fi
