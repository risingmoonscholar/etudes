#!/usr/bin/env bash
# A failure says whether anything moved, and any step it prints works and touches
# only the work that failed. Extra words and repeated flags are refused, a mistyped
# group is named, one bad key gets one answer, and the home folder needs its flag.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir) || exit 1
trap 'chmod -R u+rwx "$W" 2>/dev/null; rm -rf "$W"' EXIT
W=$(cd "${W:?}" && pwd -P) && [ -n "$W" ] || exit 1
if [ "$(id -u)" = 0 ]; then
  unproven "failure steps" "root ignores file modes"
  exit 0
fi
export HOME="$W/home" ETUDE_STATE_DIR="$W/state" SWEEP_GRACE_SECS=0
mkdir -p "$HOME" "$ETUDE_STATE_DIR"

shots() {  # shots DIR: three files that group as Screenshots
  mkdir -p "$1"
  for n in 1 2 3; do printf "$n" > "$1/Screenshot 2026-01-0$n at 10.00.00.png"; done
}
has() { case "$2" in *"$3"*) pass "$1" ;; *) fail "$1: wanted '$3' in: $2" ;; esac; }
hasnt() { case "$2" in *"$3"*) fail "$1: found '$3' in: $2" ;; *) pass "$1" ;; esac; }
no_crash() { assert_no_panic "$1" "$2"; }
same_tree() {  # same_tree DIR SNAPSHOT LABEL: DIR still matches the snapshot taken earlier
  snapshot_tree "$1" "$W/now.snap"
  assert_snapshot_eq "$2" "$W/now.snap" "$3"
}
json_field() {  # json_field 'status' FILE or json_field 'reason.code' FILE
  python3 -I - "$1" "$2" <<'PY'
import json, sys
value = json.load(open(sys.argv[2]))
for key in sys.argv[1].split("."):
    value = value[key]
print(value)
PY
}
run_step() {  # run_step 'PRINTED LINE': run a printed step exactly as a person would paste it
  local tool="${1%% *}" rest="${1#* }"
  case "$tool" in
    sweep) eval "\"$SWEEP\" $rest" ;;
    stash) eval "\"$STASH\" $rest" ;;
    *) fail "printed step names an unknown tool: $1"; return 9 ;;
  esac
}
printed_step() {  # printed_step OUT: the command after "back: "
  printf '%s\n' "$1" | sed -n 's/.*back: //p' | head -1
}

# --- sweep: folder A succeeds, folder B fails having moved nothing ---------
A="$W/A"; shots "$A"
assert_exit 0 "apply on folder A succeeds" -- "$SWEEP" apply "$A" --yes
A_SNAP="$W/a.snap"; snapshot_tree "$A" "$A_SNAP"

no_moves() {  # no_moves LABEL DIR WANT_CODE [ARGS]: apply moved nothing and says so
  local label="$1" dir="$2" want="$3" out code
  out=$("$SWEEP" apply "$dir" --yes 2>&1 </dev/null); code=$?
  no_crash "$label" "$out"
  assert_eq "$want" "$code" "$label: exit"
  has "$label says nothing moved" "$out" "Nothing was moved, so there is nothing to undo."
  hasnt "$label prints no undo step" "$out" "sweep undo"
  hasnt "$label does not call anything resumable" "$out" "resumable"
  "$SWEEP" apply "$dir" --yes --json >"$W/out.json" 2>/dev/null </dev/null
  assert_eq refused "$(json_field status "$W/out.json")" "$label: status refused"
}

B="$W/B"; shots "$B"; printf 'file' > "$B/Screenshots"
no_moves "a file in the group folder's place" "$B" 2
assert_intact "$B" 4 "that failure moved nothing"

C="$W/C"; shots "$C"; mkdir "$C/Screenshots"; printf 'x' > "$C/Screenshots/Screenshot 2026-01-01 at 10.00.00.png"
OUT=$("$SWEEP" apply "$C" --yes 2>&1 </dev/null)
no_crash "a collision" "$OUT"
hasnt "a collision prints no undo step" "$OUT" "sweep undo"

D="$W/D"; shots "$D"; chmod 555 "$D"
no_moves "a read-only folder" "$D" 3
chmod 755 "$D"
assert_intact "$D" 3 "the read-only failure moved nothing"

