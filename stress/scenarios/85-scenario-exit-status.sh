#!/usr/bin/env bash
# A scenario's exit status must answer the same question its output answers.
#
# fail() counts and prints but used to leave $? alone, so a scenario run
# outside stress/run.sh ended 0 with FAIL lines on screen; anything that asks
# the shell the ordinary question -- a git hook, a CI step, an editor task,
# an agent checking one scenario -- read that as a pass. Issue #95.
#
# The status is only reachable if lib.sh owns the EXIT trap (bash traps do
# not stack), so scenarios register cleanups with on_exit instead of setting
# their own trap. It is only CORRECT if the failure count is captured before
# any registered cleanup runs, since a cleanup that touches FAILED could
# otherwise erase a real failure on its way out. This scenario drives all
# three shapes directly, rather than trusting a green suite run to have
# exercised them.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir); on_exit 'rm -rf "$W"'
LIB="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/lib.sh"

write() { printf '%s\n' "$2" > "$W/$1"; printf 'source "%s"\n' "$LIB" | cat - "$W/$1" > "$W/$1.tmp" && mv "$W/$1.tmp" "$W/$1"; }

write failing.sh 'assert_eq one two "an assertion that must fail"'
write passing.sh 'assert_eq one one "an assertion that must pass"'
write reset.sh $'on_exit \x27FAILED=0\x27\nfail "deliberate"\nexit 0'

out=$(BIN="$BIN" SCENARIO=exit-probe-fail bash "$W/failing.sh" 2>&1); got=$?
assert_eq 1 "$got" "a scenario with a failed assertion exits non-zero"
case "$out" in *FAIL*) pass "it still prints its FAIL line" ;;
  *) fail "it printed no FAIL line: ${out%%$'\n'*}" ;; esac

out=$(BIN="$BIN" SCENARIO=exit-probe-pass bash "$W/passing.sh" 2>&1); got=$?
assert_eq 0 "$got" "a scenario with no failed assertion exits zero"
case "$out" in *ok*) pass "it still prints its ok line" ;;
  *) fail "it printed no ok line: ${out%%$'\n'*}" ;; esac

# A cleanup that resets FAILED on its way out must not erase a real failure:
# the count is captured before cleanups run, not read fresh afterward.
BIN="$BIN" SCENARIO=exit-probe-reset bash "$W/reset.sh" >/dev/null 2>&1
assert_eq 1 "$?" "a cleanup that resets FAILED does not erase a failed run"

# A cleanup calling exit itself must not terminate the decision: cleanups run
# in a subshell, so exit inside one ends only that subshell.
write shortcircuit.sh $'fail "deliberate"\non_exit "exit 0"'
BIN="$BIN" SCENARIO=exit-probe-shortcircuit bash "$W/shortcircuit.sh" >/dev/null 2>&1
assert_eq 1 "$?" "a cleanup that calls exit itself cannot hide a real failure"

# Sourcing lib.sh a second time must not reset the state the first sourcing
# already recorded; it must refuse outright.
printf '%s\n' "source \"$LIB\"" 'assert_eq one two deliberate' "source \"$LIB\"" > "$W/doublesource.sh"
BIN="$BIN" SCENARIO=exit-probe-doublesource bash "$W/doublesource.sh" >/dev/null 2>&1
assert_eq 1 "$?" "sourcing lib.sh twice cannot discard an already-recorded failure"

# A scenario-owned EXIT trap replaces the one that carries the status; none
# in this suite may set one.
owned=$(grep -lE "^[[:space:]]*trap[[:space:]].*EXIT" "$(dirname "${BASH_SOURCE[0]}")"/*.sh 2>/dev/null \
  | grep -v "$(basename "${BASH_SOURCE[0]}")" | wc -l | tr -d ' ')
assert_eq 0 "$owned" "no scenario sets its own EXIT trap; every cleanup goes through on_exit"

touch "$W/marker"
cat > "$W/cleanup.sh" <<INNER
source "$LIB"
on_exit 'rm -f "$W/marker"'
INNER
BIN="$BIN" SCENARIO=exit-probe-cleanup bash "$W/cleanup.sh" >/dev/null 2>&1
if [ -e "$W/marker" ]; then fail "an on_exit cleanup did not run"; else pass "an on_exit cleanup ran"; fi
