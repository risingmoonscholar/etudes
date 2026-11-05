#!/usr/bin/env bash
# Every failure in sweep, and every plan and scan failure sweep and stash share,
# says what failed and a next step that works, in plain words, and picks the code
# that fits the real cause. Each case checks the words, the exit and the reason.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir)
MOUNTS=()
cleanup() {
  for m in ${MOUNTS[@]+"${MOUNTS[@]}"}; do hdiutil detach "$m" -force >/dev/null 2>&1; done
  chflags -R nouchg "$W" 2>/dev/null; chmod -R u+rwx "$W" 2>/dev/null; rm -rf "$W"
}
trap cleanup EXIT
W=$(cd "$W" && pwd -P)
if [ "$(id -u)" = 0 ]; then
  unproven "sweep and plan messages" "root ignores file modes"
  exit 0
fi
export HOME="$W/home" ETUDE_STATE_DIR="$W/state" SWEEP_GRACE_SECS=0
mkdir -p "$HOME" "$ETUDE_STATE_DIR"

shots() {  # shots DIR: three files that group as Screenshots
  mkdir -p "$1"
  for n in 1 2 3; do printf "$n" > "$1/Screenshot 2026-01-0$n at 10.00.00.png"; done
}
BANNED='replan|journal seal|malformed|observations|listing failed|extractor|staging|publish|holding locator|holding parent|object identity|member row|dispatched|os error|panicked'
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
# say LABEL EXIT CODE WORDS -- COMMAND...: text run, then the same with --json on
# a command that takes it. Only for commands that change nothing when they fail.
say() {
  local label="$1" exit="$2" code="$3" words="$4"; shift 5
  local out rc
  out=$("$@" 2>&1 </dev/null); rc=$?
  clean "$label" "$out"
  assert_eq "$exit" "$rc" "$label: exit"
  has "$label: words" "$out" "$words"
  if [ "$code" != - ]; then
    "$@" --json >"$W/out.json" 2>/dev/null </dev/null
    assert_eq "$code" "$(field reason.code "$W/out.json")" "$label: reason"
  fi
  LAST="$out"
}
# plan_of TOOL DIR FILE: export a plan and print its digest
plan_of() {
  local tool="$1" dir="$2" file="$3"
  if [ "$tool" = sweep ]; then "$SWEEP" "$dir" --export-plan "$file" --json 2>/dev/null
  else "$STASH" "$dir" --export-plan "$file" --json 2>/dev/null; fi \
    | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["details"]["binding"]["plan_digest"])'
}
ZERO=$(printf '0%.0s' $(seq 64))