# The move itself fails: the group folder this attempt made must not stay behind.
E="$W/E/sub"; shots "$E"; chmod 555 "$E"
OUT=$("$SWEEP" apply "$W/E" --yes --depth 2 2>&1 </dev/null); CODE=$?
assert_eq 3 "$CODE" "a denied first move exits 3"
has "a denied first move says nothing moved" "$OUT" "Nothing was moved"
[ ! -e "$W/E/Screenshots" ] && pass "the empty group folder was removed" || fail "the failed attempt left an empty group folder"
OUT=$("$SWEEP" undo "$W/E" 2>&1 </dev/null); CODE=$?
assert_eq 1 "$CODE" "undo of that folder finds nothing to do"
chmod 755 "$E"

# The journal store cannot be written.
F="$W/F"; shots "$F"; : > "$W/state-file"
OUT=$(ETUDE_STATE_DIR="$W/state-file" "$SWEEP" apply "$F" --yes 2>&1 </dev/null); CODE=$?
no_crash "an unusable journal store" "$OUT"
assert_eq 3 "$CODE" "sweep with an unusable journal store exits 3"
has "sweep names the undo records folder" "$OUT" "state-file"
has "sweep says nothing moved" "$OUT" "Nothing was moved"
hasnt "sweep prints a bare os error" "$OUT" "os error"
ETUDE_STATE_DIR="$W/state-file" "$SWEEP" apply "$F" --yes --json >"$W/out.json" 2>/dev/null </dev/null
assert_eq journal_write_failed "$(json_field reason.code "$W/out.json")" "sweep: a failed write has its own code"
assert_eq after_change "$(json_field reason.retry "$W/out.json")" "sweep: a failed write says after_change"
assert_eq refused "$(json_field status "$W/out.json")" "sweep: a failed write that moved nothing is refused"

# Every failure above left folder A alone, and bare undo is still A's to give.
same_tree "$A" "$A_SNAP" "no failure touched folder A"

# --- sweep: a partial failure names the count and the folder ---------------
P="$W/it's partial"; mkdir -p "$P/a" "$P/z"
for n in 1 2; do printf "$n" > "$P/a/Screenshot 2026-01-0$n at 10.00.00.png"; done
printf 3 > "$P/z/Screenshot 2026-01-03 at 10.00.00.png"; chmod 555 "$P/z"
OUT=$("$SWEEP" apply "$P" --yes --depth 2 2>&1 </dev/null); CODE=$?
no_crash "a partial apply" "$OUT"
assert_eq 3 "$CODE" "a partial apply exits 3"
has "a partial apply names the count" "$OUT" "2 files were moved before this stopped."
hasnt "a partial apply does not say nothing moved" "$OUT" "Nothing was moved"
STEP=$(printed_step "$OUT")
case "$STEP" in
  "sweep undo '"*"partial'") pass "a partial apply prints undo with the folder filled in" ;;
  *) fail "a partial apply printed: $STEP" ;;
esac
chmod 755 "$P/z"
UNDONE=$(run_step "$STEP" 2>&1); CODE=$?
assert_eq 0 "$CODE" "the printed undo step works"
no_crash "the printed undo step" "$UNDONE"
[ -f "$P/a/Screenshot 2026-01-01 at 10.00.00.png" ] && pass "the printed step put the files back" || fail "the printed step did not put the files back: $UNDONE"
same_tree "$A" "$A_SNAP" "following what the failed folder printed did not touch folder A"

# And JSON sees a partial result as incomplete.
J="$W/J"; mkdir -p "$J/a" "$J/z"
for n in 1 2; do printf "$n" > "$J/a/Screenshot 2026-01-0$n at 10.00.00.png"; done
printf 3 > "$J/z/Screenshot 2026-01-03 at 10.00.00.png"; chmod 555 "$J/z"
"$SWEEP" apply "$J" --yes --depth 2 --json >"$W/out.json" 2>/dev/null </dev/null
assert_eq incomplete "$(json_field status "$W/out.json")" "a partial apply is status incomplete"
chmod 755 "$J/z"

# --- stash: the same rule -------------------------------------------------
G="$W/G"; mkdir -p "$G"; printf 'a' > "$G/a.pdf"; chmod 555 "$G"
OUT=$("$STASH" "$G" 2>&1 </dev/null); CODE=$?
no_crash "stash on a read-only folder" "$OUT"
assert_eq 3 "$CODE" "stash on a read-only folder exits 3"
has "stash names the folder and the cause" "$OUT" "could not move anything out of $G: permission denied"
has "stash says nothing moved" "$OUT" "Nothing was moved, so there is nothing to restore."
hasnt "stash prints no pop step" "$OUT" "stash pop"
hasnt "stash drops 'further'" "$OUT" "further"
"$STASH" "$G" --json >"$W/out.json" 2>/dev/null </dev/null
assert_eq refused "$(json_field status "$W/out.json")" "a stash that moved nothing is refused"
chmod 755 "$G"

