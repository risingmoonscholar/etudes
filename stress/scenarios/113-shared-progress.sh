#!/usr/bin/env bash
# Every tool's --progress-json, checked against what the run really did.
#
# 1. Operation and failure: apply, undo, stash, pop and extraction run to the
#    end, and apply and extraction are killed part-way.
# 2. Proof the fixture occurred: moved files and extracted members are counted
#    from the filesystem, and a kill is only delivered after a record shows the
#    run under way.
# 3. Passing state: scripts/check-progress-records.py accepts each stream, final
#    counts equal the filesystem, a killed run has no terminal record and no
#    record overstates what the filesystem shows.
# 4. Broken counterpart: scripts/check-progress-mutation.sh plants the faults
#    this contract rejects.
# 5. Bounds: at most 3000 empty files and 120 MiB of stored members.
# 6. Distinct gap: 80-interrupted-extraction and 30-sigkill-mid-apply prove the
#    files stay safe; neither reads what the tools said while it happened.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
exec </dev/null
W=$(workdir) || exit 1; trap 'rm -rf "$W"; eval "$_stress_state_cleanup"' EXIT
export ETUDE_JOURNAL_KEY="$(python3 -c 'import secrets; print(secrets.token_hex(32))')"
export ETUDE_STATE_DIR="$W/state" SWEEP_GRACE_SECS=0
CHECK="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)/scripts/check-progress-records.py"

records_ok() {  # LABEL OPERATION STDOUT STDERR [witness flags]
  local label="$1" operation="$2" out="$3" err="$4"; shift 4
  local said
  if said=$(python3 -I "$CHECK" --operation "$operation" --stdout "$out" --stderr "$err" "$@" 2>&1); then
    pass "$label: $said"
  else
    fail "$label: $said"
  fi
}
wait_for_record() {  # FILE PATTERN PID
  for _ in $(seq 1 2000); do
    grep -q "$2" "$1" 2>/dev/null && return 0
    kill -0 "$3" 2>/dev/null || return 1
    sleep 0.01
  done
  return 1
}
last_count() {  # FILE FIELD
  python3 -I - "$1" "$2" <<'PY'
import json, sys
rows = [json.loads(line) for line in open(sys.argv[1], errors="replace") if line.startswith("{")]
print(rows[-1][sys.argv[2]])
PY
}

stop_confirmed() {
  kill -STOP "$1" 2>/dev/null || return 1
  for _ in $(seq 1 100); do
    ps -o state= -p "$1" | grep -q T && return 0
    kill -0 "$1" 2>/dev/null || return 1
  done
  kill -CONT "$1" 2>/dev/null || true
  return 1
}

# --- full runs ---------------------------------------------------------------
D="$W/sweep"; mkdir -p "$D"
for n in $(seq 1 400); do : > "$D/Screenshot $n.png"; done
"$SWEEP" apply "$D" --yes --since 0 --json --progress-json >"$W/a.out" 2>"$W/a.err"
assert_eq 0 "$?" "sweep apply finished"
records_ok "sweep apply" apply "$W/a.out" "$W/a.err" --planned 400 --completed 400 --journalled 400 --forbid Screenshot
assert_eq 400 "$(find "$D/Screenshots" -type f | wc -l | tr -d ' ')" "the 400 records match 400 moved files"
"$SWEEP" undo "$D" --json --progress-json >"$W/u.out" 2>"$W/u.err"
assert_eq 0 "$?" "sweep undo finished"
records_ok "sweep undo" undo "$W/u.out" "$W/u.err" --planned 400 --completed 400 --journalled 400 --forbid Screenshot

D="$W/stash"; mkdir -p "$D"
for n in $(seq 1 300); do : > "$D/item $n"; done
"$STASH" "$D" --json --progress-json >"$W/s.out" 2>"$W/s.err"
assert_eq 0 "$?" "stash finished"
records_ok "stash" stash "$W/s.out" "$W/s.err" --planned 300 --completed 300 --journalled 300 --forbid "item "
"$STASH" pop "$D" --json --progress-json >"$W/p.out" 2>"$W/p.err"
assert_eq 0 "$?" "stash pop finished"
records_ok "stash pop" pop "$W/p.out" "$W/p.err" --planned 300 --completed 300 --journalled 300 --forbid "item "
assert_eq 300 "$(find "$D" -maxdepth 1 -type f | wc -l | tr -d ' ')" "pop's records match the 300 items back in the folder"

