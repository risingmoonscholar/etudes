#!/usr/bin/env bash
# etude-core now takes each moved item out of the source folder it opened and
# checked, not out of whatever the path names at the moment of the move. All
# three tools go through that code, so all three run here end to end.
#
# sweep apply is paused part way through moving files out of Inbox, confirmed
# stopped by the journal watcher, and Inbox is then swapped for a link to a
# folder outside the root that holds a file with each Inbox file's name.
# On resume the apply must refuse with plan_stale and name Inbox: the outside
# files keep their bytes, nothing of it lands in the root, and the pending file
# stays in the real Inbox. With Inbox put back, sweep undo brings every file
# home with its bytes. stash holds and pops a folder, and unpack extracts a
# synthetic archive, both to the exact bytes. Everything is synthetic, in a
# temp dir, and removed on exit.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
exec </dev/null

if [ "$(uname -s)" != Darwin ]; then
  unproven "moves out of the opened source folder" "the descriptor-relative move is macOS only"
  exit 0
fi
require python3 "builds the fixture tree and reads the --json plan" || exit 0
DUMP="$(dirname "$SWEEP")/journal-dump"
if [ ! -x "$DUMP" ]; then
  unproven "source folder swap mid-apply" "dev-only journal watcher unavailable"
  exit 0
fi

W=$(workdir) || exit 1
PID=""
cleanup() {
  if [ -n "$PID" ]; then
    kill -CONT "$PID" 2>/dev/null || true
    kill -TERM "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true
  fi
  rm -rf "$W"
  eval "$_stress_state_cleanup"
}
trap cleanup EXIT
export HOME="$W/home"; mkdir -p "$HOME"

N=300
build_tree() {
  python3 - "$1" "$N" <<'PY'
import pathlib, sys
d, n = pathlib.Path(sys.argv[1]), int(sys.argv[2])
d.mkdir(parents=True)
for i in range(n):
    (d / f"batch_{i:05d}.csv").write_text(f"synthetic row {i}\n")
PY
}
bytes_bad() {
  python3 - "$1" <<'PY'
import pathlib, re, sys
bad = 0
for p in pathlib.Path(sys.argv[1]).glob("batch_*.csv"):
    i = int(re.search(r"batch_(\d+)\.csv$", p.name).group(1))
    bad += p.read_text() != f"synthetic row {i}\n"
print(bad)
PY
}
count_csv() { find "$1" -maxdepth 1 -type f -name 'batch_*.csv' 2>/dev/null | wc -l | tr -d ' '; }

OUTSIDE="$W/outside"
SENTINEL="OUTSIDE-SOURCE-SECRET-$$-$(python3 -c 'import secrets; print(secrets.token_hex(8))')"
INJECTED=0
CODE=0
for trial in 1 2 3 4; do
  D="$W/attempt-$trial/Desktop"
  ASIDE="$W/attempt-$trial/inbox-aside"
  export ETUDE_STATE_DIR="$W/state-$trial"
  mkdir -p "$ETUDE_STATE_DIR"
  build_tree "$D/Inbox"
  VICTIM=$("$SWEEP" "$D" --depth 2 --since 0 --json 2>/dev/null | python3 -c '
import json, sys
groups = json.load(sys.stdin)["details"]["groups"]
members = next(g for g in groups if g["name"] == "Data")["members"]
print(members[int(len(members) * 0.75)])
')
  NAME=$(basename "$VICTIM")
  # Every name in Inbox also waits outside, so a move through the link would
  # find a file to take whichever one it reached first.
  rm -rf "$OUTSIDE"; mkdir -p "$OUTSIDE"
  for f in "$D/Inbox"/batch_*.csv; do printf '%s' "$SENTINEL" > "$OUTSIDE/$(basename "$f")"; done

  "$SWEEP" apply "$D" --depth 2 --since 0 --yes --json >"$W/apply.out" 2>"$W/apply.err" &
  PID=$!
  if ! WITNESS=$("$DUMP" --watch-apply "$ETUDE_STATE_DIR" "$D" "$VICTIM" "$PID" 2>"$W/watch.err"); then
    kill -CONT "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true
    PID=""
    continue
  fi
  read -r JOURNAL_MOVED JOURNAL_TOTAL PENDING_INDEX <<< "$WITNESS"
  STOPPED=$(ps -o state= -p "$PID" 2>/dev/null)
  if [[ "$STOPPED" != *T* ]] || [ ! -f "$VICTIM" ] || [ -e "$D/Data/$NAME" ]; then
    kill -CONT "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true
    PID=""
    continue
  fi
  # The swap is made only while the apply is confirmed stopped.
  mv "$D/Inbox" "$ASIDE"
  if ln -s "$OUTSIDE" "$D/Inbox" && [ -L "$D/Inbox" ] && [ "$(cat "$D/Inbox/$NAME")" = "$SENTINEL" ]; then
    INJECTED=1
  else
    fail "could not install the outside link while apply was stopped"
    exit 0
  fi
  pass "Inbox swapped for an outside link while apply was confirmed stopped ($JOURNAL_MOVED/$JOURNAL_TOTAL moved; pending index $PENDING_INDEX)"
  kill -CONT "$PID" || { fail "could not resume the stopped apply"; exit 0; }
  wait "$PID"
  CODE=$?
  PID=""
  break
done

if [ "$INJECTED" != 1 ]; then
  unproven "source folder swap mid-apply" "no confirmed stopped apply with a pending file across four trials: $(cat "$W/watch.err")"
  exit 0
fi

ERR=$(cat "$W/apply.err")
assert_no_panic "sweep apply after the source folder swap" "$ERR"
assert_eq plan_stale "$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["reason"]["code"])' "$W/apply.out")" "source swap reason code"
assert_eq 2 "$CODE" "apply refuses once the source folder is swapped (exit $CODE)"
case "$ERR" in
  *"Inbox was moved or replaced while sweep was working, so nothing more was moved out of it."*) pass "the refusal names the source folder and says nothing more left it" ;;
  *) fail "the refusal does not name the source folder: $ERR" ;;
