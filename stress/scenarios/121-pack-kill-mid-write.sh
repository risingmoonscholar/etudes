#!/usr/bin/env bash
# The archive writer owns only a private staging file until the source has been
# rechecked and the result is published exclusively. Two interruptions:
#
#   1. Stop the writer while the staging file is part-written, change one
#      included source file while nothing can run, continue. The identity
#      recheck must refuse (exit 2, unsafe_input) and publish nothing; the
#      source keeps the bytes that were written to it.
#   2. SIGKILL the supervisor at a witnessed partial state. The orphaned system
#      archiver may finish, but only inside staging: the destination stays
#      absent, and a retry publishes an archive equal to the source.
#
# Timing probes: the process is stopped and `ps` must report state T before any
# injection; a window that was missed is reported unproven and retried at most
# four times. A writer that finished before the window is a control reading,
# never a pass.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir); trap 'rm -rf "$W"' EXIT
export ETUDE_STATE_DIR="$W/state"; mkdir -p "$ETUDE_STATE_DIR"
export COPYFILE_DISABLE=1
SOURCE="$W/source"; TOTAL=80
mkdir -p "$SOURCE"
for i in $(seq 1 "$TOTAL"); do
  dd if=/dev/urandom of="$SOURCE/chunk_$i.bin" bs=1048576 count=1 status=none
done

manifest() { (cd "$1" && find . -type f -exec shasum {} \; | LC_ALL=C sort); }
partial_of() { find "$W" -maxdepth 1 -name ".$1.pack-*.partial" -print -quit; }
size_of() { stat -f %z "$1" 2>/dev/null || echo 0; }

listing=$("$PACK" "$SOURCE" --for other --into "$W/probe.zip" --list --json </dev/null 2>/dev/null)
BOUND=$(printf '%s' "$listing" | python3 -c 'import json,sys; print(json.load(sys.stdin)["details"]["upper_bound_bytes"])' 2>/dev/null)
case "$BOUND" in ''|*[!0-9]*) fail "could not read the archive bound from --list"; exit 0;; esac
pass "preflight bound is $BOUND bytes for $TOTAL MiB of source"

# wait_for_window PID NAME: the staging file exists, is part-written and the
# supervisor is still alive. Prints the staging path and size.
wait_for_window() {
  local pid="$1" name="$2" stage size
  for _ in $(seq 1 4000); do
    stage=$(partial_of "$name")
    if [ -n "$stage" ]; then
      size=$(size_of "$stage")
      if [ "$size" -gt 0 ] && [ "$size" -lt "$BOUND" ] && kill -0 "$pid" 2>/dev/null; then
        printf '%s\t%s\n' "$stage" "$size"; return 0
      fi
    fi
    kill -0 "$pid" 2>/dev/null || return 1
    sleep 0.002
  done
  return 1
}