# --- plan files, the same answer from both tools (rows 22 to 30) -----------
for tool in sweep stash; do
  if [ "$tool" = sweep ]; then RUN=("$SWEEP" apply); BIN_T="$SWEEP"; YES=(--yes); else RUN=("$STASH"); BIN_T="$STASH"; YES=(); fi
  T="$W/plans-$tool"; mkdir -p "$T"
  say "$tool: no plan file" 3 missing_input "no plan file at $T/absent" -- "${RUN[@]}" --plan "$T/absent" --plan-digest "$ZERO" ${YES[@]+"${YES[@]}"}
  printf x > "$T/locked"; chmod 000 "$T/locked"
  say "$tool: plan file denied" 3 permission_denied "cannot read $T/locked: permission denied" -- "${RUN[@]}" --plan "$T/locked" --plan-digest "$ZERO" ${YES[@]+"${YES[@]}"}
  : > "$T/empty"
  say "$tool: empty plan file" 2 invalid_arguments "$T/empty is not an exported plan" -- "${RUN[@]}" --plan "$T/empty" --plan-digest "$ZERO" ${YES[@]+"${YES[@]}"}
  head -c 400 /dev/zero | tr '\0' 'a' > "$T/junk"
  say "$tool: another kind of file" 2 invalid_arguments "$T/junk is not an exported plan" -- "${RUN[@]}" --plan "$T/junk" --plan-digest "$ZERO" ${YES[@]+"${YES[@]}"}
  say "$tool: mistyped digest" 2 invalid_arguments "--plan-digest needs the 64-character digest" -- "${RUN[@]}" --plan "$T/empty" --plan-digest abc ${YES[@]+"${YES[@]}"}
  say "$tool: plan path is a folder" 2 invalid_arguments "$T is a folder. --plan needs the plan file." -- "${RUN[@]}" --plan "$T" --plan-digest "$ZERO" ${YES[@]+"${YES[@]}"}

  D="$W/plan-tree-$tool"; shots "$D"
  DIGEST=$(plan_of "$tool" "$D" "$T/plan")
  say "$tool: wrong digest" 2 plan_stale "does not match that digest" -- "${RUN[@]}" --plan "$T/plan" --plan-digest "$ZERO" ${YES[@]+"${YES[@]}"}
  say "$tool: a flag cannot change a saved plan" 2 invalid_arguments "cannot change a saved plan" -- "${RUN[@]}" --plan "$T/plan" --plan-digest "$DIGEST" ${YES[@]+"${YES[@]}"} "$( [ "$tool" = sweep ] && echo --allow-sync || echo --for)" $( [ "$tool" = stash ] && echo 1d)
  say "$tool: a plan names its own folder" 2 invalid_arguments "already names its folder" -- "${RUN[@]}" "$D" --plan "$T/plan" --plan-digest "$DIGEST" ${YES[@]+"${YES[@]}"}
  printf x > "$D/arrived later.txt"
  say "$tool: folder changed" 2 plan_stale "$D changed since this plan was made" -- "${RUN[@]}" --plan "$T/plan" --plan-digest "$DIGEST" ${YES[@]+"${YES[@]}"}
  hasnt "$tool: folder changed does not say undo" "$LAST" "undo"
  STEP=$(printf '%s\n' "$LAST" | sed -n 's/^Make a new plan: //p')
  STEP=${STEP/NEW_PLAN_FILE/$T/plan2}
  if eval "\"$BIN_T\" ${STEP#* }" >/dev/null 2>&1; then pass "$tool: the printed step makes a new plan"; else fail "$tool: printed step failed: $STEP"; fi
  [ -s "$T/plan2" ] && pass "$tool: the new plan exists" || fail "$tool: the new plan is missing"

  # --export-plan failures (row 31)
  E="$W/export-$tool"; shots "$E"; EXP=("$BIN_T" "$E"); printf x > "$T/taken"
  say "$tool: export into a missing folder" 3 missing_input "cannot write the plan to $T/none/p: no folder there" -- "${EXP[@]}" --export-plan "$T/none/p"
  say "$tool: export onto a file" 2 destination_exists "that file already exists" -- "${EXP[@]}" --export-plan "$T/taken"
  say "$tool: export inside the folder" 2 invalid_arguments "inside the folder being planned" -- "${EXP[@]}" --export-plan "$E/p"
  mkdir "$T/shut"; chmod 500 "$T/shut"
  say "$tool: export denied" 3 permission_denied "permission denied" -- "${EXP[@]}" --export-plan "$T/shut/p"
  chmod 700 "$T/shut"; chmod 600 "$T/locked"

  # scan refusals name the folder as typed (rows 32, 33, 35, 36, 37, 127)
  say "$tool: missing folder" 3 missing_input "no folder at $W/absent." -- "$BIN_T" "$W/absent"
  printf x > "$T/f.txt"
  say "$tool: path through a file" 3 missing_input "no folder at $T/f.txt/x." -- "$BIN_T" "$T/f.txt/x"
  say "$tool: path is a file" 3 missing_input "$T/f.txt is a file. Give the folder that holds it." -- "$BIN_T" "$T/f.txt"
  L="$W/mine-$tool/Library/sub"; shots "$L"
  say "$tool: inside a folder named Library" 2 unsafe_input "$L is inside a folder named Library, which these tools never organise" -- "$BIN_T" "$L"
  N="$W/mine-$tool/node_modules"; shots "$N"
  say "$tool: a folder named node_modules" 2 unsafe_input "$N is a folder named node_modules" -- "$BIN_T" "$N"
  P="$W/proj-$tool"; shots "$P"; printf x > "$P/Cargo.toml"
  say "$tool: project folder" 2 project_folder "$P looks like a project" -- "$BIN_T" "$P"
  hasnt "$tool: project folder shows the name" "$LAST" "<name"
done
hasnt "stash: project words fit stash" "$LAST" "sorting them"
say "sweep: /System itself" 2 unsafe_input "/System is a system folder" -- "$SWEEP" /System