S="$W/source"; mkdir -p "$S/nested/folder"
for n in $(seq 1 500); do : > "$S/nested/folder/f$n"; done
(cd "$S" && find nested -type f > "$W/names" && tar -cf "$W/a.tar" -T "$W/names")
"$UNPACK" "$W/a.tar" --into "$W/tree" --json --progress-json >"$W/x.out" 2>"$W/x.err"
assert_eq 0 "$?" "unpack extraction finished"
records_ok "unpack a tar" extract "$W/x.out" "$W/x.err" --planned 500 --completed 500 --journalled 0 --forbid nested
"$UNPACK" "$W/a.tar" --list --json --progress-json >"$W/l.out" 2>"$W/l.err"
assert_eq 0 "$?" "unpack list finished"
records_ok "unpack --list" list "$W/l.out" "$W/l.err" --planned 500 --completed 500 --forbid nested

if ! ps -o state= -p $$ >/dev/null 2>&1; then
  unproven "witnessed process interruption" "host denies process-state inspection"
  exit 0
fi
# --- a kill during extraction ------------------------------------------------
Z="$W/zipsource"; mkdir -p "$Z"
for n in $(seq 1 120); do dd if=/dev/zero of="$Z/chunk$n" bs=1048576 count=1 status=none; done
(cd "$Z" && zip -0 -q "$W/big.zip" ./*)
"$UNPACK" "$W/big.zip" --into "$W/killed" --progress-json >"$W/k.out" 2>"$W/k.err" & PID=$!
if wait_for_record "$W/k.err" '"planned":120' "$PID" && stop_confirmed "$PID"; then
  pass "extraction was under way before the signal"
  STAGE=$(find "$W" -maxdepth 1 -type d -name '.killed.unpack-*.partial' -print -quit)
  kill -9 "$PID" 2>/dev/null; wait "$PID" 2>/dev/null
  WHOLE=$(find "$STAGE" -type f -size 2048 2>/dev/null | wc -l | tr -d ' ')
  records_ok "killed extraction" extract "$W/k.out" "$W/k.err" --planned 120 --killed
  SAID=$(last_count "$W/k.err" completed)
  [ "$SAID" -le "$WHOLE" ] && pass "last record said $SAID; $WHOLE members were whole when checked after the kill" \
    || fail "last record said $SAID but only $WHOLE members were whole"
  [ ! -e "$W/killed" ] && pass "the killed extraction published nothing" || fail "a killed extraction published"
  for _ in $(seq 1 1000); do pgrep -f "$STAGE" >/dev/null 2>&1 || break; sleep 0.01; done
else
  fail "extraction never reached a counted record"
  wait "$PID" 2>/dev/null
fi

# --- a kill during apply -----------------------------------------------------
D="$W/killedsweep"; mkdir -p "$D"
for n in $(seq 1 3000); do : > "$D/Screenshot $n.png"; done
"$SWEEP" apply "$D" --yes --since 0 --progress-json >"$W/ka.out" 2>"$W/ka.err" & PID=$!
if wait_for_record "$W/ka.err" '"phase":"advancing"' "$PID" && stop_confirmed "$PID"; then
  kill -9 "$PID" 2>/dev/null; wait "$PID" 2>/dev/null
  records_ok "killed apply" apply "$W/ka.out" "$W/ka.err" --killed
  MOVED=$(find "$D/Screenshots" -type f | wc -l | tr -d ' ')
  SAID=$(last_count "$W/ka.err" completed); DURABLE=$(last_count "$W/ka.err" journalled)
  [ "$SAID" -le "$MOVED" ] && pass "last record said $SAID moved; the folder holds $MOVED" \
    || fail "last record said $SAID moved but the folder holds $MOVED"
  STATUS=$("$SWEEP" status "$D" --json 2>/dev/null)
  RECORDED=$(python3 -I -c 'import json,sys; v=json.loads(sys.argv[1]); d=v["details"]; d=d[0] if isinstance(d,list) else d; print(d["counts"]["moved"])' "$STATUS")
  [ "$DURABLE" -le "$RECORDED" ] && pass "last record said $DURABLE durable; the journal holds $RECORDED" \
    || fail "last record said $DURABLE durable but the journal holds $RECORDED"
  "$SWEEP" undo "$D" --json >"$W/kau.out" 2>/dev/null
  assert_eq 3000 "$(find "$D" -maxdepth 1 -type f | wc -l | tr -d ' ')" "undo after the kill returned every file"
else
  fail "apply never reached an advancing record"
  wait "$PID" 2>/dev/null
fi
