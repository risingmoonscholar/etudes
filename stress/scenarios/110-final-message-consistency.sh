#!/usr/bin/env bash
# One condition gets one answer across the tools: a missing folder reads the same
# everywhere, and counts agree with their nouns. What a failed undo or pop prints
# is true, and its printed steps work. A scan does not offer an apply it knows will
# refuse. A relative archive gets the same free-space floor as an absolute one.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir)
MOUNTS=()
cleanup() {
  for m in ${MOUNTS[@]+"${MOUNTS[@]}"}; do hdiutil detach "$m" -force >/dev/null 2>&1; done
  chflags -R nouchg "${W:?}" 2>/dev/null
  chmod -R u+rwx "${W:?}" 2>/dev/null; rm -rf "${W:?}"
}
trap cleanup EXIT
W=$(cd "${W:?}" && pwd -P) && [ -n "$W" ] || exit 1
require python3 "the checks read JSON with Python" || exit 0
export HOME="$W/home" ETUDE_STATE_DIR="$W/state" SWEEP_GRACE_SECS=0
mkdir -p "$HOME" "$ETUDE_STATE_DIR"

BANNED='replan|journal seal|malformed|observations|listing failed|extractor|staging|publish|holding locator|holding parent|object identity|member row|dispatched|os error|panicked'
BADCOUNT='(^|[^0-9/,.])1 (files|items|entries|paths|operations|groups|records|folders|journals|tags|bytes)\b|\([a-z]*s\)|[a-z]\(s\)'
clean() {  # clean LABEL OUT: no panic, no internal words, no wrong plural
  assert_no_panic "$1" "$2"
  if printf '%s' "$2" | grep -Eqi "$BANNED"; then fail "$1: internal word in: $2"; else pass "$1: plain words"; fi
  if printf '%s' "$2" | grep -Eq "$BADCOUNT"; then fail "$1: wrong plural in: $2"; else pass "$1: counts agree"; fi
}
has() { case "$2" in *"$3"*) pass "$1" ;; *) fail "$1: wanted '$3' in: $2" ;; esac; }
hasnt() { case "$2" in *"$3"*) fail "$1: found '$3' in: $2" ;; *) pass "$1" ;; esac; }
field() {  # field 'reason.code' FILE
  python3 -I - "$1" "$2" <<'PY'
import json, sys
value = json.load(open(sys.argv[2]))
for key in sys.argv[1].split("."):
    value = value[key]
print(json.dumps(value) if isinstance(value, (bool, list)) else value)
PY
}
shots() {  # shots DIR [COUNT]: files that group as Screenshots
  local n
  mkdir -p "$1"
  for n in $(seq 1 "${2:-3}"); do printf "$n" > "$1/Screenshot 2026-01-0$n at 10.00.00.png"; done
}
step_after() {  # step_after 'WORDS: ' TEXT: the command printed after those words
  printf '%s\n' "$2" | sed -n "s/.*$1//p" | head -1
}
run_step() {  # run_step 'PRINTED LINE': run a printed step as a person would paste it
  local tool="${1%% *}" rest="${1#* }"
  case "$tool" in
    sweep) eval "\"$SWEEP\" $rest" ;;
    stash) eval "\"$STASH\" $rest" ;;
    *) fail "printed step names an unknown tool: $1"; return 9 ;;
  esac
}

