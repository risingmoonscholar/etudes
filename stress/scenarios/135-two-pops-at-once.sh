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
# pause_part_way DIR PID [MOST]: stop PID once some but at most MOST (default
# all but one) files are home in DIR, confirm it is stopped, and count again.
# Prints the count; fails if the run finished first.
pause_part_way() {
  local dir="$1" pid="$2" most="${3:-$((TOTAL - 1))}" n s=""
  for _ in $(seq 1 2000); do
    n=$(home "$dir")
    if [ "$n" -gt 0 ] && [ "$n" -le "$most" ] && kill -STOP "$pid" 2>/dev/null; then
      for _ in $(seq 1 300); do
        s=$(state "$pid") || { s=""; break; }
        [ "$s" != T ] || break
      done
      n=$(home "$dir")
      if [ "${s:-}" = T ] && [ "$n" -gt 0 ] && [ "$n" -le "$most" ]; then
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
  # Nothing moves while the first pop is stopped and the second waits.
  if [ "$(home "$D")" = "$restored" ]; then
    pass "$mode: while the second pop waits, it restores nothing ($restored of $TOTAL still home)"
  else
    fail "$mode: files moved while the first pop was stopped: $(home "$D") home, $restored when it stopped"
  fi
  if grep -q 'a `stash` or `stash pop` is running on this stash now' "$W/b.err"; then
    pass "$mode: second pop says a stash or pop is running on this stash"
  else
    fail "$mode: second pop does not say a stash or pop is running on this stash: $(head -3 "$W/b.err")"
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

# A second command that starts while the first pop is still running, not
# stopped. The pop appends to its record as it goes, so a read made then can
# find the record changing. That is not a damaged record, and nothing may say
# so. Where the two overlap is not under the scenario's control, so each case
# runs more than once, and the overlap is witnessed by the waiting line.
FALSE_DAMAGE='damaged|cannot be repaired|changed after discovery|cannot be opened'
no_false_damage() {  # LABEL FILE
  local bad
  bad=$(grep -E -i "$FALSE_DAMAGE" "$2" | head -2)
  if [ -z "$bad" ]; then
    pass "$1: no claim that a record is damaged or cannot be opened"
  else
    fail "$1: called a record being written damaged: $bad"
  fi
}
OVERLAP=0
live_case() {  # live_case N KIND: KIND is pop, pop-id-json, pop-latest, status-all, status, status-id
  local n="$1" kind="$2" D id code_a code_b
  D="$W/live-$n/Notes"
  export ETUDE_STATE_DIR="$W/live-$n/state"; mkdir -p "$ETUDE_STATE_DIR"
  make_tree "$D"
  snapshot_tree "$D" "$W/live-before.json"
  id=$("$STASH" "$D" 2>/dev/null | sed -n 's/.*Operation id: \([0-9a-f-]*\)\..*/\1/p')
  [ -n "$id" ] || { fail "live $n: setup stash printed no operation id"; return; }
  "$STASH" pop "$D" >"$W/a.out" 2>&1 &
  A=$!
  # Start the second command once the first has restored some files.
  for _ in $(seq 1 3000); do
    [ "$(home "$D")" -gt 100 ] && break
    kill -0 "$A" 2>/dev/null || break
    sleep 0.005
  done
  case "$kind" in
    pop) "$STASH" pop "$D" >"$W/b.out" 2>&1 ;;
    pop-id-json) "$STASH" pop --id "$id" --json >"$W/b.out" 2>&1 ;;
    pop-latest) "$STASH" pop --latest "$D" >"$W/b.out" 2>&1 ;;
    status-all) "$STASH" status --all >"$W/b.out" 2>&1 ;;
    status) "$STASH" status "$D" >"$W/b.out" 2>&1 ;;
    status-id) "$STASH" status --id "$id" >"$W/b.out" 2>&1 ;;
  esac
  code_b=$?
  wait "$A"; code_a=$?; A=""
  no_false_damage "live $n $kind" "$W/b.out"
  assert_eq 0 "$code_a" "live $n $kind: the first pop exits 0 (exit $code_a)"
  case "$kind" in
    pop*)
      if grep -q 'Waiting for it to finish' "$W/b.out"; then
        OVERLAP=$((OVERLAP + 1))
        if [ "$code_b" = 1 ] && grep -q 'Nothing to restore. Another `stash pop` restored this stash.' "$W/b.out"; then
          pass "live $n $kind: the second pop waited, then said another pop restored the stash (exit 1)"
        else
          fail "live $n $kind: the second pop waited, then exit $code_b: $(grep -v -e 'items$' -e '^{' "$W/b.out" | head -3)"
        fi
      else
        # The first pop finished before the second read anything.
        if [ "$code_b" = 1 ] && ! grep -q '^Restored' "$W/b.out"; then
          pass "live $n $kind: the second pop did not wait, and restores nothing (exit 1)"
        else
          fail "live $n $kind: the second pop did not wait, and exit $code_b: $(grep -v -e 'items$' -e '^{' "$W/b.out" | head -3)"
        fi
      fi
      no_false_claim "live $n $kind" "$(cat "$W/b.out")"
      ;;
    *)
      if [ "$code_b" != 3 ]; then
        pass "live $n $kind: status during a pop does not fail on the record (exit $code_b)"
      else
        fail "live $n $kind: status during a pop exit 3: $(head -3 "$W/b.out")"
      fi
      ;;
  esac
  snapshot_tree "$D" "$W/live-after.json"
  assert_snapshot_eq "$W/live-before.json" "$W/live-after.json" "live $n $kind: the folder equals its original"
}
n=0
for kind in pop pop-id-json pop-latest status-all status status-id pop pop-id-json pop-latest; do
  n=$((n + 1))
  live_case "$n" "$kind"