H="$W/H"; mkdir -p "$H"; printf 'a' > "$H/a.pdf"
OUT=$(ETUDE_STATE_DIR="$W/state-file" "$STASH" "$H" 2>&1 </dev/null); CODE=$?
no_crash "stash with an unusable journal store" "$OUT"
assert_eq 3 "$CODE" "stash with an unusable journal store exits 3"
has "stash gives the cause of a failed write" "$OUT" "cannot save the undo record in $W/state-file"
has "stash says nothing moved after a failed write" "$OUT" "Nothing was moved"
hasnt "stash prints no pop step after a failed write" "$OUT" "stash pop"
ETUDE_STATE_DIR="$W/state-file" "$STASH" "$H" --json >"$W/out.json" 2>/dev/null </dev/null
assert_eq journal_write_failed "$(json_field reason.code "$W/out.json")" "stash: a failed write has its own code"
[ -z "$(ls -A "$H" | grep -v a.pdf)" ] && pass "the failed stash left no holding folder" || fail "the failed stash left: $(ls -A "$H")"

Q="$W/Q"; mkdir -p "$Q/zz"; printf 'a' > "$Q/a.pdf"; printf 'b' > "$Q/b.pdf"; chmod 555 "$Q/zz"
OUT=$("$STASH" "$Q" 2>&1 </dev/null); CODE=$?
no_crash "a partial stash" "$OUT"
assert_eq 3 "$CODE" "a partial stash exits 3"
has "a partial stash names the count" "$OUT" "2 items were moved before this stopped."
STEP=$(printed_step "$OUT")
case "$STEP" in
  "stash pop --id "?*) pass "a partial stash prints pop with the id" ;;
  *) fail "a partial stash printed: $STEP" ;;
esac
chmod 755 "$Q/zz"
POPPED=$(run_step "$STEP" 2>&1); CODE=$?
assert_eq 0 "$CODE" "the printed pop step works"
[ -f "$Q/a.pdf" ] && [ -f "$Q/b.pdf" ] && pass "the printed pop step restored the items" || fail "the printed pop step did not restore: $POPPED"

# --- extra words change nothing ------------------------------------------
: > "$ETUDE_STATE_DIR/sweep-keepme.journal"
STATE_SNAP="$W/state.snap"; snapshot_tree "$ETUDE_STATE_DIR" "$STATE_SNAP"
X="$W/X"; shots "$X"; Y="$W/Y"; shots "$Y"
X_SNAP="$W/x.snap"; snapshot_tree "$X" "$X_SNAP"
refused_word() {  # refused_word LABEL WORD -- CMD...
  local label="$1" word="$2"; shift 3
  local out code
  out=$("$@" 2>&1 </dev/null); code=$?
  no_crash "$label" "$out"
  assert_eq 2 "$code" "$label exits 2"
  has "$label names the word" "$out" "$word"
}
refused_word "sweep apply A B" "$Y" -- "$SWEEP" apply "$X" "$Y" --yes
refused_word "sweep undo A B" "$Y" -- "$SWEEP" undo "$X" "$Y"
refused_word "sweep A B" "$Y" -- "$SWEEP" "$X" "$Y"
refused_word "sweep review A B" "$Y" -- "$SWEEP" review "$X" "$Y"
refused_word "sweep verify x" "typo" -- "$SWEEP" verify typo
refused_word "sweep version x" "typo" -- "$SWEEP" version typo
refused_word "sweep forget xyz" "xyz" -- "$SWEEP" forget xyz
same_tree "$X" "$X_SNAP" "the refused commands moved nothing"
same_tree "$ETUDE_STATE_DIR" "$STATE_SNAP" "sweep forget xyz removed no journal"
rm -f "$ETUDE_STATE_DIR/sweep-keepme.journal"

# --- repeated flags and flag mistakes ------------------------------------
R="$W/R"; mkdir -p "$R"; printf 'a' > "$R/a.pdf"
mkdir -p "$W/z"; printf 'h' > "$W/z/member"; (cd "$W/z" && /usr/bin/zip -q ../arch.zip member)
refused_word "stash --for twice" "--for" -- "$STASH" "$R" --for 1d --for 2d
refused_word "unpack --into twice" "--into" -- "$UNPACK" "$W/arch.zip" --into "$W/o1" --into "$W/o2"
refused_word "unpack --max-size twice" "--max-size" -- "$UNPACK" "$W/arch.zip" --max-size 1G --max-size 2G
[ -f "$R/a.pdf" ] && [ ! -e "$W/o1" ] && [ ! -e "$W/o2" ] && [ ! -e "$W/arch" ] && pass "repeated flags changed nothing" || fail "a repeated flag still acted"
refused_word "stash status a b" "takes one folder" -- "$STASH" status "$R" "$W/z"
refused_word "stash status --paths" "--paths needs --all" -- "$STASH" status "$R" --paths