esac
assert_eq "$N" "$(find "$OUTSIDE" -type f | wc -l | tr -d ' ')" "nothing was added to or taken from the outside folder"
assert_eq "$N" "$(grep -lF "$SENTINEL" "$OUTSIDE"/batch_*.csv | wc -l | tr -d ' ')" "every outside file keeps its bytes"
LEAKED=$(find "$D" -type f -exec grep -lF "$SENTINEL" {} + 2>/dev/null | wc -l | tr -d ' ')
assert_eq 0 "$LEAKED" "no outside file reached the root"
[ "$(cat "$ASIDE/$NAME" 2>/dev/null)" = "$(printf 'synthetic row %d' "$((10#${NAME//[!0-9]/}))")" ] \
  && pass "the pending file stays in the real Inbox with its bytes" \
  || fail "the pending file is not in the real Inbox"
assert_eq "$N" "$(( $(count_csv "$ASIDE") + $(count_csv "$D/Data") ))" "every file is either still in Inbox or in Data"

# Put the real Inbox back; undo then brings every moved file home.
rm "$D/Inbox" && mv "$ASIDE" "$D/Inbox"
CODE=0; OUT=$("$SWEEP" undo "$D" 2>&1) || CODE=$?
assert_no_panic "sweep undo after the refused apply" "$OUT"
assert_eq 0 "$CODE" "sweep undo after the refused apply succeeds (exit $CODE)"
assert_eq "$N" "$(count_csv "$D/Inbox")" "all $N files are home after undo"
assert_eq 0 "$(bytes_bad "$D/Inbox")" "every file has its original bytes"
assert_eq 0 "$(count_csv "$D/Data")" "nothing is left in Data"

# stash holds and pops a folder.
S="$W/stash-root"; build_tree "$S"
CODE=0; OUT=$("$STASH" "$S" 2>&1) || CODE=$?
assert_no_panic "stash" "$OUT"
assert_eq 0 "$CODE" "stash clears the folder (exit $CODE)"
assert_eq 0 "$(count_csv "$S")" "the stashed folder holds no batch file"
CODE=0; OUT=$("$STASH" pop "$S" 2>&1) || CODE=$?
assert_no_panic "stash pop" "$OUT"
assert_eq 0 "$CODE" "pop restores it (exit $CODE)"
assert_eq "$N" "$(count_csv "$S")" "all $N files came back"
assert_eq 0 "$(bytes_bad "$S")" "every popped file has its bytes"

# unpack extracts to the exact bytes.
SRC="$W/source"; mkdir -p "$SRC/sub"
printf 'synthetic archive member\n' > "$SRC/member.txt"
printf 'synthetic nested member\n' > "$SRC/sub/nested.txt"
tar -czf "$W/fixture.tar.gz" -C "$SRC" member.txt sub
CODE=0; OUT=$("$UNPACK" "$W/fixture.tar.gz" --into "$W/out" 2>&1) || CODE=$?
assert_no_panic "unpack" "$OUT"
assert_eq 0 "$CODE" "unpack extracts the synthetic archive (exit $CODE)"
[ "$(cat "$W/out/member.txt" 2>/dev/null)" = 'synthetic archive member' ] \
  && [ "$(cat "$W/out/sub/nested.txt" 2>/dev/null)" = 'synthetic nested member' ] \
  && pass "unpack wrote both members with their exact bytes" \
  || fail "unpack did not write the exact bytes"