done
if [ "$OVERLAP" -gt 0 ]; then
  pass "a second pop overlapped a running first pop in $OVERLAP of 6 runs"
else
  unproven "a second pop during a running first pop" "no run overlapped in 6 tries"
fi

# A pop that starts while the stash itself is still running. The stash holds
# its record from its first recorded move, so the pop waits for it, then
# restores everything. It must not restore around the stash, or say the
# record is damaged or missing entries.
DURING=0
for n in 1 2 3; do
  D="$W/during-stash-$n/Notes"
  export ETUDE_STATE_DIR="$W/during-stash-$n/state"; mkdir -p "$ETUDE_STATE_DIR"
  make_tree "$D"
  snapshot_tree "$D" "$W/stash-before.json"
  "$STASH" "$D" >"$W/a.out" 2>&1 &
  A=$!
  for _ in $(seq 1 3000); do
    if ls "$ETUDE_STATE_DIR" 2>/dev/null | grep -q '^stash-.*\.journal$' && [ "$(home "$D")" -lt $((TOTAL - 100)) ]; then
      break
    fi
    kill -0 "$A" 2>/dev/null || break
    sleep 0.005
  done
  "$STASH" pop "$D" >"$W/b.out" 2>&1; code_b=$?
  wait "$A"; code_a=$?; A=""
  no_false_damage "pop during stash $n" "$W/b.out"
  assert_eq 0 "$code_a" "pop during stash $n: the stash exits 0 (exit $code_a)"
  grep -q 'Waiting for it to finish' "$W/b.out" && DURING=$((DURING + 1))
  if [ "$code_b" = 0 ] && grep -qx "Restored $TOTAL items." "$W/b.out" && ! grep -q -i 'missing' "$W/b.out"; then
    pass "pop during stash $n: the pop restored every item once the stash finished (exit 0)"
  else
    fail "pop during stash $n: exit $code_b: $(grep -v 'items$' "$W/b.out" | head -4)"
    "$STASH" pop "$D" >/dev/null 2>&1
  fi
  snapshot_tree "$D" "$W/stash-after.json"
  assert_snapshot_eq "$W/stash-before.json" "$W/stash-after.json" "pop during stash $n: the folder equals its original"