# --- a plan from the other tool (row 26) ------------------------------------
D="$W/cross"; shots "$D"
SWEEP_DIGEST=$(plan_of sweep "$D" "$W/cross-sweep.plan")
STASH_DIGEST=$(plan_of stash "$D" "$W/cross-stash.plan")
say "stash runs a sweep plan" 2 invalid_arguments "this plan was made by sweep. Run it with sweep" -- "$STASH" --plan "$W/cross-sweep.plan" --plan-digest "$SWEEP_DIGEST"
say "sweep runs a stash plan" 2 invalid_arguments "this plan was made by stash. Run it with stash" -- "$SWEEP" apply --plan "$W/cross-stash.plan" --plan-digest "$STASH_DIGEST" --yes
printf '{}' > "$W/empty.proposal"
say "sweep proposes on a stash plan" 2 invalid_arguments "this plan was made by stash" -- "$SWEEP" propose --plan "$W/cross-stash.plan" --plan-digest "$STASH_DIGEST" --proposal "$W/empty.proposal" --export-plan "$W/out.plan"

# --- the same plan twice (row 29) -------------------------------------------
D="$W/twice"; shots "$D"
DIGEST=$(plan_of sweep "$D" "$W/twice.plan")
assert_exit 0 "a saved plan applies once" -- "$SWEEP" apply --plan "$W/twice.plan" --plan-digest "$DIGEST" --yes
say "the same plan again" 2 plan_stale "or this plan already ran" -- "$SWEEP" apply --plan "$W/twice.plan" --plan-digest "$DIGEST" --yes

# --- sync-looking names say why (row 38) ------------------------------------
D="$W/Dropbox notes 2024"; shots "$D"
say "a name that only looks synced" 2 unsafe_input 'looks cloud-synced (its path contains "Dropbox")' -- "$SWEEP" "$D"
has "that refusal names the folder" "$LAST" "$D"
has "that refusal names the flag" "$LAST" "--allow-sync"
OUT=$("$SWEEP" "$D" --allow-sync 2>&1 </dev/null); assert_eq 0 "$?" "the printed flag works"

# --- a file where the group folder goes (row 21), and collisions (row 65) ---
D="$W/blocked"; shots "$D"; printf x > "$D/Screenshots"
OUT=$("$SWEEP" apply "$D" --yes 2>&1 </dev/null); RC=$?
clean "a file holds the folder's name" "$OUT"
assert_eq 2 "$RC" "a file holds the folder's name: exit"
has "that file is named" "$OUT" 'a file named "Screenshots" is where sweep would make that folder'
has "nothing moved" "$OUT" "Nothing was moved"
hasnt "no undo step" "$OUT" "sweep undo"
"$SWEEP" apply "$D" --yes --json >"$W/out.json" 2>/dev/null </dev/null
assert_eq destination_exists "$(field reason.code "$W/out.json")" "row 21: code"
assert_eq refused "$(field status "$W/out.json")" "row 21: status"
rm "$D/Screenshots"
assert_exit 0 "after renaming it the apply works" -- "$SWEEP" apply "$D" --yes

D="$W/collide"; mkdir -p "$D/a" "$D/b"
for f in a b; do printf "$f" > "$D/$f/Screenshot 2026-01-01 at 10.00.00.png"; done
printf 2 > "$D/Screenshot 2026-01-02 at 10.00.00.png"; printf 3 > "$D/Screenshot 2026-01-03 at 10.00.00.png"
OUT=$("$SWEEP" apply "$D" --yes --depth 2 2>&1 </dev/null); RC=$?
clean "two files, one name" "$OUT"
assert_eq 2 "$RC" "two files, one name: exit"
has "says what would happen" "$OUT" "two files would land in $D/Screenshots with the same name"
hasnt "does not name the file" "$OUT" "Screenshot 2026"
has "says nothing moved" "$OUT" "Nothing was moved"

