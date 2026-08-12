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
trap 'c=$?; [ "$c" = "127" ] && echo "    FAIL     command not found at line $LINENO. The scenario is broken, not the tool"' ERR

PASSED=0; FAILED=0; UNPROVEN=0
FAIL_LINES=(); UNPROVEN_LINES=()

# The binaries under test. Built by run.sh; never resolved from PATH, so a
# stale copy elsewhere cannot be tested by accident.
BIN="${BIN:?BIN must point at target/release}"
SWEEP="$BIN/sweep"; STASH="$BIN/stash"; UNPACK="$BIN/unpack"; MKFX="$BIN/mkfx"

pass()     { PASSED=$((PASSED+1)); printf '    ok       %s\n' "$1"; }
fail()     { FAILED=$((FAILED+1)); FAIL_LINES+=("$SCENARIO: $1"); printf '    FAIL     %s\n' "$1"; }
unproven() { UNPROVEN=$((UNPROVEN+1)); UNPROVEN_LINES+=("$SCENARIO: $1 ($2)"); printf '    unproven %s (%s)\n' "$1" "$2"; }

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

# assert_intact DIR N LABEL: nothing was destroyed
assert_intact() {
  local n; n=$(find "$1" -type f 2>/dev/null | wc -l | tr -d ' ')
  if [ "$n" = "$2" ]; then pass "$3 ($n files intact)"
  else fail "$3: expected $2 files, found $n. FILES WERE LOST"; fi
}

# Journals go to a scratch state directory, never the real one.
#
# XDG_STATE_HOME redirects where journals are written while leaving HOME alone,
# so the keychain still works. Without this, a scenario would write into the
# user's own journal directory. `undo` picks the newest journal globally.
# Two scenarios could undo each other's work. A stress run would leave real
# state behind.
export XDG_STATE_HOME="${XDG_STATE_HOME_OVERRIDE:-$(mktemp -d "${TMPDIR:-/tmp}/etudes-stress-state-XXXXXX")}"
trap 'rm -rf "$XDG_STATE_HOME"' EXIT

# A scratch tree, unique per scenario, removed on exit.
workdir() {
  local d; d=$(mktemp -d "${TMPDIR:-/tmp}/etudes-stress-${SCENARIO}-XXXXXX")
  echo "$d"
}

require() {  # require CMD REASON: mark unproven and return 1 if missing
  command -v "$1" >/dev/null 2>&1 && return 0
  unproven "$2" "$1 not available on this host"
  return 1
}
