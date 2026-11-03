#!/usr/bin/env bash
# macOS tar writes each member under a temporary name and renames it into place.
# A size check that walks the staging folder sees names vanish while it runs.
# A vanished name is not an unreadable entry: a large tar must extract every time.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
require tar "the fixture needs tar" || exit 0

W=$(workdir); trap 'rm -rf "$W"' EXIT
SRC="$W/payload"; ARCHIVE="$W/input.tar"; COUNT=5000; RUNS=5
python3 - "$SRC" "$COUNT" <<'PYTHON'
from pathlib import Path
import sys
root = Path(sys.argv[1])
root.mkdir()
for n in range(int(sys.argv[2])):
    (root / f"{n:05}.txt").write_bytes(b"synthetic member\n")
PYTHON
tar -cf "$ARCHIVE" -C "$W" payload
before=$(shasum -a 256 "$ARCHIVE")

for run in $(seq "$RUNS"); do
  OUT="$W/out-$run"
  CODE=0; TEXT=$("$UNPACK" "$ARCHIVE" --into "$OUT" 2>&1) || CODE=$?
  [ "$CODE" -eq 0 ] || printf '%s\n' "$TEXT"
  assert_eq 0 "$CODE" "run $run: unpack extracts a $COUNT-file tar (exit $CODE)"
  got=$(find "$OUT" -type f 2>/dev/null | wc -l | tr -d ' ')
  assert_eq "$COUNT" "$got" "run $run: all $COUNT members are in the destination"
  rm -rf "$OUT"
done
assert_eq "$before" "$(shasum -a 256 "$ARCHIVE")" "the source archive is unchanged"