# --- undo and its counts (rows 5, 7, 19, 51, 52) ----------------------------
mkdir "$W/no-history"
say "undo with no history" 1 nothing_to_do "nothing to undo. No apply is on record." -- env ETUDE_STATE_DIR="$W/no-history" "$SWEEP" undo
say "undo of a file" 3 missing_input "is a file. Give the folder you applied." -- "$SWEEP" undo "$W/plans-sweep/f.txt"
say "status of a file" 3 missing_input "is a file. Give the folder you applied." -- "$SWEEP" status "$W/plans-sweep/f.txt"
say "stash status of a file" 3 missing_input "is a file. Give the folder you stashed." -- "$STASH" status "$W/plans-sweep/f.txt"

undo_case() {  # undo_case NAME: a fresh applied folder, its undo in OUT and J
  D="$W/$1"; shots "$D"
  assert_exit 0 "$1: apply" -- "$SWEEP" apply "$D" --yes
}
# occupied original place
undo_case occupied
printf new > "$D/Screenshot 2026-01-01 at 10.00.00.png"
OUT=$("$SWEEP" undo "$D" 2>&1 </dev/null); RC=$?
clean "occupied" "$OUT"
assert_eq 3 "$RC" "occupied: an unfinished undo is not exit 0"
has "occupied: counts" "$OUT" "Restored 2 of 3 files."
has "occupied: says why" "$OUT" "1 file was not put back because its original place is taken by another file."
hasnt "occupied: does not claim a change" "$OUT" "changed since"
hasnt "occupied: no plural brackets" "$OUT" "(s)"
"$SWEEP" undo "$D" --json >"$W/out.json" 2>/dev/null </dev/null; RC=$?
assert_eq 3 "$RC" "occupied json: exit"
assert_eq incomplete "$(field status "$W/out.json")" "occupied: status"
assert_eq operation_incomplete "$(field reason.code "$W/out.json")" "occupied: reason"
assert_eq 1 "$(field details.skipped_occupied "$W/out.json")" "occupied: its own count"
assert_eq 0 "$(field details.skipped_changed "$W/out.json")" "occupied: not counted as changed"
assert_eq 1 "$(python3 -I -c 'import json,sys; print({c["name"]: c["count"] for c in json.load(open(sys.argv[1]))["effects"]["counters"]}["occupied_items_left"])' "$W/out.json")" "occupied: its own effect"
"$SWEEP" status "$D" --json >"$W/status.json" 2>/dev/null </dev/null
assert_eq 1 "$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["details"]["counts"]["not_as_left"])' "$W/status.json")" "occupied: status counts it"
rm "$D/Screenshot 2026-01-01 at 10.00.00.png"
OUT=$("$SWEEP" undo "$D" 2>&1 </dev/null); RC=$?
assert_eq 0 "$RC" "occupied: the printed step finishes it"
assert_intact "$D" 3 "occupied: every file is back"

# changed file
undo_case edited
printf 'an edit that changes the size' > "$D/Screenshots/Screenshot 2026-01-01 at 10.00.00.png"
OUT=$("$SWEEP" undo "$D" 2>&1 </dev/null); RC=$?
assert_eq 3 "$RC" "changed: exit"
has "changed: singular" "$OUT" "1 file changed since apply and was left alone"

# the group folder was renamed
undo_case renamed
mv "$D/Screenshots" "$D/Elsewhere"
OUT=$("$SWEEP" undo "$D" 2>&1 </dev/null); RC=$?
clean "renamed" "$OUT"
assert_eq 3 "$RC" "renamed: exit"
has "renamed: counts" "$OUT" "Restored 0 of 3 files."
has "renamed: where" "$OUT" "3 files are not where apply left them ($D/Screenshots)"
hasnt "renamed: not gone" "$OUT" "already gone"
"$SWEEP" undo "$D" --json >"$W/out.json" 2>/dev/null </dev/null
assert_eq incomplete "$(field status "$W/out.json")" "renamed: status"
assert_eq operation_incomplete "$(field reason.code "$W/out.json")" "renamed: reason"
OUT=$("$SWEEP" status "$D" 2>&1 </dev/null)
has "renamed: status does not say all is well" "$OUT" "3 files are not as apply left them"
mv "$D/Elsewhere" "$D/Screenshots"
assert_exit 0 "renamed: moving it back lets undo finish" -- "$SWEEP" undo "$D"
assert_intact "$D" 3 "renamed: every file is back"