done
if [ "$DURING" -gt 0 ]; then
  pass "a pop started during a running stash and waited for it in $DURING of 3 runs"
else
  unproven "a pop during a running stash" "no pop overlapped the stash in 3 tries"
fi

# Status while a stash is stopped part way, after its first recorded move, so
# it holds its record. Every form says a run is going on now, with the counts
# so far, and none says damaged or interrupted. --json, whose states cannot
# say running, waits for the stash and then gives the finished state.
D="$W/status-during-stash/Notes"
export ETUDE_STATE_DIR="$W/status-during-stash/state"; mkdir -p "$ETUDE_STATE_DIR"
make_tree "$D"
"$STASH" "$D" >"$W/a.out" 2>&1 &
A=$!
if left=$(pause_part_way "$D" "$A" $((TOTAL - 2))); then
  pass "status during stash: the stash is stopped with $((TOTAL - left)) of $TOTAL moved"
  id=$(ls "$ETUDE_STATE_DIR" | sed -n 's/^stash-\(.*\)\.journal$/\1/p' | head -1)
  for form in folder all id; do
    case "$form" in
      folder) out=$("$STASH" status "$D" 2>&1); code=$? ;;
      all) out=$("$STASH" status --all 2>&1); code=$? ;;
      id) out=$("$STASH" status --id "$id" 2>&1); code=$? ;;
    esac
    if [ "$code" = 0 ] && grep -q 'a `stash` or `stash pop` is running on it now; so far' <<<"$out" \
        && ! grep -E -i -q 'damaged|interrupted|refuse' <<<"$out"; then
      pass "status during stash: status $form says a run is going on now, with counts so far (exit 0)"
    else
      fail "status during stash: status $form exit $code: $(head -3 <<<"$out")"
    fi
  done
  "$STASH" status --id "$id" --json >"$W/j.out" 2>"$W/j.err" &
  J=$!
  for _ in $(seq 1 600); do
    grep -q 'Waiting for it to finish' "$W/j.err" 2>/dev/null && break
    kill -0 "$J" 2>/dev/null || break
    sleep 0.05
  done
  if kill -0 "$J" 2>/dev/null && grep -q 'running on this stash now' "$W/j.err"; then
    pass "status during stash: status --id --json waits for the stash, and says so"
  else
    fail "status during stash: status --id --json did not wait: $(cat "$W/j.err" | head -2) $(tail -c 300 "$W/j.out")"
  fi
  kill -CONT "$A"
  wait "$A"; code=$?; A=""
  assert_eq 0 "$code" "status during stash: the stash exits 0 (exit $code)"
  wait "$J"; code=$?
  if [ "$code" = 0 ] && python3 - "$W/j.out" "$TOTAL" <<'PY_ID'
import json, sys
doc = json.loads([l for l in open(sys.argv[1]) if l.startswith("{")][-1])
d = doc.get("details", doc)
sys.exit(0 if d["state"] == "completed" and d["moved"] == int(sys.argv[2]) else 1)
PY_ID
  then
    pass "status during stash: status --id --json then gives state completed with every item moved"
  else
    fail "status during stash: status --id --json exit $code: $(tail -c 300 "$W/j.out")"
  fi
  "$STASH" pop "$D" >/dev/null 2>&1
else
  unproven "status during stash" "the stash finished before it could be stopped"
  wait "$A" 2>/dev/null; A=""
fi

