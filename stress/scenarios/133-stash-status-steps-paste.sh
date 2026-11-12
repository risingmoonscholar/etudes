#!/usr/bin/env bash
# The steps `stash status` prints work when pasted, whatever the folder is named.
# Run from a clear folder, status points at the stash elsewhere and prints how to
# see it and how to restore it; when that folder holds two stashes, the step
# restores the newest, since a bare pop of it refuses. With two stashes of one
# folder, pop refuses and prints how to restore the newest. Each step is run
# exactly as printed. A folder whose only stash has no undo record (made with
# --no-journal) is not one pop can restore, so status from elsewhere does not
# offer it, and status in that folder prints no pop step, only where the items
# are and that they move back by hand. A stashed folder that was moved names
# where it was stashed from and how to move it back and restore it, unless
# something else now sits there. In a new folder at the old place, status and
# pop say the stashed folder is not there, with no step that fails again, and a
# copy of the moved folder is said to match no record.
#
# A record counts for a folder only by the device and inode it kept, never by
# name: a folder whose items were put back by hand and its holding folder
# removed offers a pop that clears the record; a folder replaced by a copy of
# itself is said to match no record, and status elsewhere does not offer it; a
# holding folder moved alone into another folder gets no step to move that
# folder; and `status --id` gives no step for a record pop cannot restore.
#
# The folder names hold a space and a single quote, an escape character, a
# newline, a command substitution, or a leading dash. No raw escape byte may
# reach the terminal, nothing in a name is run, and --json carries the real
# path. Each name gets its own records, so status never names another. Steps
# run in bash, and in zsh as well when this host has it.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir) || exit 1; trap 'rm -rf "${W:?}"' EXIT
W=$(cd "${W:?}" && pwd -P) && [ -n "$W" ] || exit 1
require python3 "these checks read JSON with Python" || exit 0
export HOME="$W/home"; mkdir -p "$HOME" "$W/clear"
ESC=$'\033'
ZSH=$(command -v zsh || true)

fresh_state() {  # a stash of another case must not be the one status names
  export ETUDE_STATE_DIR="$W/$1-state"; mkdir -p "$ETUDE_STATE_DIR"
}
files() {  # files DIR NAME...: one small file per name
  local dir="$1" name; shift
  mkdir -p "$dir"
  for name in "$@"; do printf '%s\n' "$name" >"$dir/$name.txt"; done
}
# held DIR: how many visible items are in DIR (a holding folder is hidden)
# (counted without reading paths as lines: a name may hold a newline)
held() { find "$1" -mindepth 1 -maxdepth 1 ! -name '.*' -exec printf . \; | wc -c | tr -d ' '; }
in_shell() {  # in_shell SH: run STEP as printed in SH, from the clear folder
  if [ "$1" = zsh ]; then
    (cd "$W/clear" && STASH="$STASH" STEP="$STEP" "$ZSH" -f -c 'stash() { "$STASH" "$@" }; eval "$STEP"' 2>&1 </dev/null)
  else
    (cd "$W/clear" && stash() { "$STASH" "$@"; } && eval "$STEP" 2>&1 </dev/null)
  fi
}
# step LABEL TEXT PREFIX: the printed step after PREFIX on its own line
step() {
  local line
  line=$(printf '%s\n' "$2" | sed -n "s/^ *$3//p" | head -1)
  if [ -n "$line" ]; then pass "$1: step printed"; else fail "$1: no '$3' step in: $2"; fi
  STEP=$line
}
# run_step LABEL [both]: run STEP as printed, in this case's shell. A step that
# changes nothing is run in both shells when asked.
run_step() {
  local out code=0 sh
  [ -n "$STEP" ] || { fail "$1: no step to run"; return; }
  for sh in $( [ "${2:-}" = both ] && [ -n "$ZSH" ] && echo bash zsh || echo "$RUN_IN"); do
    code=0; out=$(in_shell "$sh") || code=$?
    if [ "$code" = 0 ]; then pass "$1: step runs as printed in $sh"; else fail "$1: '$STEP' exit $code in $sh: $out"; fi
    no_escape "$1: step output" "$out"
  done
}
has() { case "$2" in *"$3"*) pass "$1" ;; *) fail "$1: wanted '$3' in: $2" ;; esac; }
hasnt() { case "$2" in *"$3"*) fail "$1: found '$3' in: $2" ;; *) pass "$1" ;; esac; }
no_escape() {
  case "$2" in *"$ESC"*) fail "$1: a raw escape byte reached the terminal" ;; *) pass "$1: no raw escape" ;; esac
}