# the folder itself was moved
undo_case moved
ROOT="$D"; mv "$D" "$W/moved-away"
OUT=$("$SWEEP" undo 2>&1 </dev/null); RC=$?
clean "root moved" "$OUT"
assert_eq 3 "$RC" "root moved: exit"
has "root moved: says so" "$OUT" "$ROOT was moved or replaced"
has "root moved: next step" "$OUT" "Move it back to $ROOT, then run: sweep undo '$ROOT'"
hasnt "root moved: no identity word" "$OUT" "identity"
mv "$W/moved-away" "$ROOT"
assert_exit 0 "root moved: the printed step works" -- "$SWEEP" undo "$ROOT"
assert_intact "$ROOT" 3 "root moved: every file is back"
for extra in 1; do "$SWEEP" undo >/dev/null 2>&1 </dev/null; done

# --- damaged undo records (rows 13, 16, 47, 122) ----------------------------
cut_to_base() {  # cut a record back to its sealed base: every later entry lost
  python3 -I - "$1" <<'PY'
import struct, sys
path = sys.argv[1]
data = open(path, "rb").read()
size = struct.unpack("<I", data[:4])[0]
open(path, "wb").write(data[:4 + size])
PY
}
damaged_state() {  # a fresh state folder, so older records do not get in the way
  export ETUDE_STATE_DIR="$W/$1-state"; mkdir -p "$ETUDE_STATE_DIR"
}
damaged_state cut
D="$W/cut"; shots "$D"
assert_exit 0 "cut: apply" -- "$SWEEP" apply "$D" --yes
RECORD=$(ls "$ETUDE_STATE_DIR"/sweep-*.journal)
cut_to_base "$RECORD"
OUT=$("$SWEEP" status "$D" 2>&1 </dev/null); RC=$?
clean "cut: status" "$OUT"
has "cut: status warns" "$OUT" "This undo record is damaged, so these counts may be low."
snapshot_tree "$D" "$W/cut.snap"
OUT=$("$SWEEP" undo "$D" 2>&1 </dev/null); RC=$?
clean "cut: undo" "$OUT"
assert_eq 3 "$RC" "cut: exit"
hasnt "cut: does not first claim it is reversing" "$OUT" "puts back everything"
has "cut: says nothing moved" "$OUT" "so it moved nothing"
has "cut: says where the files are" "$OUT" "$D/Screenshots"
has "cut: the step by hand" "$OUT" "move them back by hand into $D"
hasnt "cut: does not recommend forget" "$OUT" "forget"
snapshot_tree "$D" "$W/cut-after.snap"; assert_snapshot_eq "$W/cut.snap" "$W/cut-after.snap" "cut: nothing moved"
"$SWEEP" undo "$D" --json >"$W/out.json" 2>/dev/null </dev/null
assert_eq refused "$(field status "$W/out.json")" "cut: status refused"
mv "$D/Screenshots/"* "$D/" && pass "cut: moving them by hand works" || fail "cut: moving by hand failed"

damaged_state seal
D="$W/seal"; shots "$D"
assert_exit 0 "wrong key: apply" -- "$SWEEP" apply "$D" --yes
GOOD_KEY=$ETUDE_JOURNAL_KEY
OTHER=$(python3 -c 'import secrets; print(secrets.token_hex(32))')
RECORD=$(ls "$ETUDE_STATE_DIR"/sweep-*.journal)
for how in "" "$D"; do
  OUT=$(ETUDE_JOURNAL_KEY="$OTHER" "$SWEEP" undo $how 2>&1 </dev/null); RC=$?
  clean "wrong key undo '$how'" "$OUT"
  assert_eq 3 "$RC" "wrong key: exit"
  has "wrong key: names the record" "$OUT" "the undo record $RECORD cannot be opened (wrong key, or it was altered)"
  has "wrong key: nothing moved" "$OUT" "Nothing was moved."
  has "wrong key: which key" "$OUT" "Use the key from when you applied."
  has "wrong key: by hand" "$OUT" "you can move them back by hand"
  hasnt "wrong key: no forget" "$OUT" "forget"
done
OUT=$(ETUDE_JOURNAL_KEY="$OTHER" "$SWEEP" status "$D" 2>&1 </dev/null); RC=$?
clean "wrong key status" "$OUT"
assert_eq 3 "$RC" "wrong key status: exit"
has "wrong key status: names the record" "$OUT" "cannot be opened"
assert_exit 0 "the right key undoes it" -- "$SWEEP" undo "$D"
printf garbage > "$RECORD"
OUT=$("$SWEEP" undo 2>&1 </dev/null); RC=$?
clean "damaged record" "$OUT"
assert_eq 3 "$RC" "damaged record: exit"
has "damaged record: says so" "$OUT" "cannot be opened ("

