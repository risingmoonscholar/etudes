#!/usr/bin/env bash
# Two `stash pop` of one folder at the same time. The first pop is stopped part
# way, with some but not all files home and the kernel reporting it stopped, and
# the second starts while it is held. The second must wait for the first and
# then say the stash is back. It must not restore around the stopped pop, and
# neither pop may report a failed or interrupted restore, or print a step to
# fix, for a stash whose files are home. A later status and pop agree that
# nothing is left.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir) || exit 1
W=$(cd "${W:?}" && pwd -P) && [ -n "$W" ] || exit 1
A=""; B=""
cleanup() {
  for pid in $A $B; do
    kill -CONT "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  done
  rm -rf "${W:?}"
}
trap cleanup EXIT
require python3 "the fixture and the JSON checks use Python" || exit 0
export HOME="$W/home"; mkdir -p "$HOME"
TOTAL=2000

make_tree() {
  python3 - "$1" "$TOTAL" <<'PY_TREE'
import pathlib, sys
d, n = pathlib.Path(sys.argv[1]), int(sys.argv[2])
d.mkdir(parents=True)
for i in range(n):
    (d / f"note {i:04d}.txt").write_text(f"note {i}\n")
PY_TREE
}

# state PID: the process state as ps prints its first letter, from the kernel.
# ps cannot start inside a sandbox, so this asks proc_pidinfo(2) directly.
state() {
  python3 - "$1" <<'PY_STATE'
import ctypes, sys
libc = ctypes.CDLL("/usr/lib/libSystem.B.dylib")
info = ctypes.create_string_buffer(136)  # struct proc_bsdinfo
if libc.proc_pidinfo(int(sys.argv[1]), 3, ctypes.c_uint64(0), info, 136) != 136:
    sys.exit(1)  # PROC_PIDTBSDINFO; no such process
# pbi_status is p_stat: SRUN 2, SSLEEP 3, SSTOP 4, SZOMB 5.
print({2: "R", 3: "S", 4: "T", 5: "Z"}.get(int.from_bytes(info.raw[4:8], "little"), "U"))
PY_STATE
}
home() { find "$1" -maxdepth 1 -type f | wc -l | tr -d ' '; }
# pause_part_way DIR PID: stop PID once some but not all files are home in DIR,
# confirm it is stopped, and count again. Prints the count; fails if the pop
# finished first.
pause_part_way() {
  local dir="$1" pid="$2" n s=""
  for _ in $(seq 1 2000); do
    n=$(home "$dir")
    if [ "$n" -gt 0 ] && [ "$n" -lt "$TOTAL" ] && kill -STOP "$pid" 2>/dev/null; then
      for _ in $(seq 1 300); do
        s=$(state "$pid") || { s=""; break; }
        [ "$s" != T ] || break
      done
      n=$(home "$dir")
      if [ "${s:-}" = T ] && [ "$n" -gt 0 ] && [ "$n" -lt "$TOTAL" ]; then
        printf '%s\n' "$n"; return 0
      fi
      kill -CONT "$pid" 2>/dev/null || true
    fi
    kill -0 "$pid" 2>/dev/null || return 1
    sleep 0.01
  done
  return 1
}

# no_false_claim LABEL TEXT: nothing in TEXT says the restore failed or stopped.
no_false_claim() {
  local bad
  bad=$(printf '%s\n' "$2" | grep -E -i 'interrupted|did not finish|could not restore|Fix that|then run:' | head -3)
  if [ -z "$bad" ]; then
    pass "$1: no claim of a failed or interrupted restore, and no step to fix"
  else
    fail "$1: false claim while every file is home: $bad"
  fi
}