for case in quote escape newline subst dash; do
  case "$case" in
    quote) name="my desk's" ;;
    escape) name="bad${ESC}[31mname" ;;
    newline) name=$'new\nline it\'s' ;;
    subst) name='$(touch PWNED)`touch PWNED2`' ;;
    dash) name='-dash' ;;
  esac
  RUN_IN=bash
  case "$case" in escape|subst) [ -n "$ZSH" ] && RUN_IN=zsh ;; esac
  dir="$W/$case/$name"

  # From a clear folder: status names the stash elsewhere.
  fresh_state "$case"
  files "$dir" a b
  "$STASH" "$dir" >/dev/null 2>&1 </dev/null
  assert_eq "0" "$(held "$dir")" "$case: folder is clear after stash"
  OUT=$(cd "$W/clear" && "$STASH" status 2>&1 </dev/null)
  assert_no_panic "$case: status elsewhere" "$OUT"
  no_escape "$case: status elsewhere" "$OUT"
  JSON=$(cd "$W/clear" && "$STASH" status --json 2>/dev/null </dev/null)
  GOT=$(printf '%s' "$JSON" | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["details"]["elsewhere"])')
  assert_eq "$dir" "$GOT" "$case: --json carries the real path"
  step "$case: see" "$OUT" "See it with: "
  run_step "$case: see" both
  assert_eq "0" "$(held "$dir")" "$case: seeing restores nothing"
  step "$case: restore elsewhere" "$OUT" "Restore it with: "
  run_step "$case: restore elsewhere"
  assert_eq "2" "$(held "$dir")" "$case: restore elsewhere brings both back"

  # Two stashes of one folder: pop refuses and says how to restore the newest.
  "$STASH" "$dir" >/dev/null 2>&1 </dev/null
  files "$dir" c
  "$STASH" "$dir" >/dev/null 2>&1 </dev/null
  OUT=$(cd "$W/clear" && "$STASH" status 2>&1 </dev/null)
  assert_no_panic "$case: status elsewhere, two stashes" "$OUT"
  no_escape "$case: status elsewhere, two stashes" "$OUT"
  hasnt "$case: no bare pop for two stashes" "$OUT" "Restore it with:"
  step "$case: see two" "$OUT" "See them with: "
  run_step "$case: see two" both
  assert_eq "0" "$(held "$dir")" "$case: seeing two restores nothing"
  step "$case: restore newest elsewhere" "$OUT" "Restore the newest with: "
  run_step "$case: restore newest elsewhere"
  assert_eq "1" "$(held "$dir")" "$case: the newest of two came back from elsewhere"
  [ -f "$dir/c.txt" ] && pass "$case: newest from elsewhere is c" || fail "$case: c.txt not restored from elsewhere"
  "$STASH" "$dir" >/dev/null 2>&1 </dev/null  # c goes back: two stashes again

  OUT=$("$STASH" pop "$dir" 2>&1 </dev/null)
  assert_no_panic "$case: ambiguous pop" "$OUT"
  no_escape "$case: ambiguous pop" "$OUT"
  assert_eq "0" "$(held "$dir")" "$case: ambiguous pop restores nothing"
  step "$case: newest" "$OUT" "Or restore the newest: "
  run_step "$case: newest"
  assert_eq "1" "$(held "$dir")" "$case: the newest stash came back"
  [ -f "$dir/c.txt" ] && pass "$case: the newest is the one restored" || fail "$case: c.txt not restored"
  OUT=$(cd "$W/clear" && "$STASH" status 2>&1 </dev/null)
  step "$case: restore the last" "$OUT" "Restore it with: "
  run_step "$case: restore the last"
  assert_eq "3" "$(held "$dir")" "$case: every file is back"

  # Every record of the folder is now restored. A stash with no undo record
  # is one pop cannot restore, so status from elsewhere must not offer it.
  "$STASH" "$dir" --no-journal >/dev/null 2>&1 </dev/null
  assert_eq "0" "$(held "$dir")" "$case: folder is clear after stash --no-journal"
  OUT=$(cd "$W/clear" && "$STASH" status 2>&1 </dev/null)
  assert_no_panic "$case: status with only an unrecorded stash" "$OUT"
  no_escape "$case: status with only an unrecorded stash" "$OUT"
  hasnt "$case: no step pop cannot follow" "$OUT" "stash pop"

  # From that folder, status says pop cannot restore it and where the items
  # are, as a path that names the real holding folder when pasted.
  real=$(find "$dir" -mindepth 1 -maxdepth 1 -name '.stash-*' -type d)
  OUT=$(cd "$dir" && "$STASH" status 2>&1 </dev/null)
  assert_no_panic "$case: status on an unrecorded stash" "$OUT"
  no_escape "$case: status on an unrecorded stash" "$OUT"
  has "$case: says pop cannot restore it" "$OUT" "so stash pop cannot restore it."
  hasnt "$case: no pop step on an unrecorded stash" "${OUT//so stash pop cannot/}" "stash pop"
  hasnt "$case: no restore step on an unrecorded stash" "$OUT" "Restore with"
  step "$case: items are in" "$OUT" "The items are in: "
  shown_path=$(eval "printf '%s' $STEP")
  assert_eq "$real" "$shown_path" "$case: the printed holding path is the real one"
  has "$case: says to move them back" "$OUT" "Move them back by hand."
  JSON=$(cd "$dir" && "$STASH" status --json 2>/dev/null </dev/null)
  GOT=$(printf '%s' "$JSON" | python3 -I -c 'import json,sys; d=json.load(sys.stdin)["details"]; print(d["holding"]); print(d["undo_record"])')
  assert_eq "$real"$'\n'"False" "$GOT" "$case: --json gives the real holding path and no undo record"
  hasnt "$case: --json has no pop step" "$JSON" "stash pop"

  # A stashed folder that was moved: status in its new place names where it was
  # stashed from, and both printed steps, run as printed, put every file back.
  from="$W/$case/from $name"; to="$W/$case/to $name"
  files "$from" a b
  "$STASH" "$from" >/dev/null 2>&1 </dev/null
  mv "$from" "$to"
  OUT=$(cd "$to" && "$STASH" status 2>&1 </dev/null)
  assert_no_panic "$case: status on a moved folder" "$OUT"
  no_escape "$case: status on a moved folder" "$OUT"
  hasnt "$case: a moved folder still has its record" "$OUT" "no undo record"
  has "$case: says it has moved" "$OUT" "and that folder has moved here."
  JSON=$(cd "$to" && "$STASH" status --json 2>/dev/null </dev/null)
  GOT=$(printf '%s' "$JSON" | python3 -I -c 'import json,sys; d=json.load(sys.stdin)["details"]; print(d["moved_from"]); print(d["undo_record"]); print(d["way_back"]); print(bool(d["operation_id"]))')
  assert_eq "$from"$'\n'"True"$'\n'"clear"$'\n'"True" "$GOT" "$case: --json gives where it was, the record and the id"
  step "$case: move back" "$OUT" "Move it back with: "
  move=$STEP
  step "$case: restore moved" "$OUT" "Then restore it with: "
  pop=$STEP
  STEP=$move; run_step "$case: move back"
  STEP=$pop; run_step "$case: restore moved"
  assert_eq "2" "$(held "$from")" "$case: every file is back where it was stashed"
  [ ! -e "$to" ] && pass "$case: nothing is left at the moved place" || fail "$case: $to still exists"
  assert_eq "" "$(find "$from" -mindepth 1 -maxdepth 1 -name '.stash-*')" "$case: no holding folder is left"

  # Moved again, and something else now sits where it was stashed from: no
  # step that would fail or put one folder inside the other.
  ID=$("$STASH" "$from" --json 2>/dev/null </dev/null | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["details"]["stash_id"])')
  mv "$from" "$to"; mkdir "$from"
  OUT=$(cd "$to" && "$STASH" status 2>&1 </dev/null)
  assert_no_panic "$case: status when the old place is taken" "$OUT"
  no_escape "$case: status when the old place is taken" "$OUT"
  has "$case: says the old place is taken" "$OUT" "Something else is now at "
  has "$case: names the operation id" "$OUT" "Its operation id is "
  hasnt "$case: no move when the old place is taken" "$OUT" "mv "
  hasnt "$case: no pop when the old place is taken" "$OUT" "stash pop"
  JSON=$(cd "$to" && "$STASH" status --json 2>/dev/null </dev/null)
  GOT=$(printf '%s' "$JSON" | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["details"]["way_back"])')
  assert_eq "taken" "$GOT" "$case: --json says the old place is taken"
  GOT=$(printf '%s' "$JSON" | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["details"]["operation_id"])')
  assert_eq "$ID" "$GOT" "$case: --json gives the operation id when the old place is taken"

  # In the new folder now at the old place, the record names this path but the
  # stashed folder is not here: no pop step, and pop says so in words.
  OUT=$(cd "$from" && "$STASH" status 2>&1 </dev/null)
  assert_no_panic "$case: status in the replaced folder" "$OUT"
  no_escape "$case: status in the replaced folder" "$OUT"
  assert_eq "1 operation names this folder but holds nothing here." "$(printf '%s\n' "$OUT" | head -1)" "$case: the first line says nothing is held here"
  hasnt "$case: no count of items that are not here" "$OUT" "0 items"
  has "$case: says the record cannot be restored here" "$OUT" "its folder was moved or replaced, so stash pop cannot restore it here."
  hasnt "$case: no pop step in the replaced folder" "${OUT//so stash pop cannot/}" "stash pop"
  GOT=$(cd "$from" && "$STASH" status --json 2>/dev/null </dev/null | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["details"]["not_restorable_here"])')
  assert_eq "['$ID']" "$GOT" "$case: --json lists the record as not restorable here"
  code=0; OUT=$("$STASH" pop --id "$ID" 2>&1 </dev/null) || code=$?
  [ "$code" != 0 ] && pass "$case: pop into the replaced folder fails" || fail "$case: pop into the replaced folder exit 0"
  assert_no_panic "$case: pop into the replaced folder" "$OUT"
  no_escape "$case: pop into the replaced folder" "$OUT"
  hasnt "$case: pop prints no step that fails again" "$OUT" "Fix that, then run"
  has "$case: pop says to move the folder back" "$OUT" "move that folder back there, and run this again."
  assert_eq "2" "$(find "$to" -mindepth 2 -maxdepth 2 -exec printf . \; | wc -c | tr -d ' ')" "$case: both items are still held in the moved folder"

  # A copy of the moved folder holds a holding folder of the same name, which
  # is not the one the record made.
  copy="$W/$case/copy $name"
  cp -R "$to" "$copy"
  OUT=$(cd "$copy" && "$STASH" status 2>&1 </dev/null)
  assert_no_panic "$case: status in a copy" "$OUT"
  no_escape "$case: status in a copy" "$OUT"
  has "$case: a copy matches no record" "$OUT" "No undo record matches this stash, so stash pop cannot restore it."
  has "$case: a copy is not the recorded folder" "$OUT" ", but this holding folder is not the one it recorded."
  hasnt "$case: no pop step in a copy" "${OUT//so stash pop cannot/}" "stash pop"
  GOT=$(cd "$copy" && "$STASH" status --json 2>/dev/null </dev/null | python3 -I -c 'import json,sys; d=json.load(sys.stdin)["details"]; print(d["named_by_record"]); print(d["undo_record"])')
  assert_eq "$from"$'\n'"False" "$GOT" "$case: --json names the record's folder and no matching record"

  # Doing what pop said brings everything back.
  rmdir "$from"; mv "$to" "$from"
  code=0; OUT=$("$STASH" pop --id "$ID" 2>&1 </dev/null) || code=$?
  assert_eq "0" "$code" "$case: pop works once the folder is back"
  assert_eq "2" "$(held "$from")" "$case: every file is back after moving the folder back"

  # Items put back by hand and the empty holding folder removed, in the folder
  # that was stashed: pop only clears the record, and status says so.
  fresh_state "$case-home"
  home="$W/$case/home $name"
  files "$home" a b
  ID=$("$STASH" "$home" --json 2>/dev/null </dev/null | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["details"]["stash_id"])')
  hold=$(find "$home" -mindepth 1 -maxdepth 1 -name '.stash-*' -type d)
  mv "$hold"/* "$home"/; rmdir "$hold"
  OUT=$(cd "$home" && "$STASH" status 2>&1 </dev/null)
  no_escape "$case: status with items put back" "$OUT"
  hasnt "$case: items put back are not called moved" "$OUT" "moved or replaced"
  has "$case: says the items are back" "$OUT" "its items are back in this folder."
  step "$case: clear the record" "$OUT" "$ID: its items are back in this folder. Clear the record with: "
  run_step "$case: clear the record"
  assert_eq "2" "$(held "$home")" "$case: the items stay where they were put back"
  OUT=$(cd "$home" && "$STASH" status 2>&1 </dev/null)
  has "$case: the record is cleared" "$OUT" "Nothing stashed in "

  # The stashed folder replaced by a copy of itself: the items are here, in a
  # holding folder no record made, and nothing offers a pop.
  fresh_state "$case-copied"
  copied="$W/$case/copied $name"
  files "$copied" a b
  ID=$("$STASH" "$copied" --json 2>/dev/null </dev/null | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["details"]["stash_id"])')
  cp -R "$copied" "$copied.new"; rm -rf "${copied:?}"; mv "$copied.new" "$copied"
  real=$(find "$copied" -mindepth 1 -maxdepth 1 -name '.stash-*' -type d)
  OUT=$(cd "$copied" && "$STASH" status 2>&1 </dev/null)
  no_escape "$case: status in a copied folder" "$OUT"
  has "$case: a copied folder matches no record" "$OUT" "No undo record matches this stash, so stash pop cannot restore it."
  hasnt "$case: a copied folder does not say the items are elsewhere" "$OUT" "holds nothing here"
  step "$case: items in a copied folder" "$OUT" "The items are in: "
  assert_eq "$real" "$(eval "printf '%s' $STEP")" "$case: the printed holding path in a copied folder is the real one"
  GOT=$(cd "$copied" && "$STASH" status --json 2>/dev/null </dev/null | python3 -I -c 'import json,sys; d=json.load(sys.stdin)["details"]; print(d["stashed"]); print(d["undo_record"])')
  assert_eq "2"$'\n'"False" "$GOT" "$case: --json agrees the items are here with no matching record"
  OUT=$(cd "$W/clear" && "$STASH" status 2>&1 </dev/null)
  has "$case: status elsewhere does not offer a copied folder" "$OUT" "Nothing stashed in "
  hasnt "$case: no restore step for a copied folder" "$OUT" "Restore it with"
  OUT=$("$STASH" status --id "$ID" 2>&1 </dev/null)
  has "$case: status --id says pop cannot restore it" "$OUT" "so stash pop cannot restore it here."
  hasnt "$case: status --id offers no pop" "${OUT//so stash pop cannot/}" "stash pop"
  GOT=$("$STASH" status --id "$ID" --json 2>/dev/null </dev/null | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["details"]["resume_or_undo"])')
  assert_eq "None" "$GOT" "$case: status --id --json gives no step"
  code=0; OUT=$("$STASH" pop "$copied" 2>&1 </dev/null) || code=$?
  [ "$code" != 0 ] && pass "$case: pop of a copied folder fails" || fail "$case: pop of a copied folder exit 0"
  hasnt "$case: pop of a copied folder prints no step that fails again" "$OUT" "Fix that, then run"
  assert_eq "2" "$(find "$real" -mindepth 1 -maxdepth 1 -exec printf . \; | wc -c | tr -d ' ')" "$case: the copied items stay held"

  # The holding folder alone moved into another folder: no step moves that
  # other folder to the old place.
  fresh_state "$case-alone"
  alone="$W/$case/alone $name"; other="$W/$case/other $name"
  files "$alone" a b
  "$STASH" "$alone" >/dev/null 2>&1 </dev/null
  files "$other" mine
  mv "$alone"/.stash-* "$other"/; rmdir "$alone"
  real=$(find "$other" -mindepth 1 -maxdepth 1 -name '.stash-*' -type d)
  OUT=$(cd "$other" && "$STASH" status 2>&1 </dev/null)
  no_escape "$case: status on a holding folder moved alone" "$OUT"
  hasnt "$case: no move step for a holding folder moved alone" "$OUT" "mv "
  hasnt "$case: not called a moved folder" "$OUT" "has moved here"
  has "$case: says only the holding folder is here" "$OUT" "and only its holding folder is here, so stash pop cannot restore it."
  hasnt "$case: no pop step for a holding folder moved alone" "${OUT//so stash pop cannot/}" "stash pop"
  step "$case: items in a holding folder moved alone" "$OUT" "The items are in: "
  assert_eq "$real" "$(eval "printf '%s' $STEP")" "$case: the printed holding path is the real one"
  GOT=$(cd "$other" && "$STASH" status --json 2>/dev/null </dev/null | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["details"]["way_back"])')
  assert_eq "holding_only" "$GOT" "$case: --json says only the holding folder is here"
done
assert_eq "" "$(find "$W" -name 'PWNED*')" "nothing in a folder name was run"