# the store itself (row 50)
damaged_state store
D="$W/store"; shots "$D"
BLOCK="$W/state-is-a-file"; printf x > "$BLOCK"
for args in "apply $D --yes" "undo" "status $D"; do
  OUT=$(ETUDE_STATE_DIR="$BLOCK" "$SWEEP" $args 2>&1 </dev/null); RC=$?
  clean "store is a file: $args" "$OUT"
  [ "$RC" -ne 0 ] && pass "store is a file: $args fails" || fail "store is a file: $args exit 0"
  has "store is a file: $args names it" "$OUT" "$BLOCK"
  has "store is a file: $args gives a way out" "$OUT" "ETUDE_STATE_DIR"
done
assert_intact "$D" 3 "store is a file: nothing moved"

# --- forget refuses before it removes anything -------------------------------
damaged_state forget
D="$W/forget"; shots "$D"; E="$W/forget-stashed"; shots "$E"
assert_exit 0 "forget: apply" -- "$SWEEP" apply "$D" --yes
assert_exit 0 "forget: stash" -- "$STASH" "$E"
snapshot_tree "$ETUDE_STATE_DIR" "$W/forget.snap"
OUT=$("$SWEEP" forget 2>&1 </dev/null); RC=$?
clean "forget with a stash" "$OUT"
assert_eq 2 "$RC" "forget with a stash: exit"
has "forget says nothing was removed" "$OUT" "Nothing was removed."
hasnt "forget does not report a removal" "$OUT" "Removed "
snapshot_tree "$ETUDE_STATE_DIR" "$W/forget-after.snap"
assert_snapshot_eq "$W/forget.snap" "$W/forget-after.snap" "forget with a stash removed nothing"
assert_exit 0 "forget --yes still works" -- "$SWEEP" forget --yes
[ -z "$(find "$ETUDE_STATE_DIR" -name 'sweep-*.journal')" ] && pass "forget --yes removed sweep's records" || fail "forget --yes left records"

