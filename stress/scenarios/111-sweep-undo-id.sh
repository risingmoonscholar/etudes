#!/usr/bin/env bash
# `sweep undo --id ID` reverses exactly one apply and opens only that record, so
# one record that will not open cannot block the others. The refusal for such a
# record names the records that do open, each with its own step, and the steps
# work when followed. Undoing an older apply before a newer one never overwrites
# a file and never puts one in the wrong place: it restores, or stops and says so.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir) || exit 1; trap 'chmod -R u+rwx "${W:?}" 2>/dev/null; rm -rf "${W:?}"' EXIT
W=$(cd "${W:?}" && pwd -P) && [ -n "$W" ] || exit 1
require python3 "these checks read JSON and alter a record with Python" || exit 0
export HOME="$W/home"; mkdir -p "$HOME"
KEY_RIGHT="$ETUDE_JOURNAL_KEY"
KEY_OTHER=$(python3 -I -c 'import secrets; print(secrets.token_hex(32))')

BANNED='replan|journal seal|malformed|observations|listing failed|extractor|staging|publish|holding locator|holding parent|object identity|member row|dispatched|os error|panicked|[^0-9]1 records|[^0-9]1 files|record\(s\)'
clean() {  # clean LABEL OUT: no panic and no internal words, whatever the exit
  assert_no_panic "$1" "$2"
  if printf '%s' "$2" | grep -Eqi "$BANNED"; then
    fail "$1: internal word in: $2"
  else
    pass "$1: plain words"
  fi
}
has() { case "$2" in *"$3"*) pass "$1" ;; *) fail "$1: wanted '$3' in: $2" ;; esac; }
hasnt() { case "$2" in *"$3"*) fail "$1: found '$3' in: $2" ;; *) pass "$1" ;; esac; }
field() {  # field 'reason.code' FILE
  python3 -I - "$1" "$2" <<'PY'
import json, sys
value = json.load(open(sys.argv[2]))
for key in sys.argv[1].split("."):
    value = value[key]
print(value)
PY
}
fresh() {  # a new state folder, so ids and refusals from one case do not reach the next
  export ETUDE_STATE_DIR="$W/state-$1" ETUDE_JOURNAL_KEY="$KEY_RIGHT"
  mkdir -p "$ETUDE_STATE_DIR"
}
foos() {  # foos DIR WORD NAME...: files sweep groups only when told to with --map
  local dir="$1" word="$2"; shift 2
  mkdir -p "$dir"
  local name
  for name in "$@"; do printf '%s %s\n' "$word" "$name" >"$dir/$name.foo"; done
  touch -t 202001010101 "$dir"/*.foo
}
LAST_ID=""
apply() {  # apply DIR GROUP [ARG...]: apply, take the id from what it prints
  local dir="$1" group="$2" out rc; shift 2
  sleep 0.01
  out=$("$SWEEP" apply "$dir" --map foo="$group" --since 0 --yes "$@" 2>&1 </dev/null); rc=$?
  LAST_ID=$(printf '%s\n' "$out" | sed -n 's/^Undo record id: //p')
  if [ "$rc" = 0 ] && [ -n "$LAST_ID" ]; then
    pass "apply $group: prints the undo record id"
  else
    fail "apply $group: exit $rc, no id in: $out"
  fi
}
undo_id() {  # undo_id LABEL WANT ID: OUT holds the text
  local rc
  OUT=$("$SWEEP" undo --id "$3" 2>&1 </dev/null); rc=$?
  clean "$1" "$OUT"
  assert_eq "$2" "$rc" "$1: exit"
}
tree() { (cd "$1" && find . -mindepth 1 | LC_ALL=C sort | while IFS= read -r p; do
  if [ -f "$p" ]; then printf '%s = %s\n' "$p" "$(cat "$p")"; else printf '%s/\n' "$p"; fi
done); }
records() { (cd "$ETUDE_STATE_DIR" && ls -l | awk '{print $5, $9}'; shasum ./*.journal; stat -f '%m %N' ./*.journal 2>/dev/null || stat -c '%Y %n' ./*.journal); }
run_step() { eval "\"\$SWEEP\" ${1#sweep }"; }  # run_step 'PRINTED LINE': as a person would paste it

# --- arguments: refused before any state or key is touched -------------------
fresh args
BEFORE=$(ls -A "$ETUDE_STATE_DIR")
bad() {  # bad LABEL WORDS -- ARGS...
  local label="$1" words="$2" out rc; shift 3
  out=$("$SWEEP" undo "$@" 2>&1 </dev/null); rc=$?
  clean "$label" "$out"
  assert_eq 2 "$rc" "$label: exit"
  has "$label: says what" "$out" "$words"
  "$SWEEP" undo "$@" --json >"$W/out.json" 2>/dev/null </dev/null
  assert_eq invalid_arguments "$(field reason.code "$W/out.json")" "$label: reason"
  assert_eq refused "$(field status "$W/out.json")" "$label: status"
}
mkdir -p "$W/a-folder"
bad "id and a folder" "not both" -- --id 1-2-3-ab "$W/a-folder"
bad "a folder and an id" "not both" -- "$W/a-folder" --id 1-2-3-ab
bad "id repeated" "more than once" -- --id 1-2-3-ab --id 1-2-3-ac
bad "id empty" "needs a value" -- --id ""
bad "id with no value" "needs a value" -- --id
bad "id with a flag for a value" "needs a value" -- --id --yes
bad "id with a path in it" "not an undo record id" -- --id ../up
bad "id with a leading dash" "not an undo record id" -- --id -1
bad "id too long" "not an undo record id" -- --id "$(printf 'a%.0s' $(seq 1 200))"
assert_eq "$BEFORE" "$(ls -A "$ETUDE_STATE_DIR")" "bad arguments: no record appeared"
# After `--` the word is a folder, however it looks.
OUT=$(cd "$W/a-folder" && mkdir -p ./--id && "$SWEEP" undo -- --id 2>&1 </dev/null); RC=$?
clean "a folder called --id" "$OUT"
assert_eq 1 "$RC" "a folder called --id: exit, nothing on record"

# An empty name is refused, not shown as a blank.
for DRIVEN in "$SWEEP" "$STASH" "$UNPACK"; do
  TOOL=$(basename "$DRIVEN")
  OUT=$("$DRIVEN" '' 2>&1 </dev/null); RC=$?
  clean "empty name: $TOOL" "$OUT"
  assert_eq 2 "$RC" "empty name: $TOOL: exit"
  has "empty name: $TOOL: says it is empty" "$OUT" "name is empty"
  "$DRIVEN" '' --json >"$W/out.json" 2>/dev/null </dev/null
  assert_eq invalid_arguments "$(field reason.code "$W/out.json")" "empty name: $TOOL: reason"
done

for CMD in "sweep undo" "sweep status" "sweep review" "sweep apply" "stash pop" "stash status"; do
  OUT=$("$BIN/${CMD%% *}" ${CMD#* } '' 2>&1 </dev/null); RC=$?
  clean "empty name: $CMD" "$OUT"
  assert_eq 2 "$RC" "empty name: $CMD: exit"
  has "empty name: $CMD: says it is empty" "$OUT" "the folder name is empty"
  "$BIN/${CMD%% *}" ${CMD#* } '' --json >"$W/out.json" 2>/dev/null </dev/null
  assert_eq invalid_arguments "$(field reason.code "$W/out.json")" "empty name: $CMD: reason"
done

# --- an unknown id: the same answer as status --id and stash pop --id ----------
fresh unknown
UNKNOWN=1-2-3-ab
OUT=$("$SWEEP" undo --id "$UNKNOWN" 2>&1 </dev/null); RC=$?
clean "unknown id" "$OUT"
assert_eq 1 "$RC" "unknown id: exit"
has "unknown id: words" "$OUT" "unknown operation id"
has "unknown id: nothing moved" "$OUT" "Nothing was moved."
"$SWEEP" undo --id "$UNKNOWN" --json >"$W/undo.json" 2>/dev/null </dev/null
"$SWEEP" status --id "$UNKNOWN" --json >"$W/status.json" 2>/dev/null </dev/null
"$STASH" pop --id "$UNKNOWN" --json >"$W/pop.json" 2>/dev/null </dev/null
assert_eq unknown_operation "$(field reason.code "$W/undo.json")" "unknown id: reason"
assert_eq "$(field reason.code "$W/status.json")" "$(field reason.code "$W/undo.json")" "unknown id: same reason as status"
assert_eq "$(field reason.code "$W/pop.json")" "$(field reason.code "$W/undo.json")" "unknown id: same reason as stash pop"
assert_eq "$(field status "$W/pop.json")" "$(field status "$W/undo.json")" "unknown id: same status as stash pop"
# A stash id is not a sweep record.
S="$W/stash-tree"; mkdir -p "$S"; printf x >"$S/Screenshot 2026-01-01 at 10.00.00 AM.png"
STASH_ID=$("$STASH" "$S" --json 2>/dev/null </dev/null | python3 -I -c 'import json,sys; d=json.load(sys.stdin); print(d["recovery"]["operation"])')
OUT=$("$SWEEP" undo --id "$STASH_ID" 2>&1 </dev/null); RC=$?
assert_eq 1 "$RC" "a stash id: unknown to sweep"
assert_eq "" "$(cd "$S" && ls -d Screenshot* 2>/dev/null)" "a stash id: the stash stays stashed"

# --- older before newer, nothing shared but a folder ---------------------------
fresh shared
F="$W/shared"; foos "$F" one a1 a2 a3
ORIGINAL=$(tree "$F")
apply "$F" Group; OLDER=$LAST_ID
foos "$F" two b1 b2 b3
apply "$F" Group; NEWER=$LAST_ID
undo_id "older first" 0 "$OLDER"
has "older first: restored" "$OUT" "Restored 3 files."
assert_eq "one a1" "$(cat "$F/a1.foo" | head -1 | cut -d' ' -f1,2)" "older first: its file is home"
for n in b1 b2 b3; do
  assert_eq "two $n" "$(cat "$F/Group/$n.foo")" "older first: the newer file $n was not touched"
done
undo_id "then newer" 0 "$NEWER"
has "then newer: restored" "$OUT" "Restored 3 files."
for n in a1 a2 a3 b1 b2 b3; do
  if [ -f "$F/$n.foo" ]; then pass "then newer: $n is home"; else fail "then newer: $n is not home"; fi
done
# The folder belongs to the older record, which stayed out of the way of the
# newer one. Running it again finishes the job.
if [ -d "$F/Group" ]; then
  undo_id "older again removes its folder" 0 "$OLDER"
fi
if [ -d "$F/Group" ]; then fail "the empty folder was left for good"; else pass "the empty folder is gone"; fi
undo_id "older once more" 1 "$OLDER"
has "older once more: nothing to do" "$OUT" "Nothing to undo."
"$SWEEP" undo --id "$OLDER" --json >"$W/out.json" 2>/dev/null </dev/null
assert_eq nothing_to_do "$(field reason.code "$W/out.json")" "older once more: reason"

# --- a folder the older apply made, that only the newer apply still needs ------
fresh needs
F="$W/needs"; foos "$F" one a1 a2 a3
apply "$F" Group; OLDER=$LAST_ID
# Files dropped into the older apply's folder, which the newer apply moves out.
foos "$F/Group" two c1 c2 c3
apply "$F" Other --depth 2; NEWER=$LAST_ID
# The newer apply took everything, the older files included.
mv "$F/Other/a1.foo" "$F/Group/a1.foo"
mv "$F/Other/a2.foo" "$F/Group/a2.foo"
mv "$F/Other/a3.foo" "$F/Group/a3.foo"
undo_id "older folder still needed" 0 "$OLDER"
has "older folder still needed: says so" "$OUT" "Left 1 empty folder that another undo record may still need."
if [ -d "$F/Group" ]; then pass "older folder still needed: kept"; else fail "older folder still needed: removed under the newer record"; fi
OUT2=$("$SWEEP" undo --id "$NEWER" 2>&1 </dev/null); RC=$?
clean "newer record after the older" "$OUT2"
for n in c1 c2 c3; do
  if [ -f "$F/Group/$n.foo" ]; then pass "newer record: $n back in the folder it came from"; else fail "newer record: $n is not back: $OUT2"; fi
done

# --- the newer apply moved the older apply's files: older stops truthfully ------
fresh chain
F="$W/chain"; foos "$F" one a1 a2 a3
ORIGINAL=$(tree "$F")
apply "$F" Alpha; OLDER=$LAST_ID
apply "$F" Beta --depth 2; NEWER=$LAST_ID
MID=$(tree "$F")
undo_id "chain: older first" 3 "$OLDER"
has "chain: older first restores none" "$OUT" "Restored 0 of 3 files."
has "chain: names a later apply" "$OUT" "If a later apply moved them, undo that apply first"
has "chain: the retry names the record" "$OUT" "move it back and run: sweep undo --id $OLDER"
hasnt "chain: no folder step that could land on another apply" "$OUT" "run: sweep undo '"
if [ -d "$F/Alpha" ] && [ -d "$F/Beta" ]; then pass "chain: the folder the newer record needs is kept"; else fail "chain: a folder went missing"; fi
assert_eq "one a1" "$(cat "$F/Beta/a1.foo" | cut -d' ' -f1,2)" "chain: the file stayed with the newer record"
"$SWEEP" undo --id "$OLDER" --json >"$W/out.json" 2>/dev/null </dev/null
assert_eq incomplete "$(field status "$W/out.json")" "chain: status"
assert_eq operation_incomplete "$(field reason.code "$W/out.json")" "chain: reason"
has "chain: says what the listing covers" "$OUT" "lists only the applies of that folder"
hasnt "chain: no claim that it lists every apply" "$OUT" "lists each apply"
# Follow the printed way forward: status lists the applies of this folder, each with its own step.
LIST=$(eval "\"\$SWEEP\" status '$F'" 2>&1 </dev/null); RC=$?
assert_eq 0 "$RC" "chain: printed status step: exit"
has "chain: status names the older step" "$LIST" "sweep undo --id $OLDER"
has "chain: status names the newer step" "$LIST" "sweep undo --id $NEWER"
STEP=$(printf '%s\n' "$LIST" | grep "^$NEWER " | sed 's/.*; //')
assert_eq "sweep undo --id $NEWER" "$STEP" "chain: a status row's step names that row"
OUT=$(run_step "$STEP" 2>&1 </dev/null); RC=$?
clean "chain: newer step" "$OUT"
assert_eq 0 "$RC" "chain: newer step: exit"
has "chain: newer step restored" "$OUT" "Restored 3 files."
assert_eq "one a1" "$(cat "$F/Alpha/a1.foo" | cut -d' ' -f1,2)" "chain: the file is back where the newer apply found it"
OUT=$(run_step "sweep undo --id $OLDER" 2>&1 </dev/null); RC=$?
clean "chain: older retry" "$OUT"
assert_eq 0 "$RC" "chain: older retry: exit"
assert_eq "$ORIGINAL" "$(tree "$F")" "chain: followed in order, the folder is as it was"

# --- both records want the same place: never an overwrite ----------------------
fresh place
F="$W/place"; foos "$F" one a1 a2 a3
apply "$F" Group; OLDER=$LAST_ID
foos "$F" two a1 b2 b3
apply "$F" Other; NEWER=$LAST_ID
undo_id "same place: older" 0 "$OLDER"
assert_eq "one a1" "$(cut -d' ' -f1,2 <"$F/a1.foo")" "same place: the older file took its place"
undo_id "same place: newer" 3 "$NEWER"
has "same place: says it is taken" "$OUT" "taken by another file"
has "same place: the step names the record" "$OUT" "Move the other file away, then run: sweep undo --id $NEWER"
assert_eq "one a1" "$(cut -d' ' -f1,2 <"$F/a1.foo")" "same place: the older file was not overwritten"
assert_eq "two a1" "$(cut -d' ' -f1,2 <"$F/Other/a1.foo")" "same place: the newer file was left where it is"
mv "$F/a1.foo" "$W/moved-away.foo"
STEP=$(printf '%s\n' "$OUT" | sed -n 's/.*then run: //p' | head -1)
OUT=$(run_step "$STEP" 2>&1 </dev/null); RC=$?
assert_eq 0 "$RC" "same place: printed step: exit"
assert_eq "two a1" "$(cut -d' ' -f1,2 <"$F/a1.foo")" "same place: the newer file is home"
assert_eq "one a1" "$(cut -d' ' -f1,2 <"$W/moved-away.foo")" "same place: the file moved away is intact"

# --- the original place is a folder now: it is called a folder -------------------
fresh folder
F="$W/folder"; foos "$F" one a1 a2 a3
apply "$F" Group; ID=$LAST_ID
mkdir "$F/a1.foo"
undo_id "folder in the way" 3 "$ID"
has "folder in the way: says folder" "$OUT" "taken by another folder"
hasnt "folder in the way: not another file" "$OUT" "another file"
hasnt "folder in the way: no step to move a file" "$OUT" "Move the other file"
hasnt "folder in the way: no rmdir with a found name" "$OUT" "rmdir"
has "folder in the way: names the parent in full" "$OUT" "in $F, under the name <name.foo>"
has "folder in the way: says what to do" "$OUT" "If that folder is empty, remove it. If not, move it away."
STEP=$(printf '%s\n' "$OUT" | sed -n 's/.*Then run: //p' | head -1)
has "folder in the way: the step names the record" "$STEP" "sweep undo"
rmdir "$F/a1.foo"
OUT=$(run_step "$STEP" 2>&1 </dev/null); RC=$?
assert_eq 0 "$RC" "folder in the way: then undo: exit"
if [ -f "$F/a1.foo" ]; then pass "folder in the way: file is home"; else fail "folder in the way: file not home"; fi

# --- a newer file now sits where the older record put one: it is not taken ------
fresh wrong
F="$W/wrong"; foos "$F" one a1 a2 a3
apply "$F" Group; OLDER=$LAST_ID
# Same name, same bytes, same time, but another file.
cp -p "$F/Group/a1.foo" "$W/held.foo"; rm "$F/Group/a1.foo"
cp -p "$W/held.foo" "$F/a1.foo"
foos "$F" two b2 b3
touch -r "$W/held.foo" "$F/a1.foo"
apply "$F" Group; NEWER=$LAST_ID
assert_eq "one a1" "$(cut -d' ' -f1,2 <"$F/Group/a1.foo")" "look-alike: the newer apply took the same name"
undo_id "look-alike: older" 3 "$OLDER"
has "look-alike: left it alone" "$OUT" "1 file changed since apply and was left alone"
if [ -f "$F/Group/a1.foo" ]; then pass "look-alike: the newer record's file stays put"; else fail "look-alike: the older record took the newer record's file"; fi
undo_id "look-alike: newer" 0 "$NEWER"
if [ -f "$F/a1.foo" ]; then pass "look-alike: the newer record restores it"; else fail "look-alike: the newer record lost it"; fi

# --- a record under the wrong key: the way out, and nothing lost ----------------
fresh key
A="$W/key-a"; B="$W/key-b"; C="$W/key-c"
foos "$A" one a1 a2 a3
foos "$B" one b1 b2 b3
foos "$C" one c1 c2 c3
apply "$A" Group; ID_A=$LAST_ID
apply "$B" Group; ID_B=$LAST_ID
sleep 1
export ETUDE_JOURNAL_KEY="$KEY_OTHER"
apply "$C" Group; ID_C=$LAST_ID
export ETUDE_JOURNAL_KEY="$KEY_RIGHT"
BLOCKED_RECORD="$ETUDE_STATE_DIR/sweep-$ID_C.journal"
BLOCKED_SUM=$(shasum <"$BLOCKED_RECORD")
TREES=$(tree "$W/key-a"; tree "$W/key-b"; tree "$W/key-c")
HELD=$(records)
for form in "undo" "undo $A" "status $A"; do
  # shellcheck disable=SC2086
  OUT=$("$SWEEP" $form 2>&1 </dev/null); RC=$?
  clean "wrong key, sweep $form" "$OUT"
  assert_eq 3 "$RC" "wrong key, sweep $form: exit"
  has "wrong key, sweep $form: what it cannot know" "$OUT" "wrong key, or it was altered"
  has "wrong key, sweep $form: names the record" "$OUT" "$BLOCKED_RECORD"
  has "wrong key, sweep $form: step for one" "$OUT" "sweep undo --id $ID_B"
  has "wrong key, sweep $form: step for the other" "$OUT" "sweep undo --id $ID_A"
  has "wrong key, sweep $form: the folder" "$OUT" "$B"
  hasnt "wrong key, sweep $form: no step for the blocked record" "$OUT" "sweep undo --id $ID_C"
  case "$form" in
    undo*) has "wrong key, sweep $form: says nothing moved" "$OUT" "Nothing was moved." ;;
  esac
  newest_at=$(printf '%s\n' "$OUT" | grep -n "sweep undo --id $ID_B" | cut -d: -f1)
  oldest_at=$(printf '%s\n' "$OUT" | grep -n "sweep undo --id $ID_A" | cut -d: -f1)
  if [ "$newest_at" -lt "$oldest_at" ]; then pass "wrong key, sweep $form: newest first"; else fail "wrong key, sweep $form: order: $OUT"; fi
  LAST_REFUSAL="$OUT"
done
assert_eq "$TREES" "$(tree "$W/key-a"; tree "$W/key-b"; tree "$W/key-c")" "wrong key: nothing moved"
assert_eq "$HELD" "$(records)" "wrong key: reading the others changed no record"
"$SWEEP" undo --json >"$W/out.json" 2>/dev/null </dev/null
assert_eq refused "$(field status "$W/out.json")" "wrong key: status"
assert_eq journal_unreadable "$(field reason.code "$W/out.json")" "wrong key: reason"
python3 -I - "$W/out.json" "$ID_B" "$ID_A" "$A" <<'PY' && pass "wrong key: the JSON lists the same steps" || fail "wrong key: the JSON steps differ"
import json, sys
path, newer, older, folder = sys.argv[1:]
rows = json.load(open(path))["details"]["records_that_open"]
assert [row["operation_id"] for row in rows] == [newer, older]
assert rows[0]["command"] == f"sweep undo --id {newer}"
assert rows[1]["folder"] == folder
PY
assert_eq "$HELD" "$(records)" "wrong key: the JSON run changed no record"
# The blocked record, asked for by id, does not reach for any other.
OUT=$("$SWEEP" undo --id "$ID_C" 2>&1 </dev/null); RC=$?
clean "wrong key, by its id" "$OUT"
assert_eq 3 "$RC" "wrong key, by its id: exit"
has "wrong key, by its id: what it cannot know" "$OUT" "wrong key, or it was altered"
has "wrong key, by its id: keep it" "$OUT" "Keep that record"
hasnt "wrong key, by its id: no search" "$OUT" "sweep undo --id $ID_A"
assert_eq "$HELD" "$(records)" "wrong key, by its id: no record changed"
# Follow the printed step for the older folder, past the blocked record.
STEP=$(printf '%s\n' "$LAST_REFUSAL" | sed -n "s/^ *\(sweep undo --id $ID_A\)$/\1/p")
assert_eq "sweep undo --id $ID_A" "$STEP" "wrong key: the step stands on its own line"
OUT=$(run_step "$STEP" 2>&1 </dev/null); RC=$?
clean "wrong key: followed step" "$OUT"
assert_eq 0 "$RC" "wrong key: followed step: exit"
has "wrong key: followed step restored" "$OUT" "Restored 3 files."
if [ -f "$A/a1.foo" ]; then pass "wrong key: that folder's files are home"; else fail "wrong key: that folder's files did not come home"; fi
if [ -d "$A/Group" ]; then pass "wrong key: a folder an unopened record may need is kept"; else fail "wrong key: the folder was removed with a record unread"; fi
has "wrong key: it says the folder was kept" "$OUT" "may still need"
assert_eq "$BLOCKED_SUM" "$(shasum <"$BLOCKED_RECORD")" "wrong key: the blocked record is unchanged"
# The record under the other key was good all along. With its key, everything
# comes back: the one it holds and the ones the first key holds.
export ETUDE_JOURNAL_KEY="$KEY_OTHER"
OUT=$("$SWEEP" undo --id "$ID_C" 2>&1 </dev/null); RC=$?
clean "right key restores its record" "$OUT"
assert_eq 0 "$RC" "right key restores its record: exit"
has "right key restores its record: restored" "$OUT" "Restored 3 files."
for n in c1 c2 c3; do assert_eq "one $n" "$(cat "$C/$n.foo")" "right key: $n is home"; done
export ETUDE_JOURNAL_KEY="$KEY_RIGHT"
OUT=$("$SWEEP" undo --id "$ID_B" 2>&1 </dev/null); RC=$?
assert_eq 0 "$RC" "first key restores the other: exit"
if [ -f "$B/b1.foo" ]; then pass "first key: its files are home"; else fail "first key: its files did not come home"; fi

# --- damage reads the same, because it cannot be told apart --------------------
fresh damaged
A="$W/damaged-a"; B="$W/damaged-b"
foos "$A" one a1 a2 a3; foos "$B" one b1 b2 b3
apply "$A" Group; ID_A=$LAST_ID
sleep 1
apply "$B" Group; ID_B=$LAST_ID
python3 -I - "$ETUDE_STATE_DIR/sweep-$ID_B.journal" <<'PY'
import sys
path = sys.argv[1]
data = bytearray(open(path, "rb").read())
data[20] ^= 0xff
open(path, "wb").write(data)
PY
OUT=$("$SWEEP" undo 2>&1 </dev/null); RC=$?
clean "altered record" "$OUT"
assert_eq 3 "$RC" "altered record: exit"
has "altered record: the same words as a wrong key" "$OUT" "wrong key, or it was altered"
hasnt "altered record: does not claim which" "$OUT" "it is damaged"
has "altered record: the way out" "$OUT" "sweep undo --id $ID_A"
OUT=$(run_step "sweep undo --id $ID_A" 2>&1 </dev/null); RC=$?
assert_eq 0 "$RC" "altered record: the step works"

# --- none open: no step at all ---------------------------------------------------
fresh none
A="$W/none-a"; B="$W/none-b"
foos "$A" one a1 a2 a3; foos "$B" one b1 b2 b3
A_ORIGINAL=$(tree "$A"); B_ORIGINAL=$(tree "$B")
apply "$A" Group
sleep 1
apply "$B" Group
for form in "undo" "undo $A" "status $A"; do
  # shellcheck disable=SC2086
  OUT=$(ETUDE_JOURNAL_KEY="$KEY_OTHER" "$SWEEP" $form 2>&1 </dev/null); RC=$?
  clean "none open, sweep $form" "$OUT"
  assert_eq 3 "$RC" "none open, sweep $form: exit"
  hasnt "none open, sweep $form: no step" "$OUT" "sweep undo --id"
  has "none open, sweep $form: names the other record too" "$OUT" "cannot be opened"
  has "none open, sweep $form: says none opens" "$OUT" "No other undo record opens with this key."
done
# With the right key everything comes back, with the plain forms too: a wrong
# key was never damage, and nothing was removed along the way.
OUT=$("$SWEEP" undo 2>&1 </dev/null); RC=$?
clean "none open, then the right key" "$OUT"
assert_eq 0 "$RC" "none open, then the right key: plain undo"
OUT=$("$SWEEP" undo "$A" 2>&1 </dev/null); RC=$?
assert_eq 0 "$RC" "none open, then the right key: undo by folder"
assert_eq "$A_ORIGINAL" "$(tree "$A")" "none open, then the right key: one folder as it was"
assert_eq "$B_ORIGINAL" "$(tree "$B")" "none open, then the right key: the other as it was"

# --- only spent records open: no step, and the words say why --------------------
fresh spent
A="$W/spent-a"; B="$W/spent-b"
foos "$A" one a1 a2 a3; foos "$B" one b1 b2 b3
apply "$A" Group; ID_A=$LAST_ID
"$SWEEP" undo --id "$ID_A" >/dev/null 2>&1 </dev/null
sleep 1
export ETUDE_JOURNAL_KEY="$KEY_OTHER"; apply "$B" Group; export ETUDE_JOURNAL_KEY="$KEY_RIGHT"
OUT=$("$SWEEP" undo 2>&1 </dev/null); RC=$?
clean "only spent records" "$OUT"
assert_eq 3 "$RC" "only spent records: exit"
hasnt "only spent records: no step" "$OUT" "sweep undo --id"
has "only spent records: says so" "$OUT" "The other undo records that open have nothing left to undo."

# --- five are listed, and the rest are counted ----------------------------------
fresh many
IDS=()
for i in 1 2 3 4 5 6 7; do
  foos "$W/many-$i" one a1 a2 a3
  apply "$W/many-$i" Group
  IDS+=("$LAST_ID")
done
sleep 1
foos "$W/many-blocked" one a1 a2 a3
export ETUDE_JOURNAL_KEY="$KEY_OTHER"; apply "$W/many-blocked" Group; export ETUDE_JOURNAL_KEY="$KEY_RIGHT"
OUT=$("$SWEEP" undo 2>&1 </dev/null); RC=$?
clean "many records" "$OUT"
assert_eq 3 "$RC" "many records: exit"
assert_eq 5 "$(printf '%s\n' "$OUT" | grep -c 'sweep undo --id')" "many records: five steps"
has "many records: the newest is first" "$(printf '%s\n' "$OUT" | grep 'sweep undo --id' | head -1)" "${IDS[6]}"
has "many records: the fifth newest is last" "$(printf '%s\n' "$OUT" | grep 'sweep undo --id' | tail -1)" "${IDS[2]}"
has "many records: says how many more" "$OUT" "2 more undo records open, not listed."

# --- one more is singular -----------------------------------------------------
fresh singular
for i in 1 2 3 4 5 6; do
  foos "$W/singular-$i" one a1 a2 a3
  apply "$W/singular-$i" Group
done
sleep 1
foos "$W/singular-blocked" one a1 a2 a3
export ETUDE_JOURNAL_KEY="$KEY_OTHER"; apply "$W/singular-blocked" Group; export ETUDE_JOURNAL_KEY="$KEY_RIGHT"
OUT=$("$SWEEP" undo 2>&1 </dev/null)
has "one more: singular" "$OUT" "1 more undo record opens, not listed."

# --- status --id gives the id and the step that uses it -------------------------
fresh status
F="$W/status"; foos "$F" one a1 a2 a3
apply "$F" Group; ID=$LAST_ID
OUT=$("$SWEEP" status --id "$ID" 2>&1 </dev/null)
has "status --id: names the id" "$OUT" "$ID completed"
has "status --id: the step uses it" "$OUT" "sweep undo --id $ID"
"$SWEEP" status --id "$ID" --json >"$W/out.json" 2>/dev/null </dev/null
assert_eq "sweep undo --id $ID" "$(field details.command "$W/out.json")" "status --id: the JSON step"
assert_eq "$F" "$(field details.folder "$W/out.json")" "status --id: the JSON folder"
has "status --id: the folder undo --id touches" "$OUT" "folder: $F"

# Follow it.
OUT=$("$SWEEP" undo --id "$ID" 2>&1 </dev/null); RC=$?
clean "status step followed" "$OUT"
assert_eq 0 "$RC" "status step followed: exit"
if [ -f "$F/a1.foo" ]; then pass "status step followed: home"; else fail "status step followed: not home"; fi