# --- silent wrong answers ------------------------------------------------
OUT=$("$SWEEP" review "$X" 2>&1 </dev/null); CODE=$?
assert_eq 2 "$CODE" "sweep review with no terminal exits 2"
has "sweep review says why" "$OUT" "needs a terminal"
OUT=$("$SWEEP" apply "$X" --only Nope 2>&1 </dev/null); CODE=$?
assert_eq 2 "$CODE" "--only with an unknown group exits 2"
has "--only names the typed group and the real ones" "$OUT" 'no group named "Nope" here. Groups: Screenshots.'
"$SWEEP" apply "$X" --only Nope --json >"$W/out.json" 2>/dev/null </dev/null
assert_eq invalid_arguments "$(json_field reason.code "$W/out.json")" "--only with an unknown group is invalid_arguments"
EXPORT=$("$SWEEP" "$X" --export-plan "$W/x.plan" 2>&1 </dev/null)
DIGEST=$(printf '%s\n' "$EXPORT" | grep -Eo '[0-9a-f]{64}' | head -1)
OUT=$("$SWEEP" apply --plan "$W/x.plan" --plan-digest "$DIGEST" --only Nope 2>&1 </dev/null); CODE=$?
assert_eq 2 "$CODE" "--only on a plan with an unknown group exits 2"
has "--only on a plan names the groups" "$OUT" 'no group named "Nope" in this plan. Groups: Screenshots.'
same_tree "$X" "$X_SNAP" "--only with an unknown group moved nothing"

# forget: a supplied key is not a refusal when the journals are gone
Z="$W/Z"; shots "$Z"; FS="$W/forget-state"; mkdir -p "$FS"
ETUDE_STATE_DIR="$FS" "$SWEEP" apply "$Z" --yes >/dev/null 2>&1 </dev/null
OUT=$(ETUDE_STATE_DIR="$FS" "$SWEEP" forget 2>&1 </dev/null); CODE=$?
assert_eq 0 "$CODE" "forget with a supplied key exits 0"
has "forget says it removed the journal" "$OUT" "Removed 1 journal from $FS."
has "forget says what it could not destroy" "$OUT" "supplied key"
hasnt "forget has no plural in brackets" "$OUT" "journal(s)"
[ -z "$(find "$FS" -name 'sweep-*.journal')" ] && pass "forget removed the journals" || fail "forget left journals"
# forget: an unreadable folder is not "removed 0"
LOCKED="$W/locked-state"; mkdir -p "$LOCKED"; : > "$LOCKED/sweep-a.journal"; chmod 000 "$LOCKED"
OUT=$(ETUDE_STATE_DIR="$LOCKED" "$SWEEP" forget 2>&1 </dev/null); CODE=$?
chmod 700 "$LOCKED"
no_crash "forget on an unreadable folder" "$OUT"
if [ -f "$LOCKED/sweep-a.journal" ]; then
  assert_eq 3 "$CODE" "forget on an unreadable folder exits 3"
  has "forget names the unreadable folder" "$OUT" "could not read $LOCKED: permission denied"
  has "forget says the journals stayed" "$OUT" "Journals were not removed"
  hasnt "forget does not report removing none" "$OUT" "Removed 0"
else
  unproven "forget on an unreadable folder" "startup repaired the state folder's mode and removed the journal"
fi