# Phase 1: stop, inject, continue.
VICTIM="$SOURCE/chunk_40.bin"
PHASE1_ATTEMPTS=0; PHASE1_DONE=0
for attempt in 1 2 3 4; do
  PHASE1_ATTEMPTS=$attempt
  DEST="$W/stopped-$attempt.zip"
  "$PACK" "$SOURCE" --for other --into "$DEST" --json >"$W/stopped-$attempt.out" 2>"$W/stopped-$attempt.err" </dev/null &
  PID=$!
  if ! witness=$(wait_for_window "$PID" "stopped-$attempt.zip"); then
    wait "$PID" 2>/dev/null
    echo "    (attempt $attempt: the writer finished before a partial staging file was witnessed; control reading, not a result)"
    rm -f "$DEST"
    continue
  fi
  STAGE=${witness%%$'\t'*}; SIZE=${witness##*$'\t'}
  CHILDREN=$(pgrep -P "$PID" 2>/dev/null || true)
  kill -STOP "$PID" 2>/dev/null
  for c in $CHILDREN; do kill -STOP "$c" 2>/dev/null || true; done
  STATE=""
  for _ in $(seq 1 200); do
    STATE=$(ps -o state= -p "$PID" 2>/dev/null)
    [[ "$STATE" == *T* ]] && break
    [ -z "$STATE" ] && break
    sleep 0.001
  done
  if [[ "$STATE" != *T* ]]; then
    echo "    (attempt $attempt: the writer did not reach a stopped state; continuing it without injection)"
    for c in $CHILDREN; do kill -CONT "$c" 2>/dev/null || true; done
    kill -CONT "$PID" 2>/dev/null; wait "$PID" 2>/dev/null
    rm -f "$DEST"
    continue
  fi
  pass "attempt $attempt: writer stopped at $SIZE of at most $BOUND staging bytes (ps state $STATE)"
  # Inject while nothing can run: append bytes to one included source file.
  printf 'bytes appended while the writer was stopped\n' >> "$VICTIM"
  INJECTED=$(shasum "$VICTIM" | cut -d' ' -f1)
  for c in $CHILDREN; do kill -CONT "$c" 2>/dev/null || true; done
  kill -CONT "$PID" 2>/dev/null
  wait "$PID" 2>/dev/null; STATUS=$?
  assert_eq 2 "$STATUS" "source changed while stopped: pack refused"
  CODE=$(python3 -c 'import json,sys; r=json.load(open(sys.argv[1]))["reason"]; print(r["code"] if r else "null")' "$W/stopped-$attempt.out" 2>/dev/null)
  assert_eq unsafe_input "$CODE" "source changed while stopped: reason.code"
  [ ! -e "$DEST" ] && pass "source changed while stopped: no destination published" \
    || fail "source changed while stopped: a destination was published"
  [ ! -e "$STAGE" ] && pass "source changed while stopped: staging file removed on refusal" \
    || fail "source changed while stopped: staging file remains"
  assert_eq "$INJECTED" "$(shasum "$VICTIM" | cut -d' ' -f1)" "source keeps exactly the injected bytes"
  PHASE1_DONE=1
  break
done
if [ "$PHASE1_DONE" -eq 0 ]; then
  unproven "stop-inject-continue window" "the writer finished before a partial staging file was witnessed in $PHASE1_ATTEMPTS attempts"
fi
SOURCE_MANIFEST=$(manifest "$SOURCE")

# Phase 2: SIGKILL at a witnessed partial state, then retry.
PHASE2_ATTEMPTS=0; PHASE2_DONE=0
for attempt in 1 2 3 4; do
  PHASE2_ATTEMPTS=$attempt
  DEST="$W/killed-$attempt.zip"
  "$PACK" "$SOURCE" --for other --into "$DEST" --json >"$W/killed-$attempt.out" 2>"$W/killed-$attempt.err" </dev/null &
  PID=$!
  if ! witness=$(wait_for_window "$PID" "killed-$attempt.zip"); then
    wait "$PID" 2>/dev/null
    echo "    (attempt $attempt: the writer finished before a partial staging file was witnessed; control reading, not a result)"
    rm -f "$DEST"
    continue
  fi
  STAGE=${witness%%$'\t'*}; SIZE=${witness##*$'\t'}
  CHILDREN=$(pgrep -P "$PID" 2>/dev/null || true)
  kill -9 "$PID" 2>/dev/null && pass "attempt $attempt: SIGKILL delivered at $SIZE staging bytes" \
    || { fail "could not deliver SIGKILL"; wait "$PID" 2>/dev/null; continue; }
  wait "$PID" 2>/dev/null; STATUS=$?
  [ "$STATUS" -ne 0 ] && pass "killed pack exited non-zero ($STATUS)" || fail "killed pack exited zero"
  [ ! -e "$DEST" ] && pass "killed pack published nothing" || fail "killed pack published a destination"
  settled=0
  for _ in $(seq 1 3000); do
    alive=0
    for c in $CHILDREN; do kill -0 "$c" 2>/dev/null && alive=1; done
    [ "$alive" -eq 0 ] && { settled=1; break; }
    sleep 0.01
  done
  [ "$settled" -eq 1 ] && pass "the orphaned archiver settled" || fail "the orphaned archiver did not settle within 30 seconds"
  [ ! -e "$DEST" ] && pass "destination remains absent after the orphan settled" \
    || fail "the orphan reached the destination"
  if [ -e "$STAGE" ]; then
    mode=$(stat -f %Lp "$STAGE")
    assert_eq 600 "$mode" "orphaned staging file stays private (mode $mode) and dot-prefixed; nothing cleans it after SIGKILL"
  else
    pass "no staging file remained after the orphan settled"
  fi
  assert_exit 0 "retry publishes after the killed run" -- "$PACK" "$SOURCE" --for other --into "$DEST" --json
  mkdir -p "$W/extract-$attempt"
  /usr/bin/tar -xf "$DEST" -C "$W/extract-$attempt" 2>/dev/null
  assert_eq "$SOURCE_MANIFEST" "$(manifest "$W/extract-$attempt")" "retry archive holds every source path and payload exactly"
  PHASE2_DONE=1
  break
done
if [ "$PHASE2_DONE" -eq 0 ]; then
  unproven "kill-mid-write window" "the writer finished before a partial staging file was witnessed in $PHASE2_ATTEMPTS attempts"
fi
echo "    (window attempts: stop-inject $PHASE1_ATTEMPTS, kill $PHASE2_ATTEMPTS)"
