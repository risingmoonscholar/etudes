#!/usr/bin/env bash
# Race: a file is still being appended to, by a process holding it open,
# while sweep scans it, fingerprints it, and moves it.
#
# What must hold: the same-device move (atomic rename on macOS, or the
# hard-link fallback) operates on directory entries, never file content, so a concurrent
# writer's open file descriptor keeps writing to the same inode no matter
# which name currently points at it or which directory it lives in. The file
# must come out the other side with every byte the writer actually wrote
# (nothing dropped, nothing truncated), regardless of when sweep's move lands
# relative to the writes.
#
# Nothing here is real data. Every tree is generated and removed on exit.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

require python3 "builds fixture trees, runs the writer, and verifies content" || exit 0

N=500

build_filler() {
  mkdir -p "$1"
  python3 - "$1" "$N" <<'PY'
import sys, os
d, n = sys.argv[1], int(sys.argv[2])
def letters(i, width=5):
    s = ""
    for _ in range(width):
        s = chr(97 + i % 26) + s
        i //= 26
    return s
for i in range(n):
    open(os.path.join(d, f"batch_{letters(i)}.csv"), "w").close()
PY
}

CONTROL="${1:-}"
case "$CONTROL" in ''|--control-late|--control-ignored) ;; *) fail "unknown mid-apply control"; exit 0;; esac
DUMP="$(dirname "$SWEEP")/journal-dump"
if [ ! -x "$DUMP" ]; then
  unproven "mid-apply injection" "dev-only journal watcher unavailable"
  exit 0
fi
export ETUDE_JOURNAL_KEY="$(python3 -c 'import secrets; print(secrets.token_hex(32))')"
W=$(workdir)
PID=""
D=""
cleanup() {
  if [ -n "${WRITER_PID:-}" ]; then
    touch "${RELEASE:-$W/release-writer}"
    wait "$WRITER_PID" 2>/dev/null || true
  fi
  if [ -n "$PID" ]; then
    kill -CONT "$PID" 2>/dev/null || true
    kill -TERM "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true
  fi
  rm -rf "$W"
  eval "$_stress_state_cleanup"
}
trap cleanup EXIT

# Each control runs the complete witness with its own assertion channel.
# Late apply must report an unproven injection; the unsafe-outcome control
# must make the unchanged assertions reject its simulated faulty result.
run_controls() {
  local control result output
  for control in late ignored; do
    output="$W/control-$control.out"
    env -u STRESS_WRAP_DEPTH -u STRESS_WRAPPER_PID -u STRESS_RESULT_FD \
      -u STRESS_FAILURE_ARTIFACTS /bin/bash "${BASH_SOURCE[0]}" "--control-$control" \
      >"$output" 2>&1 198<&- 199>&-
    result=$?
    if [ "$control" = late ]; then
      if [ "$result" -eq 2 ] && ! grep -q "FAIL" "$output" && ! grep -q "injection proven" "$output" && grep -q 'unproven.*injection' "$output"; then
        pass "late injection control is unproven, never a pass or a product failure"
      else
        fail "late injection control wanted unproven exit 2, got $result: $(cat "$output")"
      fi
    elif [ "$result" -eq 1 ] && grep -q 'confirmed stopped' "$output" && grep -q 'FAIL' "$output"; then
      pass "paused unsafe-outcome control fails retained assertions after a proven injection"
    else
      fail "paused unsafe-outcome control did not reject the fault (exit $result): $(cat "$output")"
    fi
  done
}

D="$W/Desktop"
export ETUDE_JOURNAL_KEY="$(python3 -c 'import secrets; print(secrets.token_hex(32))')"
export ETUDE_STATE_DIR="$W/state"
mkdir -p "$ETUDE_STATE_DIR"
build_filler "$D"

