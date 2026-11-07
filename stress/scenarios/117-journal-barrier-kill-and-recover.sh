#!/usr/bin/env bash
# Journal records are now ordered behind their moves with a barrier, and a
# finished run is flushed once. All three tools share that journal path or sit
# beside it, so all three run here end to end.
#
# sweep apply is paused part way through 600 moves, confirmed stopped, then
# killed; sweep undo must bring every file home with its bytes, which needs an
# in-order record of the moves made. A finished stash and pop of 600 files
# must round-trip. unpack extracts a synthetic archive to its exact bytes.
# Everything is synthetic, in a temp dir, and removed on exit.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
exec </dev/null
require python3 "builds the fixture tree" || exit 0

W=$(workdir)
pid=""
cleanup() {
  if [ -n "$pid" ]; then
    kill -CONT "$pid" 2>/dev/null || true
    kill -KILL "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  fi
  rm -rf "$W"
  eval "$_stress_state_cleanup"
}
trap cleanup EXIT

make_tree() {
  python3 - "$1" "$2" <<'PY'
import pathlib, sys
d, n = pathlib.Path(sys.argv[1]), int(sys.argv[2])
d.mkdir(parents=True)
for i in range(1, n + 1):
    name = f"Screenshot 2026-0{i % 9 + 1}-{i % 28 + 1:02d} at {i % 12 + 1:02d}.{i % 60:02d}.{i % 60:02d} AM ({i:x}).png"
    (d / name).write_text(f"synthetic screenshot payload {i}\n")
PY
}
home_count() { find "$1" -maxdepth 1 -type f -name 'Screenshot*' | wc -l | tr -d ' '; }
bytes_ok() {
  python3 - "$1" <<'PY'
import pathlib, re, sys
bad = 0
for p in pathlib.Path(sys.argv[1]).glob("Screenshot*"):
    i = int(re.search(r"\(([0-9a-f]+)\)\.png$", p.name).group(1), 16)
    bad += p.read_text() != f"synthetic screenshot payload {i}\n"
print(bad)
PY
}

# A killed sweep apply, recovered by undo.
D="$W/killed"; make_tree "$D" 600
"$SWEEP" apply "$D" --yes >"$W/apply.out" 2>&1 &
pid=$!
paused=0
for _ in $(seq 1 2000); do
  if [ "$(find "$D/Screenshots" -type f 2>/dev/null | wc -l | tr -d ' ')" -ge 20 ]; then
    kill -STOP "$pid" 2>/dev/null || break
    case "$(ps -o state= -p "$pid" 2>/dev/null)" in T*) paused=1 ;; esac
    break
  fi
  kill -0 "$pid" 2>/dev/null || break
  sleep 0.005
done
if [ "$paused" = 1 ]; then
  MOVED=$(find "$D/Screenshots" -type f | wc -l | tr -d ' ')
  kill -KILL "$pid" 2>/dev/null; wait "$pid" 2>/dev/null; pid=""
  if [ "$MOVED" -lt 600 ]; then
    pass "sweep apply was stopped part way ($MOVED of 600 moved) and killed"
  else
    unproven "kill part way through sweep apply" "all 600 moves finished before the pause"
  fi
  OUT=$("$SWEEP" undo "$D" 2>&1); CODE=$?
  assert_no_panic "sweep undo after a killed apply" "$OUT"
  assert_eq 0 "$CODE" "sweep undo after a killed apply succeeds (exit $CODE)"
  assert_eq 600 "$(home_count "$D")" "all 600 files are home after undo"
  assert_eq 0 "$(bytes_ok "$D")" "every file has its original bytes"
  assert_eq 0 "$(find "$D/Screenshots" -type f 2>/dev/null | wc -l | tr -d ' ')" "nothing is left in Screenshots"
else
  wait "$pid" 2>/dev/null; pid=""
  unproven "kill part way through sweep apply" "the apply finished before it could be paused"
fi

# A finished stash and pop.
S="$W/stashed"; make_tree "$S" 600
OUT=$("$STASH" "$S" 2>&1); CODE=$?
assert_no_panic "stash" "$OUT"
assert_eq 0 "$CODE" "stash clears 600 files (exit $CODE)"
assert_eq 0 "$(home_count "$S")" "no file is left in the folder"
OUT=$("$STASH" pop "$S" 2>&1); CODE=$?
assert_no_panic "stash pop" "$OUT"
assert_eq 0 "$CODE" "pop restores them (exit $CODE)"
assert_eq 600 "$(home_count "$S")" "all 600 files came back"
assert_eq 0 "$(bytes_ok "$S")" "every file came back with its bytes"

# unpack extracts to the exact bytes.
SRC="$W/source"; mkdir -p "$SRC/sub"
printf 'synthetic archive member\n' > "$SRC/member.txt"
printf 'synthetic nested member\n' > "$SRC/sub/nested.txt"
tar -czf "$W/fixture.tar.gz" -C "$SRC" member.txt sub
OUT=$("$UNPACK" "$W/fixture.tar.gz" --into "$W/out" 2>&1); CODE=$?
assert_no_panic "unpack" "$OUT"
assert_eq 0 "$CODE" "unpack extracts the synthetic archive (exit $CODE)"
[ "$(cat "$W/out/member.txt" 2>/dev/null)" = 'synthetic archive member' ] \
  && [ "$(cat "$W/out/sub/nested.txt" 2>/dev/null)" = 'synthetic nested member' ] \
  && pass "unpack wrote both members with their exact bytes" \
  || fail "unpack did not write the exact bytes"
