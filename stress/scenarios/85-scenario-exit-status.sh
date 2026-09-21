#!/usr/bin/env bash
# Direct-run wrapper witness.  Each arm is a fresh scenario process so an
# intentionally failed assertion cannot contaminate the next assertion.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SELF="$ROOT/stress/scenarios/85-scenario-exit-status.sh"

if [ -n "${EXIT_STATUS_ARM:-}" ]; then
  source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
  case "$EXIT_STATUS_ARM" in
    fail_exit) fail 'deliberate failure'; exit 0;;
    subshell) (fail 'subshell deliberate failure'); pass 'continued after subshell';;
    captured) captured=$(fail 'captured deliberate failure'); pass 'continued after captured failure';;
    closed_fd) captured=$(exec 199>&-; fail 'closed-fd deliberate failure'); pass 'continued after closed fd';;
    pass_exec) pass 'a passing scenario may exec'; exec true;;
    fail_exec) fail 'failure before exec'; exec true;;
    killed) pass 'started before SIGKILL'; kill -9 $$;;
    false_tail) pass 'assertion passed before false conditional'; false; exit $?;;
    abort_after_pass) pass 'setup passed before deliberate abort'; exit 42;;
    only_unproven) unproven 'requires unavailable host capability' 'deliberate';;
    mixed) pass 'available check'; unproven 'unavailable check' 'deliberate';;
    cleanup_exit) trap 'FAILED=0; exit 0' EXIT; fail 'cleanup must not hide failure';;
    resource) test -d "$ETUDE_STATE_DIR" && test -n "$(env | grep '^ETUDE_STATE_DIR=')" && pass 'state exported before any helper';;
    newline_name) d="$ETUDE_STATE_DIR/newline"; mkdir -p "$d"; : > "$d/one
two.txt"; assert_intact "$d" 1 'newline-containing filename counts as one file';;
    reload) fail 'failure before reload'; source "$ROOT/stress/lib.sh"; pass 'continued after reload';;
    arguments) assert_eq preserved "${1:-missing}" 'wrapper preserves script arguments';;
    bounded_timeout) run_bounded 50 "$ETUDE_STATE_DIR/bounded.json" -- sh -c 'sleep 2'; assert_eq 124 "$?" 'bounded helper reports a deadline'; python3 - "$ETUDE_STATE_DIR/bounded.json" <<'PY'
import json, sys
result = json.load(open(sys.argv[1]))
assert result['timed_out'] is True and result['signal'] == 9 and result['reaped'] is True and result['duration_ms'] < 1000, result
PY
      pass 'bounded helper records timeout, signal, duration, output evidence, and reaping';;
    catalog_rejects) python3 - "$ROOT/stress/catalog.json" "$ETUDE_STATE_DIR/missing.json" "$ETUDE_STATE_DIR/duplicate.json" "$ETUDE_STATE_DIR/invalid-disposition.json" <<'PY'
import json, sys
rows = json.load(open(sys.argv[1]))
json.dump(rows[1:], open(sys.argv[2], 'w'))
json.dump(rows + [rows[0]], open(sys.argv[3], 'w'))
rows[0]['disposition'] = 'mystery'
json.dump(rows, open(sys.argv[4], 'w'))
PY
      python3 "$ROOT/scripts/check-stress-catalog.py" --catalog "$ETUDE_STATE_DIR/missing.json" >/dev/null 2>&1; missing=$?
      python3 "$ROOT/scripts/check-stress-catalog.py" --catalog "$ETUDE_STATE_DIR/duplicate.json" >/dev/null 2>&1; duplicate=$?
      python3 "$ROOT/scripts/check-stress-catalog.py" --catalog "$ETUDE_STATE_DIR/invalid-disposition.json" >/dev/null 2>&1; invalid_disposition=$?
      assert_eq 1 "$missing" 'catalog rejects a missing tracked scenario'
      assert_eq 1 "$duplicate" 'catalog rejects a duplicate scenario id'
      assert_eq 1 "$invalid_disposition" 'catalog rejects an invalid disposition';;
    unwrapped) pass 'missing wrapper fd runs the scenario body unwrapped'; exit 0;;
  esac
  exit 0
fi

source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
run_arm() {
  local arm="$1" want="$2" got
  env -u STRESS_WRAP_DEPTH -u STRESS_WRAPPER_PID -u STRESS_RESULT_FD EXIT_STATUS_ARM="$arm" SCENARIO="85-$arm" BIN="$BIN" /bin/bash "$SELF" preserved >/dev/null 2>&1
  got=$?
  assert_eq "$want" "$got" "scenario exit status: $arm exits $want"
}
run_arm fail_exit 1
run_arm subshell 1
run_arm captured 1
run_arm closed_fd 1
run_arm pass_exec 0
run_arm fail_exec 1
env -u STRESS_WRAP_DEPTH -u STRESS_WRAPPER_PID -u STRESS_RESULT_FD EXIT_STATUS_ARM=killed SCENARIO=85-killed BIN="$BIN" bash "$SELF" >/dev/null 2>&1
[ "$?" -ne 0 ] && pass 'scenario exit status: SIGKILL is non-zero' || fail 'scenario exit status: SIGKILL was zero'
STRESS_WRAP_DEPTH=1 EXIT_STATUS_ARM=unwrapped SCENARIO=85-unwrapped BIN="$BIN" bash -c 'exec 199>&-; exec bash "$1"' -- "$SELF" >/dev/null 2>&1
[ "$?" -eq 0 ] && pass 'scenario exit status: missing wrapper fd runs unwrapped' || fail 'scenario exit status: missing wrapper fd did not run'

run_arm false_tail 1
run_arm abort_after_pass 1
run_arm only_unproven 2
run_arm mixed 0
run_arm cleanup_exit 1
run_arm resource 0
run_arm newline_name 0
run_arm reload 1
run_arm arguments 0
run_arm bounded_timeout 0
run_arm catalog_rejects 0
