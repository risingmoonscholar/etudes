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
  chmod -R u+rwx "${W:?}" 2>/dev/null; rm -rf "${W:?}"
}
trap cleanup EXIT
W=$(cd "$W" && pwd -P)
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