trial() {  # trial MODE: MODE is text or json for the second pop
  local mode="$1" attempt D restored said code_a code_b out_a out_b
  for attempt in 1 2 3 4; do
    D="$W/$mode-$attempt/Notes"
    export ETUDE_STATE_DIR="$W/$mode-$attempt/state"
    mkdir -p "$ETUDE_STATE_DIR"
    make_tree "$D"
    snapshot_tree "$D" "$W/$mode-before.json"
    if ! "$STASH" "$D" >"$W/stash.out" 2>&1; then
      fail "$mode: setup stash exits 0: $(cat "$W/stash.out")"
      return
    fi

    "$STASH" pop "$D" >"$W/a.out" 2>"$W/a.err" &
    A=$!
    if restored=$(pause_part_way "$D" "$A"); then
      break
    fi
    kill -CONT "$A" 2>/dev/null || true
    wait "$A" 2>/dev/null || true
    A=""
    echo "    attempt  $mode: attempt $attempt did not stop the first pop part way"
  done
  if [ -z "$A" ]; then
    unproven "$mode: two pops at once" "the first pop finished before it could be stopped, in four attempts"
    return
  fi
  if [ "$(state "$A")" = T ]; then
    pass "$mode: first pop is stopped with $restored of $TOTAL files home"
  else
    fail "$mode: first pop is not confirmed stopped part way ($restored of $TOTAL home)"
  fi

  # The second pop starts while the first is held. Wait until it has exited or
  # said it is waiting; both are states, not times.
  if [ "$mode" = json ]; then
    "$STASH" pop --json "$D" >"$W/b.out" 2>"$W/b.err" &
  else
    "$STASH" pop "$D" >"$W/b.out" 2>"$W/b.err" &
  fi
  B=$!
  said=""
  for _ in $(seq 1 600); do
    if grep -q 'Waiting for it to finish' "$W/b.err" 2>/dev/null; then said=1; break; fi
    kill -0 "$B" 2>/dev/null || break
    sleep 0.05
  done
  if [ -n "$said" ] && kill -0 "$B" 2>/dev/null; then
    pass "$mode: second pop waits for the first while it is stopped, and says so"
  else
    fail "$mode: second pop did not wait for the first (still running: $(kill -0 "$B" 2>/dev/null && echo yes || echo no)): $(cat "$W/b.out" "$W/b.err" | grep -v -e 'items$' -e '^{' | head -5)"
  fi
  if grep -q 'another `stash pop` is restoring this stash now' "$W/b.err"; then
    pass "$mode: second pop names the other pop"
  else
    fail "$mode: second pop does not say another pop is restoring this stash: $(head -3 "$W/b.err")"
  fi

  kill -CONT "$A"
  wait "$A"; code_a=$?; A=""
  wait "$B"; code_b=$?; B=""
  out_a=$(cat "$W/a.out" "$W/a.err")
  out_b=$(cat "$W/b.out" "$W/b.err")

  assert_eq 0 "$code_a" "$mode: first pop exits 0 (exit $code_a)"
  if grep -qx "Restored $TOTAL items." "$W/a.out"; then
    pass "$mode: first pop says it restored all $TOTAL items"
  else
    fail "$mode: first pop does not say Restored $TOTAL items: $(grep -v 'items$' <<<"$out_a" | head -4)"
  fi
  assert_eq 1 "$code_b" "$mode: second pop exits 1, nothing to do (exit $code_b)"
  if grep -q 'Nothing to restore. Another `stash pop` restored this stash.' <<<"$out_b"; then
    pass "$mode: second pop says another pop restored the stash"
  else
    fail "$mode: second pop does not say another pop restored it: $(grep -v -e 'items$' -e '^{' <<<"$out_b" | head -4)"
  fi
  no_false_claim "$mode: first pop" "$out_a"
  no_false_claim "$mode: second pop" "$out_b"
  if [ "$mode" = json ]; then
    if python3 - "$W/b.out" <<'PY_JSON'
import json, sys
line = [l for l in open(sys.argv[1]) if l.startswith("{")][-1]
doc = json.loads(line)
counters = {c["name"]: c["count"] for c in doc["effects"]["counters"]}
ok = (doc["status"] == "nothing_to_do" and doc["reason"]["code"] == "nothing_to_do"
      and counters.get("items_restored", 0) == 0)
sys.exit(0 if ok else 1)
PY_JSON
    then
      pass "json: second pop --json gives status nothing_to_do, reason nothing_to_do, nothing restored"
    else
      fail "json: second pop --json does not agree with its text: $(tail -c 400 "$W/b.out")"
    fi
  fi

  snapshot_tree "$D" "$W/$mode-after.json"
  assert_snapshot_eq "$W/$mode-before.json" "$W/$mode-after.json" \
    "$mode: the folder equals the original: every file home, no holding folder left"

  out=$("$STASH" status "$D" 2>&1)
  if grep -q '^Nothing stashed in' <<<"$out" && ! grep -q 'stash pop' <<<"$out"; then
    pass "$mode: a later stash status says nothing is stashed"
  else
    fail "$mode: a later stash status still shows a stash: $(head -4 <<<"$out")"
  fi
  out=$("$STASH" pop "$D" 2>&1); code=$?
  if [ "$code" = 1 ] && ! grep -q '^Restored' <<<"$out"; then
    pass "$mode: a later stash pop has nothing to restore (exit 1)"
  else
    fail "$mode: a later stash pop exit $code: $(grep -v 'items$' <<<"$out" | head -4)"
  fi
  snapshot_tree "$D" "$W/$mode-later.json"
  assert_snapshot_eq "$W/$mode-before.json" "$W/$mode-later.json" "$mode: the later pop changed nothing"
}

trial text
trial json
