#!/usr/bin/env bash
# Two `sweep undo` of one apply at the same time. The first undo is stopped part
# way, with some but not all files home and the kernel reporting it stopped, and
# the second starts while it is held. The second must wait for the first and
# then say the files are back. It must not restore around the stopped undo, and
# neither undo may report a failed or interrupted restore, or print a step to
# fix, for files that are home. A later status and undo agree that nothing is
# left. The undo is asked for by folder, with no folder, and by id. When the
# folder has two applies, the second undo must not go on to undo the older one:
# it was asked for one apply, and another undo did that one.
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

make_tree() {  # make_tree DIR [COUNT [PREFIX]]
  python3 - "$1" "${2:-$TOTAL}" "${3:-note}" <<'PY_TREE'
import os, pathlib, sys, time
d, n, prefix = pathlib.Path(sys.argv[1]), int(sys.argv[2]), sys.argv[3]
d.mkdir(parents=True, exist_ok=True)
old = time.time() - 3 * 86400
for i in range(n):
    p = d / f"{prefix} {i:04d}.txt"
    p.write_text(f"note {i}\n")
    os.utime(p, (old, old))
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
# pause_part_way DIR PID FROM [UPTO]: stop PID once more than FROM but fewer
# than UPTO (all) files are home in DIR, confirm it is stopped, and count
# again. Prints the count; fails if the undo finished first.
pause_part_way() {
  local dir="$1" pid="$2" from="$3" upto="${4:-$TOTAL}" n s=""
  for _ in $(seq 1 2000); do
    n=$(home "$dir")
    if [ "$n" -gt "$from" ] && [ "$n" -lt "$upto" ] && kill -STOP "$pid" 2>/dev/null; then
      for _ in $(seq 1 300); do
        s=$(state "$pid") || { s=""; break; }
        [ "$s" != T ] || break
      done
      n=$(home "$dir")
      if [ "${s:-}" = T ] && [ "$n" -gt "$from" ] && [ "$n" -lt "$upto" ]; then
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
  bad=$(printf '%s\n' "$2" | grep -E -i 'interrupted|did not finish|could not|no such file|appeared at the destination|taken by|Fix that|then run:' | head -3)
  if [ -z "$bad" ]; then
    pass "$1: no claim of a failed or interrupted restore, and no step to fix"
  else
    fail "$1: false claim while every file is home: $bad"
  fi
}

# status_row DIR: the state and counts of each record of DIR, from sweep status.
status_row() {
  "$SWEEP" status "$1" --json >"$W/status.json" 2>/dev/null </dev/null
  python3 -I - "$W/status.json" <<'PY_STATUS'
import json, sys
details = json.load(open(sys.argv[1]))["details"]
rows = details if isinstance(details, list) else [details]
print("; ".join("%s planned=%s moved=%s reversed=%s" % (
    r.get("state"), r["counts"]["planned"], r["counts"]["moved"], r["counts"]["reversed"])
    for r in rows))
PY_STATUS
}
# record_id DIR: the id of the one undo record of DIR, from sweep status.
record_id() {
  "$SWEEP" status "$1" --json >"$W/status.json" 2>/dev/null </dev/null
  python3 -I -c 'import json, sys; print(json.load(open(sys.argv[1]))["details"]["operation_id"])' "$W/status.json"
}

trial() {  # trial MODE: folder, json, newest or id
  local mode="$1" attempt D restored said code_a code_b out_a out_b id start
  local -a undo=()
  for attempt in 1 2 3 4; do
    D="$W/$mode-$attempt/Notes"
    export ETUDE_STATE_DIR="$W/$mode-$attempt/state"
    mkdir -p "$ETUDE_STATE_DIR"
    make_tree "$D"
    snapshot_tree "$D" "$W/$mode-before.json"
    if ! "$SWEEP" apply "$D" --since 0 --yes --json >"$W/apply.json" 2>"$W/apply.err" </dev/null; then
      fail "$mode: setup apply exits 0: $(head -3 "$W/apply.err")"
      return
    fi
    id=$(record_id "$D")
    start=$(home "$D")
    case "$mode" in
      folder|json) undo=(undo "$D") ;;
      newest) undo=(undo) ;;
      id) undo=(undo --id "$id") ;;
    esac

    "$SWEEP" "${undo[@]}" >"$W/a.out" 2>"$W/a.err" </dev/null &
    A=$!
    if restored=$(pause_part_way "$D" "$A" "$start"); then
      break
    fi
    kill -CONT "$A" 2>/dev/null || true
    wait "$A" 2>/dev/null || true
    A=""
    echo "    attempt  $mode: attempt $attempt did not stop the first undo part way"
  done
  if [ -z "$A" ]; then
    unproven "$mode: two undos at once" "the first undo finished before it could be stopped, in four attempts"
    return
  fi
  if [ "$(state "$A")" = T ]; then
    pass "$mode: first undo is stopped with $restored of $TOTAL files home"
  else
    fail "$mode: first undo is not confirmed stopped part way ($restored of $TOTAL home)"
  fi

  # The second undo starts while the first is held. Wait until it has exited
  # or said it is waiting; both are states, not times.
  if [ "$mode" = json ]; then
    "$SWEEP" undo --json "$D" >"$W/b.out" 2>"$W/b.err" </dev/null &
  else
    "$SWEEP" "${undo[@]}" >"$W/b.out" 2>"$W/b.err" </dev/null &
  fi
  B=$!
  said=""
  for _ in $(seq 1 600); do
    if grep -q 'Waiting for it to finish' "$W/b.err" 2>/dev/null; then said=1; break; fi
    kill -0 "$B" 2>/dev/null || break
    sleep 0.05
  done
  if [ -n "$said" ] && kill -0 "$B" 2>/dev/null; then
    pass "$mode: second undo waits for the first while it is stopped, and says so"
  else
    fail "$mode: second undo did not wait for the first (still running: $(kill -0 "$B" 2>/dev/null && echo yes || echo no)): $(cat "$W/b.out" "$W/b.err" | grep -v -e 'items$' -e '^{' | head -5)"
  fi
  # Nothing moves while the first undo is stopped and the second waits.
  if [ "$(home "$D")" = "$restored" ]; then
    pass "$mode: while the second undo waits, it restores nothing ($restored of $TOTAL still home)"
  else
    fail "$mode: files moved while the first undo was stopped: $(home "$D") home, $restored when it stopped"
  fi
  if grep -q 'another `sweep undo` is restoring these files now' "$W/b.err" \
    && grep -q 'Press Ctrl-C to stop waiting; that changes nothing.' "$W/b.err"; then
    pass "$mode: second undo names the other undo and says how to stop waiting"
  else
    fail "$mode: second undo does not say another undo is restoring these files: $(head -3 "$W/b.err")"
  fi

  kill -CONT "$A"
  wait "$A"; code_a=$?; A=""
  wait "$B"; code_b=$?; B=""
  out_a=$(cat "$W/a.out" "$W/a.err")
  out_b=$(cat "$W/b.out" "$W/b.err")

  assert_eq 0 "$code_a" "$mode: first undo exits 0 (exit $code_a)"
  if grep -qx "Restored $((TOTAL - start)) files." "$W/a.out"; then
    pass "$mode: first undo says it restored all $((TOTAL - start)) files it moved"
  else
    fail "$mode: first undo does not say Restored $((TOTAL - start)) files: $(grep -v 'items$' <<<"$out_a" | head -4)"
  fi
  assert_eq 1 "$code_b" "$mode: second undo exits 1, nothing to do (exit $code_b)"
  if grep -q 'Nothing to undo. Another `sweep undo` restored these files.' <<<"$out_b"; then
    pass "$mode: second undo says another undo restored the files"
  else
    fail "$mode: second undo does not say another undo restored them: $(grep -v -e 'items$' -e '^{' <<<"$out_b" | head -4)"
  fi
  no_false_claim "$mode: first undo" "$out_a"
  no_false_claim "$mode: second undo" "$out_b"
  if [ "$mode" = json ]; then
    if python3 -I - "$W/b.out" <<'PY_JSON'
