#!/usr/bin/env bash
# A saved route is the person's chosen destination. Under --scheme existing,
# sweep creates a route's missing folder only when the run moves a file into
# it, lists it in the plan first, and undo removes it again once it is empty.
# A route with no files here creates nothing and is not mentioned; a file
# holding the route's name still refuses before anything moves.
#
# The change sits in shared planning code, so stash and unpack run here too:
# a stash and pop round trip, and an unpack to exact bytes. Everything is
# synthetic, in a temp dir, and removed on exit.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
exec </dev/null
require python3 "builds the fixture tree and reads JSON" || exit 0

W=$(workdir)
cleanup() {
  rm -rf "$W"
  eval "$_stress_state_cleanup"
}
trap cleanup EXIT
export HOME="$W/home"; mkdir -p "$HOME"

has()   { case "$2" in *"$3"*) pass "$1" ;; *) fail "$1: missing '$3' in: ${2%%$'\n'*}" ;; esac; }
hasnt() { case "$2" in *"$3"*) fail "$1: found '$3'" ;; *) pass "$1" ;; esac; }
make_files() {  # DIR EXT N: synthetic files whose bytes name themselves
  local i
  mkdir -p "$1"
  for i in $(seq 1 "$3"); do printf 'synthetic %s payload %s\n' "$2" "$i" > "$1/item-$i.$2"; done
}
bytes_ok() {  # DIR EXT N: every file is present with its own bytes
  local i bad=0
  for i in $(seq 1 "$3"); do
    [ "$(cat "$1/item-$i.$2" 2>/dev/null)" = "synthetic $2 payload $i" ] || bad=$((bad+1))
  done
  echo "$bad"
}
reason_of() { python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["reason"]["code"])' "$1" 2>/dev/null; }

PROFILE="$ETUDE_STATE_DIR/sweep-existing-profile.toml"
printf 'schema_version = 1\n\n[[routes]]\nextension = "qoi"\nfolder = "My Pictures"\n\n[[routes]]\nextension = "xyz"\nfolder = "Never Made"\n' > "$PROFILE"

# --- plan: the missing folder is listed, nothing is made ---------------------
D="$W/tree"; make_files "$D" qoi 4
OUT=$("$SWEEP" "$D" --scheme existing --since 0 2>&1); CODE=$?
assert_no_panic "existing plan" "$OUT"
assert_eq 0 "$CODE" "a plan with a missing route folder is an ordinary plan (exit $CODE)"
has "the plan names the folder it will create" "$OUT" "sweep creates the folder My Pictures for your saved route"
has "the group says it is a saved route with a new folder" "$OUT" ".qoi (saved route, new folder sweep creates)"
hasnt "the plan does not call the name an agent's" "$OUT" "chosen by your agent"
hasnt "a route with no files here is not mentioned" "$OUT" "Never Made"
"$SWEEP" "$D" --scheme existing --since 0 --json >"$W/plan.json" 2>/dev/null
has "the JSON plan lists the folder and its signal" "$(cat "$W/plan.json")" '"name":"My Pictures","signal":".qoi (saved route, new folder sweep creates)"'
[ ! -e "$D/My Pictures" ] && pass "planning created no folder" || fail "planning created the route folder"

# --- apply creates it, undo removes it ---------------------------------------
OUT=$("$SWEEP" apply "$D" --scheme existing --since 0 --yes 2>&1); CODE=$?
assert_no_panic "existing apply" "$OUT"
assert_eq 0 "$CODE" "apply moves into the created folder (exit $CODE)"
[ -d "$D/My Pictures" ] && pass "apply created the route folder" || fail "apply did not create the route folder"
assert_eq 0 "$(bytes_ok "$D/My Pictures" qoi 4)" "every file landed with its bytes"
assert_eq 0 "$(find "$D" -maxdepth 1 -type f -name '*.qoi' | wc -l | tr -d ' ')" "no routed file was left behind"
[ ! -e "$D/Never Made" ] && pass "a route with no files created no folder" || fail "an empty route folder was created"
OUT=$("$SWEEP" undo "$D" 2>&1); CODE=$?
assert_no_panic "undo" "$OUT"
assert_eq 0 "$CODE" "undo restores the files (exit $CODE)"
assert_eq 0 "$(bytes_ok "$D" qoi 4)" "every file is home with its bytes"
[ ! -e "$D/My Pictures" ] && pass "undo removed the empty folder sweep created" || fail "undo left the created folder"

# --- a file holding the route's name still refuses ---------------------------
printf 'synthetic file holding the name\n' > "$D/My Pictures"
OUT=$("$SWEEP" apply "$D" --scheme existing --since 0 --yes 2>&1); CODE=$?
assert_no_panic "file holds the name" "$OUT"
assert_eq 2 "$CODE" "a file holding the route name refuses (exit $CODE)"
has "the refusal names the file and says nothing moved" "$OUT" "routes .qoi to $D/My Pictures, which is a file, not a folder, so nothing moved."
has "the refusal gives one next step" "$OUT" "Change the .qoi route's folder in that profile."
"$SWEEP" apply "$D" --scheme existing --since 0 --yes --json >"$W/refused.json" 2>/dev/null
assert_eq profile_invalid "$(reason_of "$W/refused.json")" "the refusal's reason is profile_invalid"
assert_eq 0 "$(bytes_ok "$D" qoi 4)" "the refusal moved nothing"
assert_eq "synthetic file holding the name" "$(cat "$D/My Pictures")" "the file holding the name is untouched"

# --- stash and pop round trip --------------------------------------------------
S="$W/stashed"; make_files "$S" txt 5
OUT=$("$STASH" "$S" 2>&1); CODE=$?
assert_no_panic "stash" "$OUT"
assert_eq 0 "$CODE" "stash clears the folder (exit $CODE)"
assert_eq 0 "$(find "$S" -maxdepth 1 -type f -name 'item-*' | wc -l | tr -d ' ')" "no file is left in the folder"
OUT=$("$STASH" pop "$S" 2>&1); CODE=$?
assert_no_panic "stash pop" "$OUT"
assert_eq 0 "$CODE" "pop restores them (exit $CODE)"
assert_eq 0 "$(bytes_ok "$S" txt 5)" "every stashed file came back with its bytes"

# --- unpack extracts to the exact bytes ----------------------------------------
SRC="$W/source"; mkdir -p "$SRC/sub"
printf 'synthetic archive member\n' > "$SRC/member.txt"
printf 'synthetic nested member\n' > "$SRC/sub/nested.txt"
tar -czf "$W/fixture.tar.gz" -C "$SRC" member.txt sub
OUT=$("$UNPACK" "$W/fixture.tar.gz" --into "$W/out" 2>&1); CODE=$?
assert_no_panic "unpack" "$OUT"
assert_eq 0 "$CODE" "unpack extracts the synthetic archive (exit $CODE)"
[ "$(cat "$W/out/member.txt" 2>/dev/null)" = 'synthetic archive member' ] \
  && [ "$(cat "$W/out/sub/nested.txt" 2>/dev/null)" = 'synthetic nested member' ] \
  && pass "unpack wrote both members with their exact bytes" \
  || fail "unpack did not write the exact bytes"
