#!/usr/bin/env bash
# Every remaining stash failure says what failed, whether anything moved, and a
# next step that works. Steps that are the point of a fix are followed here. Exit
# 0 from pop means every item came back.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir) || exit 1
MOUNTS=()
cleanup() {
  for m in ${MOUNTS[@]+"${MOUNTS[@]}"}; do hdiutil detach "$m" -force >/dev/null 2>&1; done
  chflags -R nouchg "$W" 2>/dev/null; chmod -R u+rwx "$W" 2>/dev/null; rm -rf "$W"
}
trap cleanup EXIT
W=$(cd "${W:?}" && pwd -P) && [ -n "$W" ] || exit 1
if [ "$(id -u)" = 0 ]; then
  unproven "stash messages" "root ignores file modes"
  exit 0
fi
export HOME="$W/home" SWEEP_GRACE_SECS=0
mkdir -p "$HOME"
fresh_state() {  # older records must not get in the way of this case
  export ETUDE_STATE_DIR="$W/$1-state"; mkdir -p "$ETUDE_STATE_DIR"
}
files() {  # files DIR: three plain files
  mkdir -p "$1"
  for n in a b c; do printf "$n" > "$1/$n.txt"; done
}
BANNED='replan|journal seal|malformed|observations|listing failed|extractor|staging|publish|holding locator|holding parent|holding storage|object identity|selected object|member row|dispatched|os error|panicked|day\(s\)|1 items|1 operations'
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
# say LABEL EXIT CODE WORDS -- COMMAND...: text run, then the same with --json. Only
# for commands that change nothing when they fail. LAST holds the text.
say() {
  local label="$1" exit="$2" code="$3" words="$4"; shift 5
  local out rc
  out=$("$@" 2>&1 </dev/null); rc=$?
  clean "$label" "$out"
  assert_eq "$exit" "$rc" "$label: exit"
  has "$label: words" "$out" "$words"
  if [ "$code" != - ]; then
    "$@" --json >"$W/out.json" 2>"$W/out.err" </dev/null
    assert_eq "$code" "$(field reason.code "$W/out.json")" "$label: reason"
    clean "$label: json stderr" "$(cat "$W/out.err")"
  fi
  LAST="$out"
}
stashed() {  # stashed ARGS...: stash and print the id
  "$STASH" "$@" --json 2>/dev/null </dev/null \
    | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["details"]["stash_id"])'
}
selected() {  # selected ARGS...: stash select and print the id
  "$STASH" select "$@" --json 2>/dev/null </dev/null \
    | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["details"]["stash_id"])'
}
cut_to_base() {  # every progress record gone, on a clean boundary
  local keep
  keep=$(( 4 + $(python3 -I -c "
import struct,sys
print(struct.unpack('<I', open(sys.argv[1],'rb').read()[0:4])[0])" "$1") ))
  head -c "$keep" "$1" > "$1.cut"; mv "$1.cut" "$1"
}
intact() {  # intact DIR COUNT LABEL: DIR holds COUNT regular files directly
  assert_eq "$2" "$(find "$1" -maxdepth 1 -type f | wc -l | tr -d ' ')" "$3"
}

# --- pop that leaves something behind (rows 6, 7) -----------------------------
fresh_state left
D="$W/changed"; files "$D"; ID=$(stashed "$D")
H=$(ls -d "$D"/.stash-*); printf more >> "$H/a.txt"
OUT=$("$STASH" pop "$D" 2>&1 </dev/null); RC=$?
clean "changed" "$OUT"
assert_eq 3 "$RC" "changed: not done, so not exit 0"
has "changed: counts" "$OUT" "Restored 2 of 3 items."
has "changed: says it changed" "$OUT" "1 item changed while stashed and was left alone."
has "changed: says where" "$OUT" "$H/<name.txt>"
has "changed: a step" "$OUT" "stash inspect --id $ID"
assert_exit 0 "changed: the printed step works" -- "$STASH" inspect --id "$ID"

fresh_state occupied
D="$W/occupied"; files "$D"; ID=$(stashed "$D"); printf other > "$D/b.txt"
"$STASH" pop "$D" --json >"$W/out.json" 2>/dev/null </dev/null; RC=$?
assert_eq 3 "$RC" "occupied json: exit"
assert_eq incomplete "$(field status "$W/out.json")" "occupied: status"
assert_eq operation_incomplete "$(field reason.code "$W/out.json")" "occupied: reason"
assert_eq 1 "$(field details.skipped_occupied "$W/out.json")" "occupied: its own count"
assert_eq 0 "$(field details.skipped_changed "$W/out.json")" "occupied: not called changed"
OUT=$("$STASH" pop "$D" 2>&1 </dev/null); RC=$?
clean "occupied" "$OUT"
assert_eq 3 "$RC" "occupied: exit"
has "occupied: the cause" "$OUT" "1 item was not restored because its original place is taken by another file."
hasnt "occupied: not changed" "$OUT" "changed while"
has "occupied: a step naming the id" "$OUT" "then run: stash pop --id $ID"
assert_eq other "$(cat "$D/b.txt")" "occupied: the other file is kept"
rm "$D/b.txt"
assert_exit 0 "occupied: the printed step works" -- "$STASH" pop --id "$ID"
assert_eq b "$(cat "$D/b.txt")" "occupied: the held file came back"

fresh_state moved
D="$W/moved"; files "$D"; ID=$(stashed "$D")
H=$(ls -d "$D"/.stash-*); mv "$H" "$W/moved-away"
OUT=$("$STASH" pop --id "$ID" 2>&1 </dev/null); RC=$?
clean "moved" "$OUT"
assert_eq 3 "$RC" "moved: exit"
has "moved: counts" "$OUT" "Restored 0 of 3 items."
has "moved: says where they should be" "$OUT" "3 items are not where stash left them ($H)."
hasnt "moved: not gone" "$OUT" "already gone"
"$STASH" pop --id "$ID" --json >"$W/out.json" 2>/dev/null </dev/null
assert_eq incomplete "$(field status "$W/out.json")" "moved: status"
assert_eq operation_incomplete "$(field reason.code "$W/out.json")" "moved: reason"
mv "$W/moved-away" "$H"
assert_exit 0 "moved: moving it back and running the step works" -- "$STASH" pop --id "$ID"
intact "$D" 3 "moved: all back"

# --- undo records cut short (rows 13, 46, 121) --------------------------------
fresh_state cut
D="$W/cut"; files "$D"; ID=$(stashed "$D")
RECORD="$ETUDE_STATE_DIR/stash-$ID.journal"; H=$(ls -d "$D"/.stash-*)
cut_to_base "$RECORD"
OUT=$("$STASH" status "$D" 2>&1 </dev/null); RC=$?
clean "cut: status" "$OUT"
has "cut: status warns" "$OUT" "this undo record is damaged, and stash pop will refuse it"
hasnt "cut: status offers no refused step" "$OUT" "restore with stash pop"
OUT=$("$STASH" status --all 2>&1 </dev/null)
clean "cut: status all" "$OUT"
has "cut: status all warns" "$OUT" "this undo record is damaged"
ITEM=$("$STASH" inspect --id "$ID" --json 2>/dev/null </dev/null \
  | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["details"]["items"][0]["id"])')
snapshot_tree "$D" "$W/cut.snap"
OUT=$("$STASH" pop --id "$ID" --item "$ITEM" 2>&1 </dev/null); RC=$?
clean "cut: item" "$OUT"
assert_eq 3 "$RC" "cut: item exit"
has "cut: item says nothing moved" "$OUT" "so it moved nothing"
OUT=$("$STASH" pop "$D" 2>&1 </dev/null); RC=$?
clean "cut: pop" "$OUT"
assert_eq 3 "$RC" "cut: pop exit"
hasnt "cut: does not first say it is restoring" "$OUT" "Pop restores everything"
has "cut: nothing moved" "$OUT" "so it moved nothing"
has "cut: where they are" "$OUT" "$H"
has "cut: the step by hand" "$OUT" "move them back by hand into $D."
snapshot_tree "$D" "$W/cut-after.snap"; assert_snapshot_eq "$W/cut.snap" "$W/cut-after.snap" "cut: nothing moved"
"$STASH" pop "$D" --json >"$W/out.json" 2>/dev/null </dev/null
assert_eq refused "$(field status "$W/out.json")" "cut: status refused"
assert_eq operation_incomplete "$(field reason.code "$W/out.json")" "cut: reason"
mv "$H"/* "$D/" && intact "$D" 3 "cut: moving them by hand works"

# A cut inside the last record loses one move, and pop goes ahead.
fresh_state torn
D="$W/torn"; files "$D"; ID=$(stashed "$D")
RECORD="$ETUDE_STATE_DIR/stash-$ID.journal"
SIZE=$(stat -f%z "$RECORD"); head -c $((SIZE - 3)) "$RECORD" > "$RECORD.cut"; mv "$RECORD.cut" "$RECORD"
OUT=$("$STASH" status "$D" 2>&1 </dev/null)
clean "torn: status" "$OUT"
has "torn: status warns" "$OUT" "this undo record is damaged"
has "torn: status keeps the step that works" "$OUT" "restore with stash pop --id $ID"
OUT=$("$STASH" pop --id "$ID" 2>&1 </dev/null); RC=$?
clean "torn: pop" "$OUT"
assert_eq 0 "$RC" "torn: exit"
has "torn: says it" "$OUT" "this undo record is damaged: it was cut short"
intact "$D" 3 "torn: all back"

# --- one record that will not open (rows 15, 83) ------------------------------
fresh_state unopened
D="$W/own"; files "$D"; ID=$(stashed "$D")
BAD="1000000000000000000-1-0-abcdef"
printf garbage > "$ETUDE_STATE_DIR/stash-$BAD.journal"; chmod 600 "$ETUDE_STATE_DIR/stash-$BAD.journal"
say "unopened: pop folder" 3 journal_unreadable "cannot be opened" -- "$STASH" pop "$D"
has "unopened: names the record" "$LAST" "stash-$BAD.journal"
has "unopened: nothing restored" "$LAST" "nothing was restored"
has "unopened: the way past it" "$LAST" "stash pop --id ID"
intact "$D" 0 "unopened: nothing moved"
say "unopened: status folder" 3 journal_unreadable "cannot be opened" -- "$STASH" status "$D"
say "unopened: status all" 3 journal_unreadable "cannot be opened" -- "$STASH" status --all
has "unopened: status all still lists the one that opens" "$LAST" "$ID"
"$STASH" status --all --json >"$W/out.json" 2>/dev/null </dev/null
assert_eq "['$BAD']" "$(field details.unreadable "$W/out.json")" "unopened: json names the bad record"
assert_eq "$ID" "$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["details"]["stashes"][0]["id"])' "$W/out.json")" "unopened: json lists the good one"
say "unopened: pop its id" 3 journal_unreadable "cannot be opened" -- "$STASH" pop --id "$BAD"
has "unopened: pop id says nothing restored" "$LAST" "Nothing was restored."
say "unopened: inspect its id" 3 journal_unreadable "cannot be opened" -- "$STASH" inspect --id "$BAD"
say "unopened: status its id" 3 journal_unreadable "cannot be opened" -- "$STASH" status --id "$BAD"
assert_exit 0 "unopened: the printed step works" -- "$STASH" pop --id "$ID"
intact "$D" 3 "unopened: own stash back"

# --- pop cannot write (rows 20, 123) ------------------------------------------
fresh_state readonly
D="$W/readonly"; files "$D"; ID=$(stashed "$D"); chmod 555 "$D"
OUT=$("$STASH" pop --id "$ID" 2>&1 </dev/null); RC=$?
clean "read-only folder" "$OUT"
assert_eq 3 "$RC" "read-only folder: exit"
has "read-only folder: names it and the cause" "$OUT" "could not restore into $D: permission denied."
has "read-only folder: the step" "$OUT" "then run: stash pop --id $ID"
hasnt "read-only folder: no bare pop" "$OUT" "\`stash pop\`"
"$STASH" pop --id "$ID" --json >"$W/out.json" 2>/dev/null </dev/null
assert_eq incomplete "$(field status "$W/out.json")" "read-only folder: status"
assert_eq operation_incomplete "$(field reason.code "$W/out.json")" "read-only folder: reason"
chmod 755 "$D"
assert_exit 0 "read-only folder: the printed step works" -- "$STASH" pop --id "$ID"
intact "$D" 3 "read-only folder: all back"

fresh_state gone
P="$W/parent"; mkdir -p "$P"; printf x > "$P/one"; ID=$(selected "$P/one" --into "$W")
mv "$P" "$W/parent2"
OUT=$("$STASH" pop --id "$ID" 2>&1 </dev/null); RC=$?
clean "selection parent moved" "$OUT"
assert_eq 3 "$RC" "selection parent moved: exit"
has "selection parent moved: names it" "$OUT" "could not restore into $P:"
mv "$W/parent2" "$P"
assert_exit 0 "selection parent moved: moving it back works" -- "$STASH" pop --id "$ID"
assert_eq x "$(cat "$P/one")" "selection parent moved: back"

fresh_state nosave
D="$W/nosave"; files "$D"; ID=$(stashed "$D"); chflags uchg "$ETUDE_STATE_DIR"
OUT=$("$STASH" pop --id "$ID" 2>&1 </dev/null); RC=$?
chflags nouchg "$ETUDE_STATE_DIR"
clean "record not updated" "$OUT"
assert_eq 3 "$RC" "record not updated: exit"
has "record not updated: the items are back" "$OUT" "the restored items are back"
has "record not updated: what comes next" "$OUT" "stash pop --id $ID"
intact "$D" 3 "record not updated: items are back"
OUT=$("$STASH" pop --id "$ID" 2>&1 </dev/null)
clean "record not updated: next pop" "$OUT"
hasnt "record not updated: next pop reports nothing false" "$OUT" "not where stash left"
intact "$D" 3 "record not updated: next pop changes nothing"

fresh_state nosave2
D="$W/nosave2"; files "$D"; ID=$(stashed "$D"); chflags uchg "$ETUDE_STATE_DIR/stash-$ID.journal"
OUT=$("$STASH" pop --id "$ID" 2>&1 </dev/null); RC=$?
chflags nouchg "$ETUDE_STATE_DIR/stash-$ID.journal"
clean "record not writable" "$OUT"
assert_eq 3 "$RC" "record not writable: exit"
has "record not writable: names the place" "$OUT" "cannot save the undo record in $ETUDE_STATE_DIR"
has "record not writable: what comes next" "$OUT" "The next \`stash pop --id $ID\` will see"
assert_exit 0 "record not writable: the printed step works" -- "$STASH" pop --id "$ID"
intact "$D" 3 "record not writable: all back"

# --- refusals that name the way forward (rows 39, 40, 41) ---------------------
fresh_state refusals
D="$W/project"; mkdir -p "$D"; : > "$D/Cargo.toml"; printf x > "$D/main.rs"
say "project" 2 project_folder "To stash the whole folder in one piece, run: stash select '$D'" -- "$STASH" "$D"
hasnt "project: does not say sweep" "$LAST" "Sweep"
has "project: nothing moved" "$LAST" "Nothing was moved."
assert_exit 0 "project: the printed step works" -- "$STASH" select "$D"
[ ! -e "$D" ] && pass "project: the whole folder moved" || fail "project: still there"

D="$W/film.mp4.download"; files "$D"
say "download folder" 2 download_in_flight "still in progress" -- "$STASH" "$D"
hasnt "download folder: does not say sweep" "$LAST" "Sweep"

D="$W/hidden"; mkdir -p "$D"; : > "$D/.one"
say "one hidden" 1 - "1 hidden item is still in $D" -- "$STASH" "$D"
hasnt "one hidden: not clear" "$LAST" "already clear"
: > "$D/.two"
say "two hidden" 1 - "2 hidden items are still in $D" -- "$STASH" "$D"
"$STASH" "$D" --json >"$W/out.json" 2>/dev/null </dev/null
assert_eq 2 "$(field details.skipped_hidden "$W/out.json")" "two hidden: json count"
D="$W/empty"; mkdir -p "$D"
say "empty" 1 - "$D is already clear" -- "$STASH" "$D"

# --- lines beside a stash that succeeds (rows 42, 43, 44) ---------------------
fresh_state notes
D="$HOME/Dropbox/sync"; files "$D"
OUT=$("$STASH" "$D" 2>&1 </dev/null); RC=$?
clean "synced" "$OUT"
assert_eq 0 "$RC" "synced: exit unchanged"
has "synced: warns" "$OUT" "warning: this folder is inside a cloud-synced tree"
# The warning is the last chance to stop, so it comes before the moves.
if [ "$(printf '%s\n' "$OUT" | head -1)" = "warning: this folder is inside a cloud-synced tree, so these moves reach your other devices too." ] \
  && [ "$(printf '%s\n' "$OUT" | grep -c 'cloud-synced')" = 1 ]; then
  pass "synced: warned once, before the moves"
else
  fail "synced: the warning is not first: $OUT"
fi
D="$W/plain"; files "$D"
OUT=$("$STASH" "$D" 2>&1 </dev/null)
hasnt "plain: no warning" "$OUT" "cloud-synced"
hasnt "plain: no download line" "$OUT" "still in progress"
hasnt "plain: no bare pop" "$OUT" "Restore with: stash pop"$'\n'
hasnt "plain: no second restore line" "$OUT" "No deadline. Restore"
ID=$(printf '%s' "$OUT" | sed -n 's/.*stash pop --id //p' | head -1)
(cd / && assert_exit 0 "plain: the printed step works from another folder" -- "$STASH" pop --id "$ID")
intact "$D" 3 "plain: back"
D="$W/one"; mkdir -p "$D"; printf x > "$D/only"
OUT=$("$STASH" "$D" 2>&1 </dev/null)
has "one item: singular" "$OUT" "Stashed 1 item."
D="$W/dl"; files "$D"; : > "$D/x.crdownload"
OUT=$("$STASH" "$D" 2>&1 </dev/null); RC=$?
clean "download" "$OUT"
assert_eq 0 "$RC" "download: exit unchanged"
has "download: says so" "$OUT" "1 download still in progress was stashed too; it may fail."
D="$W/dl2"; mkdir -p "$D"; : > "$D/x.crdownload"; : > "$D/y.part"
OUT=$("$STASH" "$D" 2>&1 </dev/null)
has "two downloads: plural" "$OUT" "2 downloads still in progress were stashed too; they may fail."
# Row 43 again, where the download is not at the top of the folder. It moves
# inside its subfolder, so a check of the planned items alone never sees it.
D="$W/dl-nested"; files "$D"; mkdir -p "$D/sub/deeper"; : > "$D/sub/deeper/x.crdownload"
OUT=$("$STASH" "$D" 2>&1 </dev/null); RC=$?
clean "nested download" "$OUT"
assert_eq 0 "$RC" "nested download: exit unchanged"
has "nested download: says so" "$OUT" "1 download still in progress was stashed too; it may fail."
hasnt "nested download: keeps the name out" "$OUT" "crdownload"
D="$W/dl-package"; mkdir -p "$D/sub/film.mp4.download"; : > "$D/sub/film.mp4.download/part1"; : > "$D/sub/film.mp4.download/part2"
OUT=$("$STASH" "$D" 2>&1 </dev/null)
has "download folder inside: counted once" "$OUT" "1 download still in progress was stashed too; it may fail."
D="$W/dl-select"; mkdir -p "$D/sub"; : > "$D/x.crdownload"; : > "$D/sub/y.part"; : > "$D/keep"
OUT=$("$STASH" select "$D/x.crdownload" "$D/sub" 2>&1 </dev/null); RC=$?
clean "select downloads" "$OUT"
assert_eq 0 "$RC" "select downloads: exit unchanged"
has "select downloads: says so" "$OUT" "2 downloads still in progress were stashed too; they may fail."
OUT=$("$STASH" select "$D/keep" 2>&1 </dev/null)
hasnt "select, no download: no line" "$OUT" "still in progress"
D="$W/due"; files "$D"
OUT=$("$STASH" "$D" --for 2d 2>&1 </dev/null)
clean "deadline" "$OUT"
ID=$(printf '%s' "$OUT" | sed -n 's/.*Restore with: stash pop --id //p' | head -1)
has "deadline: pop names the id" "$OUT" "Run \`stash pop --id $ID\`"
has "deadline: status names the folder" "$OUT" "\`stash status '$D'\`"
(cd / && assert_exit 0 "deadline: the printed status works from another folder" -- "$STASH" status "$D")
# Rows 85 and 86 use the same stash.
say "not due" 2 not_due_yet "due in 2 days, so nothing was restored. To restore now, run it without --if-due." -- "$STASH" pop "$D" --if-due
(cd / && assert_exit 0 "deadline: the printed pop works from another folder" -- "$STASH" pop --id "$ID")
intact "$D" 3 "deadline: back"
D="$W/nodeadline"; files "$D"; "$STASH" "$D" >/dev/null 2>&1 </dev/null
OUT=$("$STASH" pop "$D" --if-due 2>&1 </dev/null); RC=$?
clean "no deadline" "$OUT"
assert_eq 0 "$RC" "no deadline: exit unchanged"
has "no deadline: says so" "$OUT" "This stash had no deadline, so it was restored."
D="$W/nodeadline2"; files "$D"; "$STASH" "$D" >/dev/null 2>&1 </dev/null
OUT=$("$STASH" pop "$D" 2>&1 </dev/null)
hasnt "no --if-due: no such line" "$OUT" "no deadline"

# --- pop finds nothing (row 45) -----------------------------------------------
fresh_state nothing
ALL="See all of them with: stash status --all"
say "unknown id" 1 unknown_operation "no stash with id 1-2-3-abc. $ALL" -- "$STASH" pop --id 1-2-3-abc
say "bare unknown id" 1 unknown_operation "no stash with id 1-2-3-abc. $ALL" -- "$STASH" pop 1-2-3-abc
say "no such folder" 3 missing_input "no folder at $W/absent. See what is stashed with: stash status --all" -- "$STASH" pop "$W/absent"
say "through a file" 3 missing_input "no folder at $W/plain/a.txt/x. See what is stashed with: stash status --all" -- "$STASH" pop "$W/plain/a.txt/x"
say "a file" 3 missing_input "$W/plain/a.txt is a file. Give the folder you stashed. See what is stashed with: stash status --all" -- "$STASH" pop "$W/plain/a.txt"
say "nothing latest" 1 missing_input "nothing is stashed" -- "$STASH" pop --latest
OUT=$(cd "$W/plain" && "$STASH" pop 2>&1 </dev/null)
clean "here" "$OUT"
has "here: names the folder it looked at" "$OUT" "no stash from $W/plain."
hasnt "here: no here" "$OUT" "stashed here"
D="$W/roots"; files "$D"; ID=$(stashed "$D"); mv "$D" "$W/roots2"
say "moved root" 3 missing_input "no folder at $D. See what is stashed with: stash status --all" -- "$STASH" pop "$D"
OUT=$("$STASH" status --all 2>&1 </dev/null)
has "moved root: the printed step finds it" "$OUT" "$ID"
mv "$W/roots2" "$D"
"$STASH" pop --id "$ID" >/dev/null 2>&1 </dev/null
say "already popped" 1 missing_input "no stash from $D. $ALL" -- "$STASH" pop "$D"

# --- one item (rows 84, 121) --------------------------------------------------
fresh_state item
D="$W/item"; files "$D"; ID=$(stashed "$D")
say "unknown item" 2 unknown_item "no item NOPE in this stash. Nothing was restored. List them with: stash inspect --id $ID" -- "$STASH" pop --id "$ID" --item NOPE
"$STASH" pop --id "$ID" --item NOPE --json >"$W/out.json" 2>/dev/null </dev/null
assert_eq refused "$(field status "$W/out.json")" "unknown item: status"
assert_exit 0 "unknown item: the printed step works" -- "$STASH" inspect --id "$ID"
ITEM=$("$STASH" inspect --id "$ID" --json 2>/dev/null </dev/null \
  | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["details"]["items"][0]["id"])')
OUT=$("$STASH" pop --id "$ID" --item "$ITEM" 2>&1 </dev/null); RC=$?
clean "one item" "$OUT"
assert_eq 0 "$RC" "one item: exit"
has "one item: counts" "$OUT" "Restored 1 item; 2 items remain held in this operation."
assert_exit 0 "item: finish" -- "$STASH" pop --id "$ID"

D="$W/item2"; files "$D"; ID=$(stashed "$D")
ITEM=$("$STASH" inspect --id "$ID" --json 2>/dev/null </dev/null \
  | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["details"]["items"][0]["id"])')
chmod 555 "$D"
OUT=$("$STASH" pop --id "$ID" --item "$ITEM" 2>&1 </dev/null); RC=$?
chmod 755 "$D"
clean "one item: read-only" "$OUT"
assert_eq 3 "$RC" "one item: read-only exit"
has "one item: read-only cause" "$OUT" "could not restore into $D: permission denied."
has "one item: read-only step" "$OUT" "stash pop --id $ID --item $ITEM"
assert_exit 0 "one item: read-only step works" -- "$STASH" pop --id "$ID" --item "$ITEM"
assert_exit 0 "item2: finish" -- "$STASH" pop --id "$ID"

# --- bad values (row 56, both tools; row 78) ----------------------------------
fresh_state values
D="$W/values"; files "$D"
say "stash --for -1d" 2 invalid_arguments "--for must be a length of time, got \`-1d\`. Try 30m, 2h, 3d or 1w." -- "$STASH" "$D" --for -1d
hasnt "stash --for -1d: not an option" "$LAST" "another option"
say "sweep --since -1" 2 invalid_arguments "--since must be a number, optionally with h or d, got -1" -- "$SWEEP" "$D" --since -1
hasnt "sweep --since -1: not an option" "$LAST" "another option"
say "sweep --since --yes" 2 - "--since needs a value, and \`--yes\` is another option." -- "$SWEEP" "$D" --since --yes
say "select --for -1d" 2 invalid_arguments "--for must be a length of time, got \`-1d\`." -- "$STASH" select "$D/a.txt" --for -1d
OUT=$("$STASH" select "$D/a.txt" --for --json 2>&1 >"$W/out.json" </dev/null); RC=$?
clean "select --for --json" "$OUT"
assert_eq 2 "$RC" "select --for --json: exit"
has "select --for --json: words" "$OUT" "--for needs a value, and \`--json\` is another option. Try 30m"
assert_eq invalid_arguments "$(field reason.code "$W/out.json")" "select --for --json: still a json result"
assert_eq refused "$(field status "$W/out.json")" "select --for --json: status"
intact "$D" 3 "bad values: nothing moved"

# --- stash select (rows 68 to 80) ---------------------------------------------
fresh_state select
S="$W/sel"; mkdir -p "$S/dir"; printf 1 > "$S/one"; printf 2 > "$S/two"; printf 3 > "$S/dir/in"
ln "$S/one" "$S/hard"; ln -s "$S/one" "$S/link"; mkfifo "$S/fifo"
NOTHING="Nothing was moved."
say "select missing" 3 missing_input "nothing at $S/absent. $NOTHING" -- "$STASH" select "$S/absent"
"$STASH" select "$S/absent" --json >"$W/out.json" 2>/dev/null </dev/null
assert_eq refused "$(field status "$W/out.json")" "select missing: status"
say "select twice" 2 unsafe_input "you named $S/one twice. $NOTHING Name it once." -- "$STASH" select "$S/one" "$S/one"
say "select hard link" 2 unsafe_input "$S/one and $S/hard are the same file. $NOTHING Name it once." -- "$STASH" select "$S/one" "$S/hard"
say "select fifo" 2 unsafe_input "$S/fifo is not a file or folder, so it cannot be stashed. $NOTHING" -- "$STASH" select "$S/fifo"
say "select link" 2 unsafe_input "$S/link is a symbolic link" -- "$STASH" select "$S/link"
say "select overlap" 2 unsafe_input "$S/dir/in is inside $S/dir, which you also selected. $NOTHING Name one." -- "$STASH" select "$S/dir" "$S/dir/in"
say "select system" 2 unsafe_input "refused: /System/x is inside /System, a system folder." -- "$STASH" select /System/x
mkdir -p "$HOME/.ssh"; printf k > "$HOME/.ssh/k"
say "select credentials" 2 unsafe_input "refused:" -- "$STASH" select "$HOME/.ssh/k"
has "select credentials: names it" "$LAST" "$HOME/.ssh/k"
SY="$HOME/Dropbox/keep"; mkdir -p "$SY"; printf s > "$SY/f"
say "select synced" 2 unsafe_input "$SY/f is in a cloud-synced folder, and stash select cannot use those. $NOTHING" -- "$STASH" select "$SY/f"
say "select into synced" 2 unsafe_input "--into folder $SY is in a cloud-synced folder" -- "$STASH" select "$S/one" --into "$SY"
say "select into missing" 3 missing_input "no folder at $W/absent. $NOTHING" -- "$STASH" select "$S/one" --into "$W/absent"
say "select into a file" 2 unsafe_input "--into needs a folder, and $S/two is not one. $NOTHING" -- "$STASH" select "$S/one" --into "$S/two"
say "select into itself" 2 unsafe_input "--into folder $S/dir is inside $S/dir, which you selected." -- "$STASH" select "$S/dir" --into "$S/dir"
mkdir -p "$W/ro"; printf r > "$W/ro/f"; chmod 555 "$W/ro"
say "select read-only" 3 permission_denied "cannot write in $W/ro: permission denied. $NOTHING Check it with \`ls -ld\`, then run this again." -- "$STASH" select "$W/ro/f"
hasnt "select read-only: --into could not help" "$LAST" "--into"
"$STASH" select "$W/ro/f" --json >"$W/out.json" 2>/dev/null </dev/null
assert_eq refused "$(field status "$W/out.json")" "select read-only: status"
"$STASH" select "$W/ro/f" --json >"$W/out.json" 2>/dev/null </dev/null
chmod 755 "$W/ro"
ID=$(selected "$W/ro/f")
[ -n "$ID" ] && pass "select read-only: fixing it works" || fail "select read-only: still failed"
"$STASH" pop --id "$ID" >/dev/null 2>&1 </dev/null; chmod 555 "$W/ro"
say "select into read-only" 3 permission_denied "cannot write in $W/ro: permission denied. $NOTHING Choose another place with --into FOLDER." -- "$STASH" select "$S/one" --into "$W/ro"
say "select unknown option" 2 unknown_option "unknown option --frob. Run \`stash help\`." -- "$STASH" select "$S/one" --frob
say "select repeated option" 2 invalid_arguments "--into was given more than once" -- "$STASH" select "$S/one" --into "$S" --into "$S"
say "select bad duration" 2 invalid_arguments "cannot read duration soon. Try 30m, 2h, 3d or 1w." -- "$STASH" select "$S/one" --for soon
OUT=$("$STASH" select "$S/one" --for 2>&1 </dev/null); RC=$?
clean "select no value" "$OUT"; assert_eq 2 "$RC" "select no value: exit"
has "select no value: names the option" "$OUT" "--for needs a value."
say "select no list" 3 missing_input "no list file at $W/nolist. $NOTHING" -- "$STASH" select --from0 "$W/nolist"
say "select list is a folder" 2 invalid_arguments "$S/dir is a folder; --from0 needs a file. $NOTHING" -- "$STASH" select --from0 "$S/dir"
say "select no-journal" 2 unsafe_input "stash select cannot run without undo, so remove --no-journal." -- "$STASH" select --no-journal "$S/one"
printf 'bad\377\0' > "$W/badlist"
say "select bad utf-8" 2 unsafe_input "has a path that is not UTF-8" -- "$STASH" select --from0 "$W/badlist"
printf 'x' > "$W/nonul"
say "select no nul" 2 unsafe_input "must end with a NUL byte" -- "$STASH" select --from0 "$W/nonul"
printf '\0\0' > "$W/emptyitem"
say "select empty path" 2 unsafe_input "has an empty path" -- "$STASH" select --from0 "$W/emptyitem"
[ -f "$S/one" ] && [ -f "$S/two" ] && [ -f "$S/dir/in" ] && pass "select refusals moved nothing" || fail "a select refusal moved something"
hidden_left=$(find "$S" "$W/ro" -maxdepth 1 -name '.stash-selection-*' | wc -l | tr -d ' ')
assert_eq 0 "$hidden_left" "select refusals left no private folder"

# The store of undo records cannot be used (row 48).
BLOCK="$W/state-is-a-file"; printf x > "$BLOCK"
OUT=$(ETUDE_STATE_DIR="$BLOCK" "$STASH" select "$S/two" 2>&1 </dev/null); RC=$?
clean "select no record" "$OUT"
assert_eq 3 "$RC" "select no record: exit"
has "select no record: names the place" "$OUT" "cannot save the undo record in $BLOCK"
has "select no record: a way out" "$OUT" "ETUDE_STATE_DIR"
has "select no record: nothing moved" "$OUT" "Nothing was moved, so there is nothing to restore."
ETUDE_STATE_DIR="$BLOCK" "$STASH" select "$S/two" --json >"$W/out.json" 2>/dev/null </dev/null
assert_eq refused "$(field status "$W/out.json")" "select no record: status"
assert_eq journal_write_failed "$(field reason.code "$W/out.json")" "select no record: reason"
[ -f "$S/two" ] && pass "select no record: file stayed" || fail "select no record: file moved"
assert_eq 0 "$(find "$S" -maxdepth 1 -name '.stash-selection-*' | wc -l | tr -d ' ')" "select no record: no private folder left"

OUT=$("$STASH" select "$S/two" 2>&1 </dev/null); RC=$?
clean "select works" "$OUT"
has "select works: singular" "$OUT" "Stashed 1 selected item."
ID=$(printf '%s' "$OUT" | sed -n 's/.*stash pop --id //p')
assert_exit 0 "select works: the printed step works" -- "$STASH" pop --id "$ID"

# --- across disks (rows 75, 76) -----------------------------------------------
if require hdiutil "disk images are needed for a folder on another disk"; then
  IMAGE="$W/small.dmg"
  if create_disk_image "$IMAGE" 8m HFS+ etudes-stress-108; then
    M="$W/mnt"; mkdir "$M"
    if hdiutil attach "$IMAGE" -nobrowse -mountpoint "$M" >/dev/null 2>&1; then
      MOUNTS+=("$M")
      fresh_state disks
      mkdir -p "$M/there"; printf f > "$M/f"
      say "select across disks" 2 unsafe_input "folder $M/there is on another disk from $S, and a folder cannot be stashed across disks. $NOTHING Use --into" -- "$STASH" select "$M/there" --into "$S"
      ID=$(selected "$M/there" --into "$M")
      [ -n "$ID" ] && pass "select across disks: the printed way out works" || fail "select across disks: --into on that disk failed"
      "$STASH" pop --id "$ID" >/dev/null 2>&1 </dev/null
      hdiutil detach "$M" >/dev/null 2>&1; MOUNTS=()
      hdiutil attach "$IMAGE" -readonly -nobrowse -mountpoint "$M" >/dev/null 2>&1 && MOUNTS+=("$M")
      say "select read-only disk" 3 permission_denied "cannot write in $M: the disk is read-only. $NOTHING Items on a read-only disk cannot be moved." -- "$STASH" select "$M/f"
      hdiutil detach "$M" >/dev/null 2>&1; MOUNTS=()
      hdiutil attach "$IMAGE" -nobrowse -mountpoint "$M" >/dev/null 2>&1 && MOUNTS+=("$M")
      head -c 100000000 /dev/zero > "$M/fill" 2>/dev/null
      i=0; while [ "$i" -lt 4000 ] && mkdir "$M/d$i" 2>/dev/null; do i=$((i+1)); done
      if mkdir "$M/probe" 2>/dev/null; then
        unproven "select on a full disk" "this file system still made a folder when full"
      else
        say "select full disk" 3 insufficient_space "cannot write in $M: the disk is full. $NOTHING Free some space, or choose another disk with --into FOLDER." -- "$STASH" select "$M/f"
        ID=$(selected "$M/f" --into "$S")
        [ -n "$ID" ] && pass "select full disk: the printed way out works" || fail "select full disk: --into another disk failed"
      fi
      hdiutil detach "$M" >/dev/null 2>&1; MOUNTS=()
    else
      unproven "disk messages" "the disk image would not attach"
    fi
  else
    unproven "disk messages" "the disk image could not be made"
  fi
fi