# --- one bad key, one answer ---------------------------------------------
"$STASH" "$R" >/dev/null 2>&1 </dev/null   # gives the keyed commands a record to find
key_refused() {  # key_refused NAMES_NO_JOURNAL LABEL -- CMD...
  local names="$1" label="$2"; shift 3
  local out code
  out=$(ETUDE_JOURNAL_KEY=not-a-key "$@" 2>&1 >"$W/key.json" </dev/null); code=$?
  no_crash "$label" "$out"
  assert_eq 2 "$code" "$label: bad key exits 2"
  assert_eq journal_key_unavailable "$(json_field reason.code "$W/key.json")" "$label: bad key has one code"
  assert_eq after_change "$(json_field reason.retry "$W/key.json")" "$label: bad key is not 'later'"
  assert_eq refused "$(json_field status "$W/key.json")" "$label: bad key is refused"
  has "$label says how to fix the key" "$out" "ETUDE_JOURNAL_KEY"
  if [ "$names" = yes ]; then
    has "$label names --no-journal, which it takes" "$out" "--no-journal"
  else
    hasnt "$label does not offer --no-journal" "$out" "--no-journal"
  fi
}
K="$W/K"; shots "$K"; mkdir -p "$W/KS"; printf 'k' > "$W/KS/k.pdf"
key_refused yes "sweep apply" -- "$SWEEP" apply "$K" --yes --json
key_refused yes "stash" -- "$STASH" "$W/KS" --json
key_refused no "sweep apply --map" -- "$SWEEP" apply "$K" --yes --map xyz=Other --json
key_refused no "sweep undo" -- "$SWEEP" undo --json
key_refused no "stash pop" -- "$STASH" pop --latest --json
key_refused no "stash inspect" -- "$STASH" inspect --id x --json
key_refused no "stash select" -- "$STASH" select "$W/KS/k.pdf" --json
key_refused no "stash status" -- "$STASH" status "$W/KS" --json
key_refused no "stash status --all" -- "$STASH" status --all --json
# sweep status and stash status --id read the store first: give each a journal.
"$SWEEP" apply "$K" --yes >/dev/null 2>&1 </dev/null
key_refused no "sweep status" -- "$SWEEP" status "$K" --json
printf 'not a record' > "$ETUDE_STATE_DIR/stash-x.journal"
key_refused no "stash status --id" -- "$STASH" status --id x --json
rm -f "$ETUDE_STATE_DIR/stash-x.journal"
"$SWEEP" undo "$K" >/dev/null 2>&1 </dev/null

# A damaged record never heals by waiting.
printf 'not a record' > "$ETUDE_STATE_DIR/sweep-damaged.journal"
"$SWEEP" status --id damaged --json >"$W/out.json" 2>/dev/null </dev/null
assert_eq journal_unreadable "$(json_field reason.code "$W/out.json")" "a damaged record is journal_unreadable"
assert_eq after_change "$(json_field reason.retry "$W/out.json")" "journal_unreadable says after_change"
assert_eq journal "$(json_field reason.subject.kind "$W/out.json")" "journal_unreadable names the journal"
rm -f "$ETUDE_STATE_DIR/sweep-damaged.journal"

# --- the home folder needs its flag --------------------------------------
printf 'h' > "$HOME/keep.pdf"; mkdir -p "$HOME/Documents"
HOME_SNAP="$W/home.snap"; snapshot_tree "$HOME" "$HOME_SNAP"
home_refused() {  # home_refused LABEL -- CMD...
  local label="$1"; shift 2
  local out code
  out=$(cd "$HOME" && "$@" 2>&1 </dev/null); code=$?
  no_crash "$label" "$out"
  assert_eq 2 "$code" "$label exits 2"
  has "$label says it is the home folder" "$out" "home folder"
  has "$label says nothing moved" "$out" "Nothing was moved"
  has "$label names the flag" "$out" "--allow-home"
}
home_refused "bare stash in the home folder" -- "$STASH"
home_refused "stash HOME" -- "$STASH" "$HOME"
home_refused "stash HOME/" -- "$STASH" "$HOME/"
home_refused "stash ." -- "$STASH" .
home_refused "stash --for 1d in the home folder" -- "$STASH" --for 1d
ln -s "$HOME" "$W/home-link"
home_refused "stash through a link to HOME" -- "$STASH" "$W/home-link"
same_tree "$HOME" "$HOME_SNAP" "the home refusals moved nothing"
"$STASH" "$HOME" --json >"$W/out.json" 2>/dev/null </dev/null
assert_eq unsafe_input "$(json_field reason.code "$W/out.json")" "the home refusal is unsafe_input"
has "stash help names --allow-home" "$("$STASH" help 2>&1)" "--allow-home"
has "the contract names --allow-home" "$("$STASH" contract --json 2>&1)" '"flag":"--allow-home"'
OUT=$("$STASH" "$HOME" --allow-home 2>&1 </dev/null); CODE=$?
assert_eq 0 "$CODE" "--allow-home stashes the (fake) home folder"
[ ! -e "$HOME/keep.pdf" ] && pass "--allow-home moved the items" || fail "--allow-home moved nothing: $OUT"
assert_exit 0 "and pop brings them back" -- "$STASH" pop "$HOME"
same_tree "$HOME" "$HOME_SNAP" "the home folder is whole again"