TARGET="$D/batch_growing.csv"
: > "$TARGET"
# Match the canonical source paths stored in the accepted apply journal, even
# when TMPDIR reaches the fixture through the macOS /var alias.
TARGET=$(python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$TARGET")
RESULT="$W/writer-result.txt"
READY="$W/writer-ready"
RELEASE="$W/release-writer"
START="$W/start-writer"
APPENDED="$W/writer-appended"

# Hold an acknowledged open descriptor without writing until the journal
# watcher proves apply is stopped and this source remains pending. Then append
# while stopped and keep writing across the resumed move; the deadline bounds
# a broken witness rather than deciding when injection happened.
python3 - "$TARGET" "$RESULT" "$READY" "$RELEASE" "$START" "$APPENDED" <<'PY' &
import sys, time, hashlib
path, result_path, ready_path, release_path, start_path, appended_path = sys.argv[1:]
deadline = time.monotonic() + 30.0
f = open(path, "ab", buffering=0)
h = hashlib.sha256()
total = 0
i = 0
open(ready_path, "w").write("open\n")
while not __import__("os").path.exists(start_path) and not __import__("os").path.exists(release_path) and time.monotonic() < deadline:
    time.sleep(0.003)
while not __import__("os").path.exists(release_path) and time.monotonic() < deadline:
    chunk = (f"CHUNK-{i:08d}-".encode() * 64)[:1024]
    f.write(chunk)
    h.update(chunk)
    total += len(chunk)
    open(appended_path, "w").write("appended\n")
    i += 1
    time.sleep(0.003)
f.close()
with open(result_path, "w") as r:
    r.write(f"{total}\t{h.hexdigest()}\n")
PY
WRITER_PID=$!

# Wait for the writer to acknowledge its open descriptor. A fixed sleep can
# pass before the child even opens the file on a loaded runner.
for _ in $(seq 1 200); do [ -f "$READY" ] && break; sleep 0.01; done
[ -f "$READY" ] || { fail "growing file writer never acknowledged its open descriptor"; exit 0; }
"$SWEEP" apply "$D" --yes >"$W/apply.out" 2>"$W/apply.err" &
PID=$!
if [ "$CONTROL" = --control-late ]; then wait "$PID" 2>/dev/null || true; fi
if ! WITNESS=$("$DUMP" --watch-apply "$ETUDE_STATE_DIR" "$D" "$TARGET" "$PID" 2>"$W/watch.err"); then
  kill -CONT "$PID" 2>/dev/null || true
  wait "$PID" 2>/dev/null || true
  PID=""
  unproven "growing file injection" "no confirmed stopped apply with pending writer target: $(cat "$W/watch.err")"
  exit 0
fi
read -r JOURNAL_MOVED JOURNAL_TOTAL PENDING_INDEX <<< "$WITNESS"
STOPPED=$(ps -o state= -p "$PID" 2>/dev/null)
if [[ "$STOPPED" != *T* ]] || [ ! -f "$TARGET" ] || [ -e "$D/Data/batch_growing.csv" ]; then
  unproven "growing file injection" "target no longer pending while stopped"
  exit 0
fi
touch "$START"
for _ in $(seq 1 200); do [ -f "$APPENDED" ] && break; sleep 0.01; done
STOPPED=$(ps -o state= -p "$PID" 2>/dev/null)
if [ ! -f "$APPENDED" ] || ! kill -0 "$WRITER_PID" 2>/dev/null || [[ "$STOPPED" != *T* ]] || [ ! -s "$TARGET" ] || [ -e "$D/Data/batch_growing.csv" ]; then
  unproven "growing file injection" "writer did not append while apply was stopped"
  exit 0
fi
pass "growing file injection proven: writer appended to pending source while apply was confirmed stopped ($JOURNAL_MOVED/$JOURNAL_TOTAL journalled moves)"
kill -CONT "$PID" || { fail "could not resume proven writer injection"; exit 0; }
wait "$PID"
APPLY_CODE=$?
PID=""

FOUND="$(find "$D" -name "batch_growing.csv" 2>/dev/null | head -1)"
if [ -n "$FOUND" ]; then
  FOUND=$(python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$FOUND")
fi
if [ "$APPLY_CODE" = 0 ] && [ "$FOUND" != "$TARGET" ] && kill -0 "$WRITER_PID" 2>/dev/null; then
  pass "the growing file moved while its acknowledged writer was still running"
else
  fail "apply did not move the growing file while its writer was alive (exit=$APPLY_CODE path=${FOUND:-missing})"
fi
touch "$RELEASE"
wait "$WRITER_PID"
WRITER_PID=""
if [ "$CONTROL" = --control-ignored ] && [ -n "$FOUND" ]; then : > "$FOUND"; fi

if [ ! -f "$RESULT" ]; then
  unproven "growing file survives a concurrent move" "the writer never finished. Could not establish an expected byte count"
  exit 0
fi
EXPECT_SIZE="$(cut -f1 "$RESULT")"
EXPECT_HASH="$(cut -f2 "$RESULT")"

assert_eq 0 "$APPLY_CODE" "apply succeeds while the writer holds the file open"

if [ -z "$FOUND" ]; then
  fail "the growing file cannot be found anywhere in the tree after apply. It was lost"
  exit 0
fi

ACTUAL_SIZE=$(stat -f '%z' "$FOUND")
ACTUAL_HASH=$(shasum -a 256 "$FOUND" | awk '{print $1}')

assert_eq "$EXPECT_SIZE" "$ACTUAL_SIZE" "final file size matches every byte the writer actually wrote (no truncation)"
assert_eq "$EXPECT_HASH" "$ACTUAL_HASH" "final file content is byte-identical to what the writer wrote (no corruption, no interleaving damage)"

# The file must actually have MOVED. Accepting "still at the original path"
# made this a clean green over "nothing to apply": the fixture used .dat,
# which is in no type family, so no group formed, apply exited 1, and the
# size and hash below were measured on a file nothing had touched. A race
# scenario that passes when the race never started proves nothing.
if [ "$FOUND" = "$TARGET" ]; then
  fail "the growing file never moved (still at $TARGET). Either no group formed or apply did not reach it -- either way the mid-move race this scenario exists for did not happen, and the size and hash below would be measuring a stationary file"
else
  pass "the growing file was moved to $FOUND while $((EXPECT_SIZE)) bytes were being written"
fi

[ -n "$CONTROL" ] || run_controls
true
