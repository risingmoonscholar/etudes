#!/usr/bin/env bash
# unpack reads a private copy of the archive. SIGKILL runs no cleanup, so the
# copy must have no name on disk once extraction starts. A private TMPDIR holds
# the whole witness: the kill comes while staging exists and before publication.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
require tar "the fixture needs tar" || exit 0
require zip "the fixture needs zip" || exit 0
require pgrep "stopping the surviving extractor needs pgrep" || exit 0

W=$(workdir); trap 'rm -rf "$W"' EXIT
python3 - "$W/payload" <<'PYTHON'
from pathlib import Path
import sys
root = Path(sys.argv[1])
root.mkdir()
for n in range(8000):
    (root / f"{n:05}.txt").write_bytes(b"synthetic member\n")
PYTHON
(cd "$W" && zip -q -r -0 input.zip payload)
tar -cf "$W/input.tar" -C "$W" payload
tar -czf "$W/input.tar.gz" -C "$W" payload
rm -rf "$W/payload"

stranded() { find "$1" -mindepth 1 2>/dev/null | wc -l | tr -d ' '; }

# The detector must see a stranded copy, or a clean result proves nothing.
mkdir -p "$W/control/unpack-00000000000000000000000000000000"
printf 'synthetic copy\n' > "$W/control/unpack-00000000000000000000000000000000/input.zip"
[ "$(stranded "$W/control")" -gt 0 ] \
  && pass "control: the detector reports a planted copy" \
  || fail "control: the detector missed a planted copy"

for name in input.zip input.tar input.tar.gz; do
  ARCHIVE="$W/$name"; TEMP="$W/tmp-$name"; OUT="$W/out-$name"; DEST="$OUT/dest"
  mkdir -p "$TEMP" "$OUT"
  before=$(shasum -a 256 "$ARCHIVE")
  TMPDIR="$TEMP" "$UNPACK" "$ARCHIVE" --into "$DEST" --max-size 4G >/dev/null 2>&1 &
  PID=$!

  STARTED=0
  for _ in $(seq 6000); do
    # Staging and its ownership lock exist before spawn. A fixture member,
    # rather than the directory alone, witnesses a running extractor.
    if [ -n "$(find "$OUT" -type f -name '*.txt' -print -quit 2>/dev/null)" ]; then STARTED=1; break; fi
    kill -0 "$PID" 2>/dev/null || break
    sleep 0.01
  done
  # Freeze the supervisor after real payload exists so it cannot spawn a
  # writer between enumerating its children and killing it.
  kill -STOP "$PID" 2>/dev/null
  KIDS=$(pgrep -P "$PID" || true)
  kill -KILL "$PID" 2>/dev/null
  CODE=0; wait "$PID" 2>/dev/null || CODE=$?
  # The system extractor survives the kill. Stop it so the folder can go.
  for kid in $KIDS; do kill -KILL "$kid" 2>/dev/null; done

  # kill returns before descriptor release. Do not race rm against a writer
  # still exiting; the new inherited lock is a direct shutdown witness.
  if python3 - "$OUT" <<'PYTHON'
import fcntl
from pathlib import Path
import sys
import time
locks = list(Path(sys.argv[1]).glob(".*.partial/.unpack-*.lock"))
assert len(locks) == 1, "the killed staging must have exactly one ownership lock"
with locks[0].open("rb") as lock:
    deadline = time.monotonic() + 10
    while True:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            break
        except BlockingIOError:
            if time.monotonic() >= deadline:
                raise AssertionError("the extractor retained its lock after SIGKILL")
            time.sleep(0.01)
PYTHON
  then pass "$name: extractor released its lock before fixture teardown"
  else fail "$name: extractor shutdown was not verified"; fi

  assert_eq 1 "$STARTED" "$name: the kill came during extraction"
  assert_eq 137 "$CODE" "$name: SIGKILL ended unpack"
  [ ! -e "$DEST" ] && pass "$name: the kill came before publication" \
    || fail "$name: the destination was already published"
  assert_eq 0 "$(stranded "$TEMP")" "$name: nothing is left in the temporary folder"
  assert_eq "$before" "$(shasum -a 256 "$ARCHIVE")" "$name: the source archive is unchanged"
  rm -rf "$TEMP" "$OUT"
done