# A pop --latest that waits never goes on to a stash made after it started.
# The first pop of one folder is stopped part way; pop --latest waits on it;
# meanwhile another folder is stashed. When the first pop finishes, pop
# --latest says that pop restored the stash, and the newer stash stays held.
X="$W/latest/First"; Y="$W/latest/Second"
export ETUDE_STATE_DIR="$W/latest/state"; mkdir -p "$ETUDE_STATE_DIR"
make_tree "$X"; make_tree "$Y"
"$STASH" "$X" >/dev/null 2>&1
"$STASH" pop "$X" >"$W/a.out" 2>&1 &
A=$!
if restored=$(pause_part_way "$X" "$A"); then
  pass "latest: the first pop is stopped with $restored of $TOTAL home"
  "$STASH" pop --latest >"$W/b.out" 2>&1 &
  B=$!
  for _ in $(seq 1 600); do
    grep -q 'Waiting for it to finish' "$W/b.out" 2>/dev/null && break
    kill -0 "$B" 2>/dev/null || break
    sleep 0.05
  done
  if kill -0 "$B" 2>/dev/null && grep -q 'running on this stash now' "$W/b.out"; then
    pass "latest: pop --latest waits on the stash being restored, and says so"
  else
    fail "latest: pop --latest did not wait: $(grep -v 'items$' "$W/b.out" | head -3)"
  fi
  "$STASH" "$Y" >"$W/s.out" 2>&1; code=$?
  assert_eq 0 "$code" "latest: another folder is stashed while pop --latest waits (exit $code)"
  kill -CONT "$A"
  wait "$A"; code=$?; A=""
  assert_eq 0 "$code" "latest: the first pop exits 0 (exit $code)"
  wait "$B"; code=$?; B=""
  if [ "$code" = 1 ] && grep -q 'Nothing to restore. Another `stash pop` restored this stash.' "$W/b.out"; then
    pass "latest: pop --latest says another pop restored the stash (exit 1)"
  else
    fail "latest: pop --latest exit $code: $(grep -v 'items$' "$W/b.out" | head -3)"
  fi
  assert_eq 0 "$(home "$Y")" "latest: the stash made after pop --latest started is still held"
  "$STASH" pop "$Y" >/dev/null 2>&1
  assert_eq "$TOTAL" "$(home "$Y")" "latest: that stash then restores by its folder"
else
  unproven "latest" "the first pop finished before it could be stopped"
  wait "$A" 2>/dev/null; A=""
fi

# pop --latest FOLDER while a pop of another folder runs. It may read the
# other record while it is written and wait; if it does, it says it is reading
# a record to choose a stash, not that its own stash is in use. It then
# restores its own folder.
for n in 1 2 3; do
  X="$W/other-$n/First"; Y="$W/other-$n/Second"
  export ETUDE_STATE_DIR="$W/other-$n/state"; mkdir -p "$ETUDE_STATE_DIR"
  make_tree "$X"; make_tree "$Y"
  "$STASH" "$Y" >/dev/null 2>&1
  "$STASH" "$X" >/dev/null 2>&1
  "$STASH" pop "$X" >"$W/a.out" 2>&1 &
  A=$!
  for _ in $(seq 1 3000); do
    [ "$(home "$X")" -gt 100 ] && break
    kill -0 "$A" 2>/dev/null || break
    sleep 0.005
  done
  "$STASH" pop --latest "$Y" >"$W/b.out" 2>&1; code=$?
  wait "$A"; A=""
  if [ "$code" = 0 ] && grep -qx "Restored $TOTAL items." "$W/b.out" \
      && ! grep -q 'running on this stash now' "$W/b.out"; then
    pass "other $n: pop --latest FOLDER restores its own folder, and never says its stash is in use"
  else
    fail "other $n: pop --latest FOLDER exit $code: $(grep -v 'items$' "$W/b.out" | head -3)"
  fi
  grep -q 'to choose a stash' "$W/b.out" && echo "    note     other $n: it waited while reading another folder's record"
  assert_eq "$TOTAL" "$(home "$X")" "other $n: the other pop restored its folder"
done
