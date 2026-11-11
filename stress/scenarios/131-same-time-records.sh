#!/usr/bin/env bash
# Two undo records with the same modification time. sweep cannot tell which
# one is newer, so plain `sweep undo` and `sweep undo FOLDER` must refuse,
# move nothing, name each record and give a step per record. Each printed step
# must work, run exactly as printed, while the times are still the same.
# `sweep status` must still list the records, each tie marked, because status
# is where a person finds the ids. With one second between the times, plain
# undo takes the newer time again, whatever the ids say.
#
# An entry named like a record that is not a regular file is not a tie: it
# gets its own words, its own code, and a step that works.
#
# Stash: a pop by folder needs one live stash and no order, so a tie must not
# block it. The stash tie refusal itself needs legacy or rewritten ids, which
# a shell cannot seal; stash-cli's multi_batch tests write those.
#
# The times are set with os.utime, never by waiting, so nothing here depends on
# the clock or on how coarse this volume's times are.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir) || exit 1; trap 'rm -rf "${W:?}"' EXIT
W=$(cd "${W:?}" && pwd -P) && [ -n "$W" ] || exit 1
require python3 "these checks read JSON and set times with Python" || exit 0

has() { case "$2" in *"$3"*) pass "$1" ;; *) fail "$1: wanted '$3' in: $2" ;; esac; }
hasnt() { case "$2" in *"$3"*) fail "$1: found '$3' in: $2" ;; *) pass "$1" ;; esac; }
field() {  # field 'reason.code' FILE: a value, or a list joined by spaces
  python3 -I - "$1" "$2" <<'PY'
import json, sys
value = json.load(open(sys.argv[2]))
for key in sys.argv[1].split("."):
    value = value[int(key)] if isinstance(value, list) else value[key]
print(" ".join(map(str, value)) if isinstance(value, list) else value)
PY
}
same_time_ids() {  # same_time_ids FILE: the tied ids in details, one line
  python3 -I - "$1" <<'PY'
import json, sys
rows = json.load(open(sys.argv[1]))["details"]["same_time_records"]
print(" ".join(row["operation_id"] for row in rows))
print(" | ".join(row["command"] for row in rows), file=sys.stderr)
PY
}
set_time() {  # set_time SECONDS RECORD...: the exact modification time, in whole seconds
  python3 -I - "$@" <<'PY'
import os, sys
seconds = int(sys.argv[1])
for path in sys.argv[2:]:
    os.utime(path, ns=(seconds * 10**9, seconds * 10**9))
PY
}
record_ids() {  # record_ids TOOL: every record id of TOOL, sorted, one line
  local path ids=()
  for path in "$ETUDE_STATE_DIR/$1"-*.journal; do
    [ -f "$path" ] || continue
    path=${path##*/}; path=${path#"$1"-}; ids+=("${path%.journal}")
  done
  printf '%s\n' "${ids[@]}" | sort | tr '\n' ' ' | sed 's/ $//'
}
fixture() {  # fixture DIR: three old reports, which a plain apply groups
  local i
  mkdir -p "$1"
  for i in 1 2 3; do printf 'report %s\n' "$i" >"$1/report_$i.pdf"; done
  touch -t 202001010101 "$1"/*
}
apply() {  # apply DIR LABEL
  local out rc
  out=$("$SWEEP" apply "$1" --since 0 --yes 2>&1 </dev/null); rc=$?
  assert_eq 0 "$rc" "$2: apply exit"
}
sweep() { "$SWEEP" "$@"; }  # so a printed step runs exactly as printed
TIED=1800000000

# --- the tie: refused, nothing moved, each record named ---------------------
A="$W/it's a desk"; B="$W/downloads"
fixture "$A"; fixture "$B"
snapshot_tree "$A" "$W/a-original.json"; snapshot_tree "$B" "$W/b-original.json"
apply "$A" "first folder"; apply "$B" "second folder"
IDS=$(record_ids sweep)
assert_eq 2 "$(wc -w <<<"$IDS" | tr -d ' ')" "two applies left two undo records"
read -r ID1 ID2 <<<"$IDS"
set_time "$TIED" "$ETUDE_STATE_DIR"/sweep-*.journal
applied() {  # applied NAME: both folders and the records, in one manifest each
  snapshot_tree "$A" "$W/$1-a.json"; snapshot_tree "$B" "$W/$1-b.json"
  snapshot_tree "$ETUDE_STATE_DIR" "$W/$1-state.json"
}
applied before

OUT=$("$SWEEP" undo 2>&1 </dev/null); RC=$?
assert_no_panic "undo with a tie" "$OUT"
assert_eq 3 "$RC" "undo with a tie: exit 3"
has "undo with a tie: says why" "$OUT" \
  "sweep: 2 undo records have the same time, so sweep cannot tell which one is newer. Nothing was moved."
has "undo with a tie: a step for the first record" "$OUT" "To undo one of them, run: sweep undo --id $ID1"
has "undo with a tie: a step for the second record" "$OUT" "Or run: sweep undo --id $ID2"
has "undo with a tie: how to see a record's folder" "$OUT" "To see the folder of a record, run: sweep status --id $ID1"
hasnt "undo with a tie: no step-less advice" "$OUT" "fix this one"
OUT=$("$SWEEP" undo "$A" 2>&1 </dev/null); RC=$?
assert_eq 3 "$RC" "undo FOLDER with a tie: exit 3"
has "undo FOLDER with a tie: the same words" "$OUT" "2 undo records have the same time"
applied after
assert_snapshot_eq "$W/before-a.json" "$W/after-a.json" "the refusals moved nothing in the first folder"
assert_snapshot_eq "$W/before-b.json" "$W/after-b.json" "the refusals moved nothing in the second folder"
assert_snapshot_eq "$W/before-state.json" "$W/after-state.json" "the refusals left the records as they were"

"$SWEEP" undo --json >"$W/tie.json" 2>/dev/null </dev/null; RC=$?
assert_eq 3 "$RC" "json undo with a tie: exit 3"
assert_eq refused "$(field status "$W/tie.json")" "json: status refused"
assert_eq journal_times_tied "$(field reason.code "$W/tie.json")" "json: the tie has its own code"
assert_eq after_change "$(field reason.retry "$W/tie.json")" "json: retry after_change"
assert_eq journal "$(field reason.subject.kind "$W/tie.json")" "json: the subject is the journal"
assert_eq "$ID1 $ID2" "$(same_time_ids "$W/tie.json" 2>"$W/commands.txt")" "json: details name both records"
assert_eq "sweep undo --id $ID1 | sweep undo --id $ID2" "$(cat "$W/commands.txt")" "json: each record has its step"

# --- status still lists them, each tie marked ------------------------------
OUT=$("$SWEEP" status "$A" 2>&1 </dev/null); RC=$?
assert_eq 0 "$RC" "status FOLDER with a tie: exit 0"
has "status FOLDER: marks the tie" "$OUT" "This record has the same time as"
"$SWEEP" status "$A" --json >"$W/status.json" 2>/dev/null </dev/null
MINE=$(field details.operation_id "$W/status.json")
OTHER=$ID1; [ "$MINE" = "$ID1" ] && OTHER=$ID2
assert_eq "$OTHER" "$(field details.same_time_as "$W/status.json")" "status json: same_time_as names the other record"
for ID in $ID1 $ID2; do
  "$SWEEP" status --id "$ID" --json >/dev/null 2>&1 </dev/null; RC=$?
  assert_eq 0 "$RC" "status --id with a tie: exit 0"
done

# --- each printed step works while the times are the same ------------------
STEPS=$("$SWEEP" undo 2>&1 </dev/null | sed -n 's/.*run: //p')
assert_eq 3 "$(printf '%s\n' "$STEPS" | wc -l | tr -d ' ')" "three printed steps"
STATUS_STEP=$(printf '%s\n' "$STEPS" | sed -n 3p)
OUT=$(eval "$STATUS_STEP" 2>&1 </dev/null); RC=$?
assert_eq 0 "$RC" "the printed status step: exit 0"
has "the printed status step: names a folder" "$OUT" "folder: $W/"
for N in 1 2; do
  set_time "$TIED" "$ETUDE_STATE_DIR"/sweep-*.journal  # an undo rewrites its record
  STEP=$(printf '%s\n' "$STEPS" | sed -n "${N}p")
  OUT=$(eval "$STEP" 2>&1 </dev/null); RC=$?
  assert_no_panic "printed undo step $N" "$OUT"
  assert_eq 0 "$RC" "printed undo step $N: exit 0"
  has "printed undo step $N: restored" "$OUT" "Restored 3 files."
done
snapshot_tree "$A" "$W/a-after.json"; snapshot_tree "$B" "$W/b-after.json"
assert_snapshot_eq "$W/a-original.json" "$W/a-after.json" "the first folder equals its original"
assert_snapshot_eq "$W/b-original.json" "$W/b-after.json" "the second folder equals its original"

# --- no tie: plain undo takes the newer time, not the larger id -------------
rm -f "$ETUDE_STATE_DIR"/sweep-*.journal
C="$W/first"; D="$W/second"
fixture "$C"; fixture "$D"
apply "$C" "order: first"; CID=$(record_ids sweep)
apply "$D" "order: second"
set_time "$TIED" "$ETUDE_STATE_DIR/sweep-$CID.journal"
for path in "$ETUDE_STATE_DIR"/sweep-*.journal; do
  [ "$path" = "$ETUDE_STATE_DIR/sweep-$CID.journal" ] || set_time $((TIED - 1)) "$path"
done
OUT=$("$SWEEP" undo 2>&1 </dev/null); RC=$?
assert_eq 0 "$RC" "order: plain undo with one second between: exit 0"
[ -f "$C/report_1.pdf" ] && pass "order: the record with the newer time was undone" \
  || fail "order: the record with the newer time was not undone"
[ -d "$D/Documents" ] && pass "order: the older record was left" \
  || fail "order: the older record was undone too"

# --- not a regular file: its own words and a step that works ---------------
mkdir "$ETUDE_STATE_DIR/sweep-odd.journal"
OUT=$("$SWEEP" undo 2>&1 </dev/null); RC=$?
assert_eq 3 "$RC" "a folder named like a record: exit 3"
has "a folder named like a record: says what it is" "$OUT" \
  "sweep-odd.journal in it is not a regular file, so it is not an undo record. Nothing was moved."
hasnt "a folder named like a record: not called a tie" "$OUT" "same time"
"$SWEEP" undo --json >"$W/odd.json" 2>/dev/null </dev/null
assert_eq journal_unreadable "$(field reason.code "$W/odd.json")" "a folder named like a record: its own code"
STEP=$(printf '%s\n' "$OUT" | sed -n 's/.*no longer reads it: //p')
has "a folder named like a record: the step renames it" "$STEP" "mv -i "
OUT=$(eval "$STEP" 2>&1 </dev/null); RC=$?
assert_eq 0 "$RC" "the printed rename step: exit 0"
OUT=$("$SWEEP" undo 2>&1 </dev/null); RC=$?
assert_eq 0 "$RC" "undo after the rename step: exit 0"
[ -f "$D/report_1.pdf" ] && pass "undo after the rename step restored the folder" \
  || fail "undo after the rename step did not restore the folder"

# --- stash: a pop by folder is not blocked by a tie --------------------------
S1="$W/stash one"; S2="$W/stash two"
mkdir -p "$S1" "$S2"; printf 'one\n' >"$S1/one.txt"; printf 'two\n' >"$S2/two.txt"
snapshot_tree "$S1" "$W/s1-original.json"
"$STASH" "$S1" >/dev/null 2>&1 </dev/null; RC=$?; assert_eq 0 "$RC" "stash: first folder"
"$STASH" "$S2" >/dev/null 2>&1 </dev/null; RC=$?; assert_eq 0 "$RC" "stash: second folder"
set_time "$TIED" "$ETUDE_STATE_DIR"/stash-*.journal
OUT=$("$STASH" pop "$S1" 2>&1 </dev/null); RC=$?
assert_no_panic "stash pop FOLDER with a tie" "$OUT"
assert_eq 0 "$RC" "stash pop FOLDER with a tie: exit 0"
hasnt "stash pop FOLDER with a tie: not refused" "$OUT" "same time"
snapshot_tree "$S1" "$W/s1-after.json"
assert_snapshot_eq "$W/s1-original.json" "$W/s1-after.json" "stash: the folder equals its original"
[ ! -e "$S2/two.txt" ] && pass "stash: the other stash is still held" || fail "stash: the other stash was restored too"