import json, sys
line = [l for l in open(sys.argv[1]) if l.startswith("{")][-1]
doc = json.loads(line)
counters = {c["name"]: c["count"] for c in doc["effects"]["counters"]}
ok = (doc["status"] == "nothing_to_do" and doc["reason"]["code"] == "nothing_to_do"
      and counters.get("items_restored", 0) == 0
      and doc["details"].get("remaining_reversible") == 0)
sys.exit(0 if ok else 1)
PY_JSON
    then
      pass "json: second undo --json gives status nothing_to_do, reason nothing_to_do, nothing restored, nothing remaining"
    else
      fail "json: second undo --json does not agree with its text: $(tail -c 400 "$W/b.out")"
    fi
  fi

  snapshot_tree "$D" "$W/$mode-after.json"
  # Undo leaves the empty folder apply made only when it cannot remove it, so
  # the folder equals the original exactly.
  assert_snapshot_eq "$W/$mode-before.json" "$W/$mode-after.json" \
    "$mode: the folder equals the original: every file home, once"

  row=$(status_row "$D")
  if [ "$row" = "reversed planned=0 moved=0 reversed=$((TOTAL - start))" ]; then
    pass "$mode: a later sweep status says the apply is reversed ($row)"
  else
    fail "$mode: a later sweep status is not reversed with every file: $row"
  fi
  out=$("$SWEEP" "${undo[@]}" 2>&1 </dev/null); code=$?
  if [ "$code" = 1 ] && ! grep -q '^Restored' <<<"$out"; then
    pass "$mode: a third undo has nothing to undo (exit 1)"
  else
    fail "$mode: a third undo exit $code: $(grep -v 'items$' <<<"$out" | head -4)"
  fi
  snapshot_tree "$D" "$W/$mode-later.json"
  assert_snapshot_eq "$W/$mode-before.json" "$W/$mode-later.json" "$mode: the third undo changed nothing"
}

