#!/usr/bin/env bash
# Killing the unpack supervisor must not publish a partial tree at the user's
# requested destination. The system extractor may outlive that kill, but it
# writes only to a private staging directory; retrying the requested target
# must therefore remain safe and exact.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir); trap 'rm -rf "$W"' EXIT
SOURCE="$W/source"; ARCHIVE="$W/archive.zip"; OUT="$W/out"; TOTAL=80
mkdir -p "$SOURCE"
for i in $(seq 1 "$TOTAL"); do
  dd if=/dev/urandom of="$SOURCE/chunk_$i.bin" bs=1048576 count=1 status=none
done
(cd "$SOURCE" && zip -0 -q "$ARCHIVE" ./*)

manifest() { (cd "$1" && find . -type f -exec shasum {} \; | LC_ALL=C sort); }
SOURCE_MANIFEST=$(manifest "$SOURCE")
wait_for_partial_staging() {
  local pid="$1" stage n
  for _ in $(seq 1 1000); do
    stage=$(find "$W" -type d -name '.out.unpack-*.partial' -print -quit)
    n=0
    [ -n "$stage" ] && n=$(find "$stage" -type f | wc -l | tr -d ' ')
    if [ "$n" -gt 0 ] && [ "$n" -lt "$TOTAL" ] && kill -0 "$pid" 2>/dev/null; then
      printf '%s\t%s\n' "$stage" "$n"; return 0
    fi
    kill -0 "$pid" 2>/dev/null || return 1
    sleep 0.01
  done
  return 1
}
wait_for_staging_completion() {
  local stage="$1" n
  for _ in $(seq 1 1000); do
    n=$(find "$stage" -type f 2>/dev/null | wc -l | tr -d ' ')
    [ "$n" -eq "$TOTAL" ] && return 0
    sleep 0.01
  done
  return 1
}

"$UNPACK" "$ARCHIVE" --into "$OUT" >"$W/first.out" 2>&1 & PID=$!
if witness=$(wait_for_partial_staging "$PID"); then
  STAGE=${witness%%$'\t'*}; COUNT=${witness##*$'\t'}
  pass "interrupted extraction witnessed $COUNT of $TOTAL files in private staging while unpack remained live"
  kill -9 "$PID" 2>/dev/null && pass "SIGKILL reached the unpack supervisor" || fail "could not deliver SIGKILL to unpack"
  wait "$PID" 2>/dev/null; STATUS=$?
  [ "$STATUS" -ne 0 ] && pass "killed unpack exited non-zero ($STATUS)" || fail "killed unpack exited zero"
else
  wait "$PID" 2>/dev/null || true
  fail "unpack did not reach a witnessed partial staging state within 10 seconds"
  exit 0
fi

[ ! -e "$OUT" ] && pass "killed unpack never published a partial requested destination" \
  || fail "killed unpack published a partial requested destination"
if wait_for_staging_completion "$STAGE"; then
  pass "the orphaned extractor completed only inside private staging"
else
  fail "private staging did not settle within 10 seconds after supervisor death"
fi
[ ! -e "$OUT" ] && pass "requested destination remains absent after orphan staging settles" \
  || fail "orphan staging appeared at the requested destination"

assert_exit 0 "retry extracts after interrupted supervisor" -- "$UNPACK" "$ARCHIVE" --into "$OUT"
assert_eq "$SOURCE_MANIFEST" "$(manifest "$OUT")" "retry restored every archived path and payload exactly"
[ "$(find "$OUT" -type f | wc -l | tr -d ' ')" -eq "$TOTAL" ] \
  && pass "retry published exactly the expected file count" \
  || fail "retry published the wrong file count"
