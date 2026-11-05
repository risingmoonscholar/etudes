#!/usr/bin/env bash
# Pack's unfinished file: never at the requested name, removed when a run stops,
# and removed by a later run after a kill. Cleanup is the one place pack deletes,
# so it is attacked the way any tool that removes files is: it must delete only
# its own file, whatever else is lying beside the archive with a similar name.
# Each timing probe pauses the run first, then injects, then continues.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir)
PIDS=()
cleanup() {
  for p in ${PIDS[@]+"${PIDS[@]}"}; do kill -9 "$p" 2>/dev/null; done
  chmod -R u+rwx "${W:?}" 2>/dev/null; rm -rf "${W:?}"
}
trap cleanup EXIT
W=$(cd "$W" && pwd -P)
export HOME="$W/home" ETUDE_STATE_DIR="$W/state"
mkdir -p "$HOME" "$ETUDE_STATE_DIR"

S="$W/source"; mkdir -p "$S"
for i in 1 2 3 4 5 6 7 8; do dd if=/dev/urandom of="$S/chunk$i.bin" bs=1048576 count=8 status=none; done
mkdir -p "$W/small"; printf 'x\n' > "$W/small/a"
MARK=org.etudes.pack.staging
hex() { python3 -I -c 'import secrets; print(secrets.token_hex(16))'; }
field() { python3 -I -c 'import json,sys; v=json.load(open(sys.argv[2]))
for k in sys.argv[1].split("."): v=v[k]
print(v)' "$1" "$2"; }

# start_paused DEST: run pack, wait for a part-written file, pause pack and tar.
# Sets PID, CHILDREN, STAGE. Fails when the writer finished first.
start_paused() {
  local dest="$1" size name
  # A background job starts with SIGINT ignored. Restore it, as a terminal would.
  perl -e '$SIG{INT}="DEFAULT"; $SIG{HUP}="DEFAULT"; exec @ARGV or exit 127' \
    "$PACK" "$S" --for other --into "$dest" --json >"$W/run.out" 2>"$W/run.err" </dev/null &
  PID=$!; PIDS+=("$PID")
  STAGE=""
  name=$(basename "$dest")
  for _ in $(seq 1 4000); do
    STAGE=$(find "$(dirname "$dest")" -maxdepth 1 -name ".$name.pack-*.partial" -print -quit)
    if [ -n "$STAGE" ]; then
      size=$(stat -f %z "$STAGE" 2>/dev/null || echo 0)
      [ "$size" -gt 0 ] && break
    fi
    kill -0 "$PID" 2>/dev/null || return 1
    sleep 0.002
  done
  CHILDREN=$(pgrep -P "$PID" 2>/dev/null || true)
  kill -STOP "$PID" 2>/dev/null
  for c in $CHILDREN; do kill -STOP "$c" 2>/dev/null || true; done
  [ -n "$STAGE" ] && [ -n "$CHILDREN" ] && [[ "$(ps -o state= -p "$PID" 2>/dev/null)" == *T* ]] && return 0
  resume
  wait "$PID" 2>/dev/null
  return 1
}
resume() {
  for c in $CHILDREN; do kill -CONT "$c" 2>/dev/null || true; done
  kill -CONT "$PID" 2>/dev/null
}
# paused LABEL DEST: start_paused with retries; unproven if it never pauses.
paused() {
  local label="$1" dest="$2" attempt
  for attempt in 1 2 3 4; do
    if start_paused "$dest"; then return 0; fi
    rm -f "${dest:?}"
    echo "    ($label, attempt $attempt: the writer finished first; control reading, not a result)"
  done
  unproven "$label" "the writer finished before it could be paused in 4 attempts"
  return 1
}
await_gone() {  # await_gone PIDS: wait for processes that are not this shell's children
  local c alive
  for _ in $(seq 1 3000); do
    alive=0; for c in $1; do kill -0 "$c" 2>/dev/null && alive=1; done
    [ "$alive" -eq 0 ] && return 0
    sleep 0.01
  done
  return 1
}

# --- a stop signal: nothing at the requested name, nothing left ------------------
for sig in TERM INT HUP; do
  D="$W/signal-$sig/out.zip"; mkdir -p "$(dirname "$D")"
  printf 'a neighbour\n' > "$(dirname "$D")/neighbour.txt"
  if paused "SIG$sig" "$D"; then
    kill -"$sig" "$PID" 2>/dev/null
    resume
    wait "$PID" 2>/dev/null; RC=$?
    assert_eq 2 "$RC" "SIG$sig: exit"
    assert_eq extraction_stopped "$(field reason.code "$W/run.out")" "SIG$sig: reason"
    assert_eq refused "$(field status "$W/run.out")" "SIG$sig: status"
    grep -q "stopped by a signal before the archive was finished. No archive was made" "$W/run.err" \
      && pass "SIG$sig: says so" || fail "SIG$sig: $(cat "$W/run.err")"
    [ ! -e "$D" ] && pass "SIG$sig: nothing at the requested name" || fail "SIG$sig: something at the requested name"
    [ ! -e "$STAGE" ] && pass "SIG$sig: the unfinished file is gone" || fail "SIG$sig: the unfinished file remains"
    assert_eq "neighbour.txt" "$(ls -A "$(dirname "$D")")" "SIG$sig: only the neighbour is left"
    await_gone "$CHILDREN" && pass "SIG$sig: the writer was stopped" || fail "SIG$sig: the writer is still running"
  fi
done

# --- a signal that was already ignored stays ignored, so nohup still works --------
D="$W/nohup/out.zip"; mkdir -p "$(dirname "$D")"
perl -e '$SIG{HUP}="IGNORE"; exec @ARGV or exit 127' "$PACK" "$S" --for other --into "$D" >/dev/null 2>"$W/run.err" </dev/null &
PID=$!; PIDS+=("$PID")
sleep 0.05
kill -HUP "$PID" 2>/dev/null
wait "$PID" 2>/dev/null; RC=$?
assert_eq 0 "$RC" "nohup: a hang-up does not stop the run"
[ -f "$D" ] && pass "nohup: the archive is made" || fail "nohup: no archive"

# --- the requested name never holds a partial archive ----------------------------
D="$W/watched/out.zip"; mkdir -p "$(dirname "$D")"
if paused "mid-write" "$D"; then
  [ ! -e "$D" ] && pass "mid-write: nothing at the requested name" || fail "mid-write: a partial archive at the requested name"
  assert_eq 600 "$(stat -f %Lp "$STAGE")" "mid-write: the unfinished file is private"
  resume; wait "$PID" 2>/dev/null; RC=$?
  assert_eq 0 "$RC" "mid-write: finishes"
  /usr/bin/tar -tf "$D" >/dev/null 2>&1 && pass "mid-write: the archive is complete" || fail "mid-write: the archive does not read"
  xattr "$D" 2>/dev/null | grep -q "$MARK" && fail "mid-write: the finished archive still has the mark" || pass "mid-write: the finished archive has no mark"
fi

# --- a file appears at the requested name mid-run: it is never replaced ----------
D="$W/raced/out.zip"; mkdir -p "$(dirname "$D")"
if paused "a racing file" "$D"; then
  printf 'the other writer\n' > "$D"
  resume; wait "$PID" 2>/dev/null; RC=$?
  assert_eq 2 "$RC" "a racing file: exit"
  assert_eq destination_exists "$(field reason.code "$W/run.out")" "a racing file: reason"
  assert_eq "the other writer" "$(cat "$D")" "a racing file: not replaced"
  [ ! -e "$STAGE" ] && pass "a racing file: the unfinished file is gone" || fail "a racing file: the unfinished file remains"
fi

# --- the folder changes during the run --------------------------------------------
D="$W/changed/out.zip"; mkdir -p "$(dirname "$D")"
if paused "a changed file" "$D"; then
  printf 'more\n' >> "$S/chunk8.bin"
  resume; wait "$PID" 2>/dev/null; RC=$?
  assert_eq 2 "$RC" "a changed file: exit"
  assert_eq unsafe_input "$(field reason.code "$W/run.out")" "a changed file: reason"
  grep -q "changed while pack was reading it. No archive was made" "$W/run.err" && pass "a changed file: says so" || fail "a changed file: $(cat "$W/run.err")"
  [ ! -e "$D" ] && pass "a changed file: no archive" || fail "a changed file: an archive was made"
  [ ! -e "$STAGE" ] && pass "a changed file: the unfinished file is gone" || fail "a changed file: the unfinished file remains"
fi

# --- the unfinished file is swapped mid-run: the replacement is not ours ----------
D="$W/swapped/out.zip"; mkdir -p "$(dirname "$D")"
if paused "a swapped file" "$D"; then
  mv "$STAGE" "$W/swapped/moved-away"
  printf 'belongs to someone else\n' > "$STAGE"
  resume; wait "$PID" 2>/dev/null; RC=$?
  [ "$RC" -ne 0 ] && pass "a swapped file: not reported as done" || fail "a swapped file: reported done"
  [ ! -e "$D" ] && pass "a swapped file: no archive" || fail "a swapped file: an archive was made"
  assert_eq "belongs to someone else" "$(cat "$STAGE" 2>/dev/null)" "a swapped file: the replacement is untouched"
  grep -q "left it alone" "$W/run.err" && pass "a swapped file: says it left it" || fail "a swapped file: $(cat "$W/run.err")"
  grep -q "panicked" "$W/run.err" && fail "a swapped file: panic" || pass "a swapped file: no panic"
fi

# --- SIGKILL, and the decoys a later run must not touch --------------------------
K="$W/killed"; mkdir -p "$K"; D="$K/out.zip"
if paused "SIGKILL" "$D"; then
  kill -9 "$PID" 2>/dev/null; for c in $CHILDREN; do kill -CONT "$c" 2>/dev/null || true; done
  wait "$PID" 2>/dev/null
  await_gone "$CHILDREN" || fail "SIGKILL: the orphaned writer did not finish"
  LEFT="$STAGE"
  [ -e "$LEFT" ] && pass "SIGKILL: the unfinished file is still there" || fail "SIGKILL: nothing to test"
  [ ! -e "$D" ] && pass "SIGKILL: nothing at the requested name" || fail "SIGKILL: something at the requested name"

  # Decoys: each has a name a leftover could have, and each fails one test.
  mark() { xattr -w "$MARK" "$2" "$1"; }
  name() { printf '%s/.out.zip.pack-%s.partial' "$K" "$1"; }
  H1=$(hex); UNMARKED=$(name "$H1"); printf 'mine\n' > "$UNMARKED"; chmod 600 "$UNMARKED"
  H2=$(hex); WRONG=$(name "$H2"); printf 'mine\n' > "$WRONG"; chmod 600 "$WRONG"; mark "$WRONG" "$(hex)"
  H3=$(hex); LINK=$(name "$H3"); printf 'link target\n' > "$K/target.txt"; ln -s "$K/target.txt" "$LINK"
  H4=$(hex); FOLDER=$(name "$H4"); mkdir "$FOLDER"; printf 'inside\n' > "$FOLDER/kept"
  H5=$(hex); LINKED=$(name "$H5"); printf 'mine\n' > "$LINKED"; chmod 600 "$LINKED"; mark "$LINKED" "$H5"; ln "$LINKED" "$K/second-name"
  H6=$(hex); OPENED=$(name "$H6"); printf 'mine\n' > "$OPENED"; chmod 644 "$OPENED"; mark "$OPENED" "$H6"
  H7=$(hex); FIFO=$(name "$H7"); mkfifo "$FIFO"
  H8=$(hex); OTHER="$K/.other.zip.pack-$H8.partial"; printf 'theirs\n' > "$OTHER"; chmod 600 "$OTHER"; mark "$OTHER" "$H8"
  SHORT="$K/.out.zip.pack-abc.partial"; printf 'mine\n' > "$SHORT"; chmod 600 "$SHORT"; mark "$SHORT" abc
  UPPER="$K/.out.zip.pack-$(hex | tr a-f A-F).partial"; printf 'mine\n' > "$UPPER"; chmod 600 "$UPPER"
  SIMILAR="$K/out.zip.bak"; printf 'a person file\n' > "$SIMILAR"
  snapshot() { ls -ldi "$UNMARKED" "$WRONG" "$LINK" "$FOLDER" "$LINKED" "$OPENED" "$FIFO" "$OTHER" "$SHORT" "$UPPER" "$SIMILAR" "$K/target.txt" "$K/second-name" "$FOLDER/kept"; }
  BEFORE=$(snapshot)

  # --list writes nothing, so it removes nothing either.
  "$PACK" "$S" --for other --into "$D" --list >/dev/null 2>&1 </dev/null
  [ -e "$LEFT" ] && pass "--list leaves the leftover alone" || fail "--list removed something"

  # A refusal before writing also removes nothing.
  "$PACK" "$S" --for other --into "$D" --format rar >/dev/null 2>&1 </dev/null
  [ -e "$LEFT" ] && pass "a bad argument leaves the leftover alone" || fail "a bad argument removed something"

  "$PACK" "$S" --for other --into "$D" --json >"$W/retry.out" 2>"$W/retry.err" </dev/null; RC=$?
  assert_eq 0 "$RC" "a later run: exit"
  [ ! -e "$LEFT" ] && pass "a later run: the leftover is gone" || fail "a later run: the leftover remains"
  [ -f "$D" ] && pass "a later run: the archive is made" || fail "a later run: no archive"
  assert_eq 1 "$(field details.leftovers_removed "$W/retry.out")" "a later run: reports one removed"
  assert_eq 7 "$(field details.leftovers_kept "$W/retry.out")" "a later run: reports the seven with the exact name that it would not prove"
  grep -q "removed 1 unfinished file from an earlier run" "$W/retry.err" && pass "a later run: says what it removed" || fail "a later run: $(cat "$W/retry.err")"
  grep -q "pack cannot prove they are its own" "$W/retry.err" && pass "a later run: says what it left" || fail "a later run: $(cat "$W/retry.err")"
  assert_eq "$BEFORE" "$(snapshot)" "a later run: every decoy is exactly as it was"
  assert_eq "link target" "$(cat "$K/target.txt")" "a later run: the link's target is untouched"
  assert_eq "inside" "$(cat "$FOLDER/kept")" "a later run: the folder's content is untouched"
  assert_eq "a person file" "$(cat "$SIMILAR")" "a later run: a similar name is untouched"
fi

# --- a leftover that another live run is writing is not removed --------------------
L="$W/live"; mkdir -p "$L"; D="$L/out.zip"
if paused "a live run" "$D"; then
  FIRST="$STAGE"; FIRST_PID="$PID"; FIRST_CHILDREN="$CHILDREN"; FIRST_OUT="$W/first.out"
  cp "$W/run.out" "$FIRST_OUT" 2>/dev/null
  "$PACK" "$W/small" --for other --into "$D" --json >"$W/second.out" 2>"$W/second.err" </dev/null; RC=$?
  assert_eq 0 "$RC" "a live run: the second run finishes"
  assert_eq 0 "$(field details.leftovers_removed "$W/second.out")" "a live run: the second run removed nothing"
  assert_eq 0 "$(field details.leftovers_kept "$W/second.out")" "a live run: a busy file is not called a leftover"
  assert_eq 1 "$(field details.leftovers_still_open "$W/second.out")" "a live run: the open file is counted"
  grep -q "because another process still has it open" "$W/second.err" && pass "a live run: the second run says it left an open file" || fail "a live run: $(cat "$W/second.err")"
  [ -e "$FIRST" ] && pass "a live run: its unfinished file is still there" || fail "a live run: its unfinished file was removed"
  CHILDREN="$FIRST_CHILDREN"; PID="$FIRST_PID"; resume
  wait "$FIRST_PID" 2>/dev/null; RC=$?
  assert_eq 2 "$RC" "a live run: the first run loses the name, and says so"
  assert_eq destination_exists "$(field reason.code "$W/run.out")" "a live run: reason"
  [ ! -e "$FIRST" ] && pass "a live run: the first run removed its own file" || fail "a live run: the first run left its file"
  /usr/bin/tar -tf "$D" | grep -q '^small/a$' && pass "a live run: the winner's archive is intact" || fail "a live run: the winner's archive was damaged"
fi

# --- an orphaned writer still holds its file: a later run leaves it ----------------
O="$W/orphan"; mkdir -p "$O"; D="$O/out.zip"
if paused "an orphaned writer" "$D"; then
  kill -9 "$PID" 2>/dev/null; wait "$PID" 2>/dev/null
  ORPHAN_STAGE="$STAGE"
  "$PACK" "$W/small" --for other --into "$D" --json >"$W/o.out" 2>"$W/o.err" </dev/null
  assert_eq 0 "$(field details.leftovers_removed "$W/o.out")" "an orphaned writer: a file it still holds is not removed"
  assert_eq 1 "$(field details.leftovers_still_open "$W/o.out")" "an orphaned writer: the open file is counted"
  grep -q "the tar of a pack that was killed" "$W/o.err" && pass "an orphaned writer: says so" || fail "an orphaned writer: $(cat "$W/o.err")"
  # The destination now exists. A refused run still looks, and removes nothing locked.
  "$PACK" "$W/small" --for other --into "$D" --json >"$W/o2.out" 2>"$W/o2.err" </dev/null; RC=$?
  assert_eq 2 "$RC" "an orphaned writer, destination exists: refused"
  grep -q "because another process still has it open" "$W/o2.err" && pass "an orphaned writer, destination exists: reports the open file" || fail "an orphaned writer, destination exists: $(cat "$W/o2.err")"
  [ -e "$ORPHAN_STAGE" ] && pass "an orphaned writer, destination exists: the open file stays" || fail "an orphaned writer, destination exists: the open file was removed"
  [ -e "$ORPHAN_STAGE" ] && pass "an orphaned writer: the file is still there" || fail "an orphaned writer: the file was removed under it"
  for c in $CHILDREN; do kill -9 "$c" 2>/dev/null; done
  await_gone "$CHILDREN" || fail "an orphaned writer: it did not stop"
  # --list reports it and removes nothing.
  OUT=$("$PACK" "$W/small" --for other --into "$D" --list 2>&1 </dev/null)
  case "$OUT" in *"can be removed"*) pass "--list: reports what a run would remove" ;; *) fail "--list: $OUT" ;; esac
  [ -e "$ORPHAN_STAGE" ] && pass "--list: the leftover stays" || fail "--list: the leftover was removed"
  # The destination still exists: the run refuses, and the leftover is dealt with.
  BEFORE_DEST=$(ls -li "$D")
  "$PACK" "$W/small" --for other --into "$D" --json >"$W/o3.out" 2>"$W/o3.err" </dev/null; RC=$?
  assert_eq 2 "$RC" "destination exists: refused"
  assert_eq destination_exists "$(field reason.code "$W/o3.out")" "destination exists: reason"
  grep -q "removed 1 unfinished file from an earlier run" "$W/o3.err" && pass "destination exists: says what it removed" || fail "destination exists: $(cat "$W/o3.err")"
  [ ! -e "$ORPHAN_STAGE" ] && pass "destination exists: the leftover is gone" || fail "destination exists: the leftover remains"
  assert_eq "$BEFORE_DEST" "$(ls -li "$D")" "destination exists: the archive is untouched"
fi

# --- two runs at once: one wins the name, nothing is left --------------------------
C="$W/concurrent"; mkdir -p "$C"
"$PACK" "$W/small" --for other --into "$C/out.zip" >/dev/null 2>"$W/c1.err" </dev/null & P1=$!
"$PACK" "$W/small" --for other --into "$C/out.zip" >/dev/null 2>"$W/c2.err" </dev/null & P2=$!
wait "$P1"; R1=$?; wait "$P2"; R2=$?
assert_eq "0 2" "$(printf '%s\n%s\n' "$R1" "$R2" | sort -n | tr '\n' ' ' | sed 's/ $//')" "two runs: one made it, one refused"
assert_eq "out.zip" "$(ls -A "$C")" "two runs: only the archive is left"