# --- a missing folder: one answer on every command --------------------------
mkdir -p "$W/plain"; printf x > "$W/plain/a.txt"
for gone in "$W/absent" "$W/plain/a.txt/under"; do
  for tool_args in "sweep:" "sweep:apply --yes" "sweep:undo" "sweep:status" "stash:" "stash:status" "stash:pop" "stash:pop --latest"; do
    tool=${tool_args%%:*}; words=${tool_args#*:}
    bin=$SWEEP; [ "$tool" = stash ] && bin=$STASH
    # The verb comes first, then the folder, then any flag.
    verb=${words%% *}; flags=""; case "$words" in *" "*) flags=${words#* } ;; esac
    label="$tool $words on a missing folder"
    # shellcheck disable=SC2086
    OUT=$("$bin" $verb "$gone" $flags 2>&1 </dev/null); RC=$?
    clean "$label" "$OUT"
    assert_eq 3 "$RC" "$label: exit"
    has "$label: words" "$OUT" "$tool: no folder at $gone."
    # shellcheck disable=SC2086
    "$bin" $verb "$gone" $flags --json >"$W/out.json" 2>/dev/null </dev/null
    assert_eq error "$(field status "$W/out.json")" "$label: status"
    assert_eq missing_input "$(field reason.code "$W/out.json")" "$label: reason"
  done
done
OUT=$("$SWEEP" review "$W/absent" 2>&1 </dev/null); RC=$?
assert_eq 3 "$RC" "sweep review on a missing folder: exit"
has "sweep review on a missing folder: words" "$OUT" "sweep: no folder at $W/absent."
OUT=$("$STASH" pop "$W/plain/a.txt" 2>&1 </dev/null); RC=$?
assert_eq 3 "$RC" "stash pop on a file: exit"
has "stash pop on a file: words" "$OUT" "is a file. Give the folder you stashed."

# --- counts agree with their nouns -------------------------------------------
SINGLE="$W/single"; mkdir -p "$SINGLE"; printf 1 > "$SINGLE/a.xyz"
OUT=$("$SWEEP" "$SINGLE" 2>&1 </dev/null)
clean "scan of one item" "$OUT"
has "scan of one item: count" "$OUT" "Scanned 1 item "
# Only a proposal makes a group of one file: every scan group needs three.
ONE="$W/one"; shots "$ONE"; mkdir "$ONE/Reading"
EXPORT=$("$SWEEP" "$ONE" --export-plan "$W/one.plan" --json 2>/dev/null </dev/null)
printf '%s' "$EXPORT" >"$W/one.json"
DIGEST=$(field details.binding.plan_digest "$W/one.json")
python3 -I - "$W/one.json" "$DIGEST" >"$W/one.proposal" <<'PY'
import json, sys
items = json.load(open(sys.argv[1]))["details"]["items"]
print(json.dumps({"schema_version": 1, "plan_digest": sys.argv[2], "rest": "leave", "decisions": [
    {"item": items[0]["id"], "folder": "Reading", "mode": "require_existing"}]}))
PY
OUT=$("$SWEEP" propose --plan "$W/one.plan" --plan-digest "$DIGEST" --proposal "$W/one.proposal" --export-plan "$W/one2.plan" 2>&1 </dev/null)
clean "proposal of one file" "$OUT"
has "proposal of one file: count" "$OUT" "1 file to move"
DIGEST2=$(printf '%s\n' "$OUT" | sed -n 's/.*Digest: //p')
OUT=$("$SWEEP" apply --plan "$W/one2.plan" --plan-digest "$DIGEST2" --yes 2>&1 </dev/null)
clean "apply of one file" "$OUT"
has "apply of one file: file" "$OUT" "Moved 1 file."
mv "$ONE/Reading" "$ONE/Gone"
OUT=$("$SWEEP" undo "$ONE" 2>&1 </dev/null)
clean "undo of one lost file" "$OUT"
has "undo of one lost file: count" "$OUT" "Restored 0 of 1 file."
has "undo of one lost file: deleted" "$OUT" "If you deleted it, there is nothing to restore."
mv "$ONE/Gone" "$ONE/Reading"
assert_exit 0 "undo of one file: after moving it back" -- "$SWEEP" undo "$ONE"
ONE2="$W/one2"; mkdir -p "$ONE2"; printf 1 > "$ONE2/a.txt"
OUT=$("$STASH" "$ONE2" 2>&1 </dev/null)
clean "stash of one item" "$OUT"
has "stash of one item: count" "$OUT" "Stashed 1 item."
OUT=$("$STASH" status "$ONE2" 2>&1 </dev/null)
clean "status of one item" "$OUT"
has "status of one item: count" "$OUT" "1 item in 1 operation"
ID=$("$STASH" status "$ONE2" --json 2>/dev/null </dev/null | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["details"]["id"])' 2>/dev/null)
if [ -n "$ID" ]; then
  OUT=$("$STASH" inspect --id "$ID" 2>&1 </dev/null)
  clean "inspect of one item" "$OUT"
  has "inspect of one item: count" "$OUT" "1 of 1 item remains held"
else
  unproven "inspect of one item" "status gave no id"
fi
"$STASH" pop "$ONE2" >/dev/null 2>&1 </dev/null
python3 -I - "$W/one.zip" <<'PY'
import sys, zipfile
info = zipfile.ZipInfo("one.txt"); info.external_attr = 0o100644 << 16
with zipfile.ZipFile(sys.argv[1], "w") as z:
    z.writestr(info, "hi")
PY
OUT=$("$UNPACK" "$W/one.zip" --list 2>&1 </dev/null)
clean "list of one entry" "$OUT"
has "list of one entry: count" "$OUT" "1 entry"
OUT=$("$UNPACK" "$W/one.zip" --into "$W/one-out" 2>&1 </dev/null)
clean "unpack of one entry" "$OUT"

# --- a damaged undo record: only what is true --------------------------------
DSTATE="$W/state-damaged"; mkdir -p "$DSTATE"
D="$W/damaged"; shots "$D"
ETUDE_STATE_DIR="$DSTATE" "$SWEEP" apply "$D" --yes >/dev/null 2>&1 </dev/null
RECORD=$(ls "$DSTATE"/sweep-*.journal | head -1)
python3 -I - "$RECORD" <<'PY'
import sys
path = sys.argv[1]
data = bytearray(open(path, "rb").read())
data[len(data) // 2] ^= 0xff
open(path, "wb").write(data)
PY
snapshot_tree "$D" "$W/d.snap"
for args in "undo" "undo $D"; do
  # shellcheck disable=SC2086
  OUT=$(ETUDE_STATE_DIR="$DSTATE" "$SWEEP" $args 2>&1 </dev/null); RC=$?
  clean "damaged record, sweep $args" "$OUT"
  assert_eq 3 "$RC" "damaged record, sweep $args: exit"
  has "damaged record, sweep $args: says nothing moved" "$OUT" "Nothing was moved."
  hasnt "damaged record, sweep $args: no claim about the files" "$OUT" "still where it put them"
  has "damaged record, sweep $args: says what it cannot know" "$OUT" "cannot say where the files from that apply are"
done
snapshot_tree "$D" "$W/d2.snap"
assert_snapshot_eq "$W/d.snap" "$W/d2.snap" "damaged record: nothing moved"

# --- sweep undo: files not where apply left them ------------------------------
USTATE="$W/state-undo"; mkdir -p "$USTATE"
A="$W/older"; C="$W/newer"; shots "$A"; shots "$C"
ETUDE_STATE_DIR="$USTATE" "$SWEEP" apply "$A" --yes >/dev/null 2>&1 </dev/null
sleep 1
ETUDE_STATE_DIR="$USTATE" "$SWEEP" apply "$C" --yes >/dev/null 2>&1 </dev/null
mv "$C/Screenshots" "$C/Moved"
OUT=$(ETUDE_STATE_DIR="$USTATE" "$SWEEP" undo 2>&1 </dev/null); RC=$?
clean "lost files, bare undo" "$OUT"
assert_eq 3 "$RC" "lost files, bare undo: exit"
has "lost files: restored none" "$OUT" "Restored 0 of 3 files."
has "lost files: the moved case" "$OUT" "If you moved that folder, move it back and run: sweep undo '$C'"
has "lost files: the deleted case" "$OUT" "If you deleted them, there is nothing to restore."
has "lost files: the older apply" "$OUT" "To undo the apply of $A, run: sweep undo '$A'"
# Named, the record stops nothing else, so no step for other work is printed.
OUT1=$(ETUDE_STATE_DIR="$USTATE" "$SWEEP" undo "$C" 2>&1 </dev/null)
has "lost files, named undo: still says what is lost" "$OUT1" "If you moved that folder, move it back and run: sweep undo '$C'"
hasnt "lost files, named undo: no step for other work" "$OUT1" "To undo the apply of"
assert_eq "" "$(ls "$A" | grep -v '^Screenshots$')" "lost files: the older folder was not touched"
# Bare undo must not skip this record and reverse a folder nobody named.
OUT2=$(ETUDE_STATE_DIR="$USTATE" "$SWEEP" undo 2>&1 </dev/null)
has "lost files: bare undo stays on this record" "$OUT2" "not where apply left them ($C/Screenshots)"
if [ -d "$A/Screenshots" ]; then pass "lost files: bare undo left the older folder alone"; else fail "lost files: bare undo reversed the older folder"; fi
# Follow the older-apply step exactly as printed.
STEP=$(step_after 'run: ' "$(printf '%s\n' "$OUT" | grep 'To undo the apply of')")
OUT3=$(export ETUDE_STATE_DIR="$USTATE"; run_step "$STEP" 2>&1 </dev/null); RC=$?
assert_eq 0 "$RC" "older apply step: exit"
has "older apply step: restored" "$OUT3" "Restored 3 files."
assert_eq 3 "$(ls "$A" | grep -c '^Screenshot 2026')" "older apply step: its files are home"
if [ -d "$C/Moved" ] && [ "$(ls "$C/Moved" | wc -l | tr -d ' ')" = 3 ]; then
  pass "older apply step: the other folder was left alone"
else
  fail "older apply step: the other folder changed"
fi
# Follow the moved-folder step.
mv "$C/Moved" "$C/Screenshots"
STEP=$(step_after 'move it back and run: ' "$OUT")
OUT4=$(export ETUDE_STATE_DIR="$USTATE"; run_step "$STEP" 2>&1 </dev/null); RC=$?
assert_eq 0 "$RC" "moved-folder step: exit"
assert_eq 3 "$(ls "$C" | grep -c '^Screenshot 2026')" "moved-folder step: files are home"
# With nothing else on record, no step points at another folder.
E="$W/alone"; shots "$E"
ESTATE="$W/state-alone"; mkdir -p "$ESTATE"
ETUDE_STATE_DIR="$ESTATE" "$SWEEP" apply "$E" --yes >/dev/null 2>&1 </dev/null
rm -rf "$E/Screenshots"
OUT=$(ETUDE_STATE_DIR="$ESTATE" "$SWEEP" undo "$E" 2>&1 </dev/null)
has "deleted files: the deleted case" "$OUT" "If you deleted them, there is nothing to restore."
hasnt "deleted files: no other folder to name" "$OUT" "To undo the apply of"

# --- stash pop: items not where stash left them -------------------------------
PSTATE="$W/state-pop"; mkdir -p "$PSTATE"
F="$W/stash-older"; G="$W/stash-newer"; shots "$F"; shots "$G"
ETUDE_STATE_DIR="$PSTATE" "$STASH" "$F" >/dev/null 2>&1 </dev/null
sleep 1
ETUDE_STATE_DIR="$PSTATE" "$STASH" "$G" >/dev/null 2>&1 </dev/null
HOLD=$(cd "$G" && ls -A | grep '^\.stash-' | head -1)
mv "$G/$HOLD" "$G/.moved-away"
OUT=$(ETUDE_STATE_DIR="$PSTATE" "$STASH" pop "$G" 2>&1 </dev/null); RC=$?
clean "lost items, pop" "$OUT"
assert_eq 3 "$RC" "lost items, pop: exit"
has "lost items: restored none" "$OUT" "Restored 0 of 3 items."
has "lost items: the moved case" "$OUT" "If you moved that folder, move it back and run: stash pop --id "
has "lost items: the deleted case" "$OUT" "If you deleted them, there is nothing to restore."
# A named folder or an id picks its own stash, so the other one is not in the way.
hasnt "lost items, named pop: no step for other work" "$OUT" "instead, run:"
LOST_ID=$(step_after 'move it back and run: stash pop --id ' "$OUT")
OUT1=$(ETUDE_STATE_DIR="$PSTATE" "$STASH" pop --id "$LOST_ID" 2>&1 </dev/null)
has "lost items, pop by id: still says what is lost" "$OUT1" "If you deleted them, there is nothing to restore."
hasnt "lost items, pop by id: no step for other work" "$OUT1" "instead, run:"
OUT1=$(cd "$G" && ETUDE_STATE_DIR="$PSTATE" "$STASH" pop 2>&1 </dev/null)
has "lost items, bare pop: still says what is lost" "$OUT1" "If you deleted them, there is nothing to restore."
hasnt "lost items, bare pop: no step for other work" "$OUT1" "instead, run:"
OUT1=$(ETUDE_STATE_DIR="$PSTATE" "$STASH" pop "$G" --latest 2>&1 </dev/null)
hasnt "lost items, --latest FOLDER: another folder's stash is not in the way" "$OUT1" "instead, run:"
# --latest keeps stopping at this stash, so there the step is the way forward.
OUT=$(ETUDE_STATE_DIR="$PSTATE" "$STASH" pop --latest 2>&1 </dev/null); RC=$?
clean "lost items, --latest" "$OUT"
assert_eq 3 "$RC" "lost items, --latest: exit"
has "lost items: the other stash" "$OUT" "To restore the stash from $F instead, run: stash pop --id "
STEP=$(step_after 'instead, run: ' "$OUT")
OUT3=$(export ETUDE_STATE_DIR="$PSTATE"; run_step "$STEP" 2>&1 </dev/null); RC=$?
assert_eq 0 "$RC" "other stash step: exit"
assert_eq 3 "$(ls "$F" | grep -c '^Screenshot 2026')" "other stash step: its items are home"
if [ -d "$G/.moved-away" ]; then pass "other stash step: the lost stash was left alone"; else fail "other stash step: the lost stash changed"; fi
mv "$G/.moved-away" "$G/$HOLD"
STEP=$(step_after 'move it back and run: ' "$OUT")
OUT4=$(export ETUDE_STATE_DIR="$PSTATE"; run_step "$STEP" 2>&1 </dev/null); RC=$?
assert_eq 0 "$RC" "moved-folder step, pop: exit"
assert_eq 3 "$(ls "$G" | grep -c '^Screenshot 2026')" "moved-folder step, pop: items are home"

# --- a locked file: held and said, or the failed move says so, and the printed
# steps work. Sweep and stash hold a locked item before any move; undo and pop
# meet a lock only when it is set after the item moved. -------------------------
LOCKED="Screenshot 2026-01-02 at 10.00.00.png"
PRIVACY="Privacy & Security"
locked_words() {  # locked_words LABEL OUT FOLDER: the cause, then a step that finds it
  clean "$1" "$2"
  has "$1: says it is locked" "$2" "1 item is locked"
  has "$1: how to unlock" "$2" "choose Get Info and clear Locked, or run chflags nouchg on the item."
  has "$1: how to find it" "$2" "See which with: find '$3' -flags +uchg"
  hasnt "$1: does not blame privacy settings" "$2" "$PRIVACY"
  hasnt "$1: keeps the file's name out" "$2" "Screenshot 2026"
  local found
  found=$(eval "$(step_after 'See which with: ' "$2")" 2>/dev/null)
  has "$1: the find step finds it" "$found" "$LOCKED"
}
fresh_lock() { rm -f "$ETUDE_STATE_DIR"/*.journal; shots "$1"; chflags uchg "$1/$LOCKED"; }

D="$W/lock-stash"; fresh_lock "$D"
OUT=$("$STASH" "$D" 2>&1 </dev/null); RC=$?
assert_eq 3 "$RC" "locked, stash: exit"
locked_words "locked, stash" "$OUT" "$D"
chflags nouchg "$D/$LOCKED"
assert_exit 0 "locked, stash: run again after unlocking" -- "$STASH" "$D"
[ -z "$(ls "$D")" ] && pass "locked, stash: the folder is clear" || fail "locked, stash: still has files"

D="$W/lock-apply"; fresh_lock "$D"
OUT=$("$SWEEP" apply "$D" --yes 2>&1 </dev/null); RC=$?
# Two files left do not group by themselves, so nothing moves and nothing needs undoing.
assert_eq 1 "$RC" "locked, apply: exit"
locked_words "locked, apply" "$OUT" "$D"
chflags nouchg "$D/$LOCKED"
hasnt "locked, apply: no undo step, because nothing moved" "$OUT" "To put it back"
assert_exit 0 "locked, apply: apply works after unlocking" -- "$SWEEP" apply "$D" --yes
assert_eq 3 "$(ls "$D/Screenshots" | wc -l | tr -d ' ')" "locked, apply: every file moved"

D="$W/lock-select"; fresh_lock "$D"
OUT=$("$STASH" select "$D/Screenshot 2026-01-01 at 10.00.00.png" "$D/$LOCKED" 2>&1 </dev/null); RC=$?
assert_eq 3 "$RC" "locked, select: exit"
locked_words "locked, select" "$OUT" "$D"
chflags nouchg "$D/$LOCKED"
STEP=$(step_after 'Restore with: ' "$OUT")
OUT2=$(run_step "$STEP" 2>&1 </dev/null); RC=$?
assert_eq 0 "$RC" "locked, select: the printed step works"
assert_exit 0 "locked, select: select works after unlocking" -- "$STASH" select "$D/Screenshot 2026-01-01 at 10.00.00.png" "$D/$LOCKED"

# A file locked after it moved stops the way back in the same words.
D="$W/lock-undo"; rm -f "$ETUDE_STATE_DIR"/*.journal; shots "$D"
"$SWEEP" apply "$D" --yes >/dev/null 2>&1 </dev/null
chflags uchg "$D/Screenshots/$LOCKED"
OUT=$("$SWEEP" undo "$D" 2>&1 </dev/null); RC=$?
assert_eq 3 "$RC" "locked, undo: exit"
locked_words "locked, undo" "$OUT" "$D"
chflags nouchg "$D/Screenshots/$LOCKED"
STEP=$(step_after 'stopped, run: ' "$OUT")
OUT2=$(run_step "$STEP" 2>&1 </dev/null); RC=$?
assert_eq 0 "$RC" "locked, undo: the printed step works after unlocking"
assert_eq 3 "$(ls "$D" | grep -c '^Screenshot 2026')" "locked, undo: files are home"

D="$W/lock-pop"; rm -f "$ETUDE_STATE_DIR"/*.journal; shots "$D"
"$STASH" "$D" >/dev/null 2>&1 </dev/null
HOLD=$(ls -A "$D" | head -1)
chflags uchg "$D/$HOLD/$LOCKED"
OUT=$("$STASH" pop "$D" 2>&1 </dev/null); RC=$?
assert_eq 3 "$RC" "locked, pop: exit"
locked_words "locked, pop" "$OUT" "$D/$HOLD"
chflags nouchg "$D/$HOLD/$LOCKED"
STEP=$(step_after 'Fix that, then run: ' "$OUT")
OUT2=$(run_step "$STEP" 2>&1 </dev/null); RC=$?
assert_eq 0 "$RC" "locked, pop: the printed step works after unlocking"
assert_eq 3 "$(ls "$D" | grep -c '^Screenshot 2026')" "locked, pop: items are home"

# A locked folder the items are going back to is found the same way.
D="$W/lock-home"; rm -f "$ETUDE_STATE_DIR"/*.journal; shots "$D"
"$STASH" "$D" >/dev/null 2>&1 </dev/null
chflags uchg "$D"
OUT=$("$STASH" pop "$D" 2>&1 </dev/null); RC=$?
assert_eq 3 "$RC" "locked folder, pop: exit"
has "locked folder, pop: says it is locked" "$OUT" "1 item is locked"
has "locked folder, pop: looks in that folder" "$OUT" "See which with: find '$D' -flags +uchg"
chflags nouchg "$D"
rm -f "$ETUDE_STATE_DIR"/*.journal

# --- an empty value is a missing value, in all three tools --------------------
mkdir -p "$W/empty-values"; printf x > "$W/empty-values/one"
empty_value() {  # empty_value LABEL WANT -- COMMAND: exit 2, the flag named, no empty name
  local label="$1" want="$2"; shift 3
  local out rc
  out=$("$@" 2>&1 </dev/null); rc=$?
  clean "$label" "$out"
  assert_eq 2 "$rc" "$label: exit"
  has "$label: says what is missing" "$out" "$want"
  hasnt "$label: no empty name" "$out" '""'
  hasnt "$label: no empty place" "$out" ' at : '
}
empty_value "unpack --into first" "--into needs a folder" -- "$UNPACK" --into '' "$W/one.zip"
empty_value "unpack --into after" "--into needs a folder" -- "$UNPACK" "$W/one.zip" --into ''
empty_value "unpack --max-size" "--max-size needs a size" -- "$UNPACK" "$W/one.zip" --max-size ''
empty_value "select --into" "--into needs a value." -- "$STASH" select --into '' "$W/empty-values/one"
empty_value "select --from0" "--from0 needs a value." -- "$STASH" select --from0 ''
empty_value "select --for" "--for needs a value." -- "$STASH" select --for '' "$W/empty-values/one"
empty_value "stash --for" "--for needs a value." -- "$STASH" "$W/empty-values" --for ''
empty_value "pop --id" "--id needs a value." -- "$STASH" pop --id ''
empty_value "stash --plan" "--plan needs a value." -- "$STASH" "$W/empty-values" --plan ''
empty_value "stash status --id" "--id needs a value." -- "$STASH" status --id ''
for flag in --only --since --depth --map --scheme --plan --plan-digest; do
  empty_value "apply $flag" "$flag needs a value" -- "$SWEEP" apply "$W/empty-values" "$flag" ''
done
empty_value "sweep --depth" "--depth needs a value" -- "$SWEEP" "$W/empty-values" --depth ''
empty_value "sweep status --id" "--id needs a value" -- "$SWEEP" status --id ''
assert_eq x "$(cat "$W/empty-values/one")" "empty values: nothing moved"

# --- review with nobody to answer: only the refusal ----------------------------
D="$W/review-no-terminal"; shots "$D"
OUT=$("$SWEEP" review "$D" 2>&1 </dev/null); RC=$?
assert_eq 2 "$RC" "review, no terminal: exit"
assert_eq "sweep: review needs a terminal. Use \`sweep apply PATH --yes\`
or \`--only NAME\` when running non-interactively." "$OUT" "review, no terminal: prints only the refusal"
assert_eq 3 "$(ls "$D" | wc -l | tr -d ' ')" "review, no terminal: nothing moved"

# --- a scan does not offer an apply that will refuse ---------------------------
B="$W/blocked"; shots "$B"; printf x > "$B/Screenshots"
OUT=$("$SWEEP" "$B" 2>&1 </dev/null); RC=$?
clean "blocked scan" "$OUT"
assert_eq 0 "$RC" "blocked scan: the scan itself finished"
has "blocked scan: says so" "$OUT" "Not ready to apply: a file named \"Screenshots\" is where sweep would make that folder."
has "blocked scan: next step" "$OUT" "Rename it, then scan again."
hasnt "blocked scan: no apply step" "$OUT" "Apply:"
"$SWEEP" "$B" --json >"$W/out.json" 2>/dev/null </dev/null
assert_eq '["Screenshots"]' "$(field details.blocked_folders "$W/out.json")" "blocked scan: JSON names the folder"
OUT=$("$SWEEP" apply "$B" --yes 2>&1 </dev/null); RC=$?
assert_eq 2 "$RC" "blocked apply: still refused"
mv "$B/Screenshots" "$B/renamed-note"
OUT=$("$SWEEP" "$B" 2>&1 </dev/null)
hasnt "after the rename: not blocked" "$OUT" "Not ready to apply"
has "after the rename: apply offered" "$OUT" "Apply:"
assert_exit 0 "after the rename: apply works" -- "$SWEEP" apply "$B" --yes
"$SWEEP" "$ONE" --json >"$W/out.json" 2>/dev/null </dev/null
if python3 -I -c 'import json,sys; sys.exit("blocked_folders" in json.load(open(sys.argv[1]))["details"])' "$W/out.json"; then
  pass "a plan that can apply has no blocked field"
else
  fail "a plan that can apply has a blocked field"
fi

# --- unpack: a relative path gets the same floor as an absolute one ------------
if require hdiutil "disk images are needed for little free space"; then
  if create_disk_image "$W/small.dmg" 8m HFS+ etudes-stress-110; then
    M="$W/mnt"; mkdir "$M"
    if hdiutil attach "$W/small.dmg" -nobrowse -mountpoint "$M" >/dev/null 2>&1; then
      MOUNTS+=("$M")
      cp "$W/one.zip" "$M/a.zip"
      OUT=$(cd "$M" && "$UNPACK" a.zip 2>&1 </dev/null); RC=$?
      clean "bare relative archive, little free space" "$OUT"
      assert_eq 2 "$RC" "bare relative archive: refused"
      has "bare relative archive: the floor" "$OUT" "unpack needs 256.0 MB free to start"
      OUT=$(cd "$M" && "$UNPACK" a.zip --into out 2>&1 </dev/null); RC=$?
      assert_eq 2 "$RC" "bare relative --into: refused"
      has "bare relative --into: the floor" "$OUT" "unpack needs 256.0 MB free to start"
      OUT=$("$UNPACK" "$M/a.zip" 2>&1 </dev/null); RC=$?
      assert_eq 2 "$RC" "absolute archive: refused the same"
      (cd "$M" && "$UNPACK" a.zip --json >"$W/out.json" 2>/dev/null </dev/null)
      assert_eq insufficient_space "$(field reason.code "$W/out.json")" "bare relative archive: reason"
      if [ -e "$M/a" ]; then fail "bare relative archive: wrote a folder"; else pass "bare relative archive: nothing written"; fi
      hdiutil detach "$M" >/dev/null 2>&1; MOUNTS=()
    else
      unproven "bare relative archive" "the image did not attach"
    fi
  else
    unproven "bare relative archive" "the image could not be made"
  fi
  rm -f "$W/small.dmg"
fi
# And with room, a relative path still works from where it is.
REL="$W/rel"; mkdir -p "$REL"; cp "$W/one.zip" "$REL/a.zip"
OUT=$(cd "$REL" && "$UNPACK" a.zip 2>&1 </dev/null); RC=$?
assert_eq 0 "$RC" "relative archive with room: exit"
if [ -f "$REL/a/one.txt" ]; then pass "relative archive with room: extracted beside it"; else fail "relative archive with room: $OUT"; fi