trial folder
trial json
trial newest
trial id

# Two applies of one folder: an older one of OLDER files, then a newer one of
# TOTAL more. Both undos ask for the folder, so both choose the newer apply.
# The second waits, and after the first restores that apply it must say so and
# stop. Going on would undo the older apply, which nobody asked this run to do.
two_applies() {
  local OLDER=300 attempt D restored said code_a code_b out_b start mid row before newer
  for attempt in 1 2 3 4; do
    D="$W/two-$attempt/Notes"
    export ETUDE_STATE_DIR="$W/two-$attempt/state"
    mkdir -p "$ETUDE_STATE_DIR"
    # The original: both kinds of file, built once more beside it.
    rm -rf "${W:?}/two-original"
    make_tree "$W/two-original" "$OLDER" old
    make_tree "$W/two-original" "$TOTAL" note
    snapshot_tree "$W/two-original" "$W/two-before.json"
    make_tree "$D" "$OLDER" old
    if ! "$SWEEP" apply "$D" --since 0 --yes >"$W/apply.out" 2>&1 </dev/null; then
      fail "two applies: setup older apply exits 0: $(head -3 "$W/apply.out")"
      return
    fi
    # Set the older record a minute back, so the two records never tie.
    python3 -I - "$ETUDE_STATE_DIR" <<'PY_OLDER'
import os, pathlib, sys, time
for p in pathlib.Path(sys.argv[1]).glob("sweep-*.journal"):
    os.utime(p, (time.time() - 60, time.time() - 60))
PY_OLDER
    make_tree "$D" "$TOTAL" note
    snapshot_tree "$D" "$W/two-between.json"
    before=$(home "$D")
    if ! "$SWEEP" apply "$D" --since 0 --yes >"$W/apply.out" 2>&1 </dev/null; then
      fail "two applies: setup newer apply exits 0: $(head -3 "$W/apply.out")"
      return
    fi
    start=$(home "$D")
    newer=$((before - start))
    mid=$(find "$W/two-$attempt" -name 'old *.txt' -path '*/Notes/*/*' | wc -l | tr -d ' ')
    "$SWEEP" undo "$D" >"$W/a.out" 2>"$W/a.err" </dev/null &
    A=$!
    if restored=$(pause_part_way "$D" "$A" "$start" "$before"); then
      break
    fi
    kill -CONT "$A" 2>/dev/null || true
    wait "$A" 2>/dev/null || true
    A=""
    echo "    attempt  two applies: attempt $attempt did not stop the first undo part way"
  done
  if [ -z "$A" ]; then
    unproven "two applies: two undos at once" "the first undo finished before it could be stopped, in four attempts"
    return
  fi
  if [ "$(state "$A")" = T ] && [ "$mid" -gt 0 ]; then
    pass "two applies: first undo is stopped part way, with the older apply's $mid files still moved"
  else
    fail "two applies: first undo is not confirmed stopped part way ($restored home, $mid older files moved)"
  fi
  "$SWEEP" undo "$D" >"$W/b.out" 2>"$W/b.err" </dev/null &
  B=$!
  said=""
  for _ in $(seq 1 600); do
    if grep -q 'Waiting for it to finish' "$W/b.err" 2>/dev/null; then said=1; break; fi
    kill -0 "$B" 2>/dev/null || break
    sleep 0.05
  done
  if [ -n "$said" ] && kill -0 "$B" 2>/dev/null && [ "$(home "$D")" = "$restored" ]; then
    pass "two applies: second undo waits and restores nothing while the first is stopped"
  else
    fail "two applies: second undo did not wait ($(home "$D") home, $restored when stopped): $(cat "$W/b.out" "$W/b.err" | grep -v 'items$' | head -4)"
  fi
  kill -CONT "$A"
  wait "$A"; code_a=$?; A=""
  wait "$B"; code_b=$?; B=""
  out_b=$(cat "$W/b.out" "$W/b.err")
  assert_eq 0 "$code_a" "two applies: first undo exits 0 (exit $code_a)"
  assert_eq 1 "$code_b" "two applies: second undo exits 1, nothing to do (exit $code_b)"
  if grep -q '^Restored' <<<"$out_b"; then
    fail "two applies: second undo restored files: $(grep '^Restored' <<<"$out_b")"
  else
    pass "two applies: second undo restored nothing"
  fi
  if grep -q 'Nothing to undo. Another `sweep undo` restored these files.' <<<"$out_b" \
    && grep -q 'Another apply of this folder can still be undone.' <<<"$out_b"; then
    pass "two applies: second undo says another undo restored these files, and that the older apply can still be undone"
  else
    fail "two applies: second undo does not say what happened: $(grep -v 'items$' <<<"$out_b" | head -4)"
  fi
  no_false_claim "two applies: second undo" "$(grep -v 'To undo it, run:' <<<"$out_b")"
  snapshot_tree "$D" "$W/two-after.json"
  assert_snapshot_eq "$W/two-between.json" "$W/two-after.json" \
    "two applies: the folder is as it was between the applies: the older apply is still in place"
  row=$(status_row "$D")
  case "$row" in
    *"reversed planned=0 moved=0 reversed=$newer"*"completed planned=0 moved=$mid reversed=0"* \
    | *"completed planned=0 moved=$mid reversed=0"*"reversed planned=0 moved=0 reversed=$newer"*)
      pass "two applies: sweep status says the newer apply is reversed and the older one is not ($row)" ;;
    *) fail "two applies: sweep status does not match the folder: $row" ;;
  esac
  out=$("$SWEEP" undo "$D" 2>&1 </dev/null); code=$?
  assert_eq 0 "$code" "two applies: a third undo undoes the older apply (exit $code)"
  snapshot_tree "$D" "$W/two-later.json"
  # The original was built beside the folder, so compare entries, not roots.
  if python3 -I - "$W/two-before.json" "$W/two-later.json" <<'PY_SAME'
import json, sys
a, b = (json.load(open(p))["entries"] for p in sys.argv[1:])
sys.exit(0 if a == b else 1)
PY_SAME
  then
    pass "two applies: the folder then equals its original"
  else
    fail "two applies: the folder does not equal its original after both undos (before=$W/two-before.json after=$W/two-later.json)"
  fi
}

two_applies