# --- existing-folder profile (rows 12, 57 to 62, 114) ------------------------
damaged_state profile
D="$W/profile"; shots "$D"; printf x > "$D/one.xyz"; mkdir "$D/Mine"
PROFILE="$ETUDE_STATE_DIR/sweep-existing-profile.toml"
say "no profile" 2 profile_missing "no profile at $PROFILE" -- "$SWEEP" "$D" --scheme existing
has "no profile: next step" "$LAST" "drop --scheme existing"
OUT=$("$SWEEP" apply "$D" --scheme existing --yes 2>&1 </dev/null); RC=$?
assert_eq 2 "$RC" "no profile: apply refuses"
assert_intact "$D" 4 "no profile: nothing moved"
ROUTE='[[routes]]\nextension = "xyz"\nfolder = "Mine"\n'
printf "schema_version = 1\n${ROUTE}${ROUTE}" > "$PROFILE"
say "route twice" 2 profile_invalid "the profile at $PROFILE: .xyz" -- "$SWEEP" "$D" --scheme existing
hasnt "route twice: not --map" "$LAST" "--map"
has "route twice: next step" "$LAST" "Edit that file."
printf 'schema_version = 1\n[[routes]]\nextension = "xyz"\nfolder = "../up"\n' > "$PROFILE"
say "traversal route" 2 profile_invalid "the profile at $PROFILE" -- "$SWEEP" "$D" --scheme existing
hasnt "traversal route: not --map" "$LAST" "--map"
printf 'schema_version = 1\n\n[[routes]]\ncolor = "red"\n' > "$PROFILE"
say "unknown field" 2 profile_invalid 'line 4: unknown field "color". Allowed: extension, folder.' -- "$SWEEP" "$D" --scheme existing
has "unknown field: names the file" "$LAST" "$PROFILE"
printf 'schema_version = 1\n[[routes]]\nextension = "xyz"\nfolder = "Absent"\n' > "$PROFILE"
say "route to an absent folder" 2 profile_invalid "does not exist; create it or edit the profile" -- "$SWEEP" "$D" --scheme existing
chmod 000 "$PROFILE"
say "denied profile" 2 profile_invalid "cannot read the profile at $PROFILE: permission denied" -- "$SWEEP" "$D" --scheme existing
chmod 600 "$PROFILE"; rm "$PROFILE"; mkdir "$PROFILE"
say "profile is a folder" 2 profile_invalid "cannot read the profile at $PROFILE" -- "$SWEEP" "$D" --scheme existing
rmdir "$PROFILE"
printf "schema_version = 1\n${ROUTE}" > "$PROFILE"
say "--map with a profile" 2 invalid_arguments "--map cannot be combined with --scheme existing" -- "$SWEEP" "$D" --scheme existing --map xyz=Other
DIGEST=$(plan_of_existing() { "$SWEEP" "$D" --scheme existing --export-plan "$W/existing.plan" --json 2>/dev/null | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["details"]["binding"]["plan_digest"])'; }; plan_of_existing)
printf '# edited\n' >> "$PROFILE"
say "profile changed" 2 plan_stale "the profile changed since this plan was made" -- "$SWEEP" apply --plan "$W/existing.plan" --plan-digest "$DIGEST" --yes
has "profile changed: next step" "$LAST" "Export a new plan."

# --- proposals (rows 61, 63, 64) ---------------------------------------------
damaged_state propose
D="$W/propose"; shots "$D"; mkdir "$D/Reading"
"$SWEEP" "$D" --export-plan "$W/p.plan" --json >"$W/p.json" 2>/dev/null </dev/null
DIGEST=$(field details.binding.plan_digest "$W/p.json")
PROPOSE=("$SWEEP" propose --plan "$W/p.plan" --plan-digest "$DIGEST" --export-plan "$W/p.out")
say "no proposal file" 3 missing_input "no proposal file at $W/absent.json" -- "${PROPOSE[@]}" --proposal "$W/absent.json"
printf '{"schema_version":1,"rest":"leave","plan_digest":"%s","decisions":[{"item":"i-000000","skip":true},{"item":"i-000001"}]}' "$DIGEST" > "$W/shape.json"
say "which decision" 2 invalid_arguments "decision 2 (item i-000001)" -- "${PROPOSE[@]}" --proposal "$W/shape.json"
printf '{"schema_version":1,"rest":"leave","plan_digest":"%s","decisions":[{"item":"i-000000","folder":"Absent","mode":"require_existing"}]}' "$DIGEST" > "$W/absent-folder.json"
say "proposed folder absent" 2 missing_input 'folder "Absent" does not exist' -- "${PROPOSE[@]}" --proposal "$W/absent-folder.json"

# --- what apply and scan hold back (rows 107, 108) ---------------------------
D="$W/recent"; shots "$D"
OUT=$(env -u SWEEP_GRACE_SECS "$SWEEP" "$D" 2>&1 </dev/null); RC=$?
clean "only recent files: scan" "$OUT"
assert_eq 1 "$RC" "only recent files: scan exit"
assert_eq 1 "$(printf '%s\n' "$OUT" | grep -c 'changed too recently')" "the count prints once"
OUT=$(env -u SWEEP_GRACE_SECS "$SWEEP" apply "$D" --yes 2>&1 </dev/null); RC=$?
clean "only recent files: apply" "$OUT"
assert_eq 1 "$RC" "only recent files: apply exit"
has "apply gives the way forward" "$OUT" "pass --since 0"
assert_exit 0 "the way forward works" -- env -u SWEEP_GRACE_SECS "$SWEEP" apply "$D" --yes --since 0

# --- the item cap says what to do (row 66) -----------------------------------
D="$W/many"; mkdir "$D"; (cd "$D" && python3 -I -c '
for i in range(20005): open(f"f{i}.bin", "w").close()')
say "too many items" 2 unsafe_input "has 20005 items, and sweep handles 20000 at a time. Choose a smaller folder." -- "$SWEEP" "$D"
hasnt "too many items: shows the folder" "$LAST" "<name"
say "too many items: stash" 2 unsafe_input "and stash handles 20000 at a time" -- "$STASH" "$D"
rm -rf "$D"

# --- read-only and full disks (rows 67, 123) ---------------------------------
if require hdiutil "disk images are needed for read-only and full disks"; then
  IMAGE="$W/small.dmg"
  if create_disk_image "$IMAGE" 8m HFS+ etudes-stress-107; then
    M="$W/mnt"; mkdir "$M"
    if hdiutil attach "$IMAGE" -nobrowse -mountpoint "$M" >/dev/null 2>&1; then
      MOUNTS+=("$M")
      damaged_state disk
      shots "$M/s"; shots "$M/t"
      hdiutil detach "$M" >/dev/null 2>&1; MOUNTS=()
      hdiutil attach "$IMAGE" -readonly -nobrowse -mountpoint "$M" >/dev/null 2>&1 && MOUNTS+=("$M")
      for tool in sweep stash; do
        if [ "$tool" = sweep ]; then RUN=("$SWEEP" apply "$M/s" --yes); else RUN=("$STASH" "$M/t"); fi
        OUT=$("${RUN[@]}" 2>&1 </dev/null); RC=$?
        clean "$tool on a read-only disk" "$OUT"
        assert_eq 3 "$RC" "$tool on a read-only disk: exit"
        has "$tool on a read-only disk: says so" "$OUT" "the disk is read-only"
        has "$tool on a read-only disk: nothing moved" "$OUT" "Nothing was moved"
        "${RUN[@]}" --json >"$W/out.json" 2>/dev/null </dev/null
        assert_eq permission_denied "$(field reason.code "$W/out.json")" "$tool on a read-only disk: same code"
      done
      hdiutil detach "$M" >/dev/null 2>&1; MOUNTS=()
      hdiutil attach "$IMAGE" -nobrowse -mountpoint "$M" >/dev/null 2>&1 && MOUNTS+=("$M")
      # Fill the disk until a new folder cannot be made.
      head -c 100000000 /dev/zero > "$M/fill" 2>/dev/null
      i=0; while [ "$i" -lt 4000 ] && mkdir "$M/d$i" 2>/dev/null; do i=$((i+1)); done
      if mkdir "$M/probe" 2>/dev/null; then
        unproven "a full disk" "this file system still made a folder when full"
      else
        for tool in sweep stash; do
          if [ "$tool" = sweep ]; then RUN=("$SWEEP" apply "$M/s" --yes); else RUN=("$STASH" "$M/t"); fi
          OUT=$("${RUN[@]}" 2>&1 </dev/null); RC=$?
          clean "$tool on a full disk" "$OUT"
          assert_eq 3 "$RC" "$tool on a full disk: exit"
          has "$tool on a full disk: says so" "$OUT" "the disk is full"
          has "$tool on a full disk: next step" "$OUT" "Free some space"
          "${RUN[@]}" --json >"$W/out.json" 2>/dev/null </dev/null
          assert_eq insufficient_space "$(field reason.code "$W/out.json")" "$tool on a full disk: code"
          assert_eq refused "$(field status "$W/out.json")" "$tool on a full disk: status"
        done
      fi
      hdiutil detach "$M" >/dev/null 2>&1; MOUNTS=()
    else
      unproven "disk messages" "the disk image would not attach"
    fi
  else
    unproven "disk messages" "the disk image could not be made"
  fi
fi

# --- an undo whose record cannot be saved says the files are back (row 123) --
damaged_state saved
D="$W/saved"; shots "$D"
assert_exit 0 "unsaved record: apply" -- "$SWEEP" apply "$D" --yes
chflags uchg "$ETUDE_STATE_DIR"
OUT=$("$SWEEP" undo "$D" 2>&1 </dev/null); RC=$?
chflags nouchg "$ETUDE_STATE_DIR"
clean "unsaved record" "$OUT"
case "$OUT" in
  *"could not be updated"*)
    assert_eq 3 "$RC" "unsaved record: exit"
    has "unsaved record: next undo" "$OUT" "The next sweep undo will see which files are already home"
    OUT=$("$SWEEP" undo "$D" 2>&1 </dev/null); RC=$?
    clean "unsaved record: next undo" "$OUT"
    hasnt "unsaved record: next undo does not call them gone" "$OUT" "not where apply left"
    ;;
  *) unproven "unsaved record" "the record was still saved under a read-only folder" ;;
esac
assert_intact "$D" 3 "unsaved record: every file is back"
