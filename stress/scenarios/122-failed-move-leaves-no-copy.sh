#!/usr/bin/env bash
# A move that fails after it made its copy (across volumes) or its second name
# (link then unlink), and before the source was removed, now removes that copy
# or name. When it cannot, the failure names the copy, and sweep and stash no
# longer say "Nothing was moved". Every apply, tag apply and undo that wrote
# journal records now flushes them once on its error returns too.
#
# Honest scope: those failures cannot be produced from the command line
# without a fault hook, so they are proven in etude-core's own tests
# (tests/failed_move_leaves_no_copy.rs and the apply and journal unit tests).
# This scenario is the end-to-end check that ordinary runs of all three tools
# still round-trip with exact bytes after the change: sweep apply and undo,
# stash and pop, and unpack. Everything is synthetic, in a temp dir, and
# removed on exit.
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

# sweep apply and undo.
D="$W/swept"; make_tree "$D" 120
OUT=$("$SWEEP" apply "$D" --yes 2>&1); CODE=$?
assert_no_panic "sweep apply" "$OUT"
assert_eq 0 "$CODE" "sweep apply succeeds (exit $CODE)"
assert_eq 0 "$(home_count "$D")" "every file left the folder"
assert_eq 120 "$(find "$D/Screenshots" -type f 2>/dev/null | wc -l | tr -d ' ')" "all 120 files are in Screenshots, with no extra copy"
assert_eq 0 "$(bytes_ok "$D/Screenshots")" "every moved file has its bytes"
OUT=$("$SWEEP" undo "$D" 2>&1); CODE=$?
assert_no_panic "sweep undo" "$OUT"
assert_eq 0 "$CODE" "sweep undo succeeds (exit $CODE)"
assert_eq 120 "$(home_count "$D")" "all 120 files are home after undo"
assert_eq 0 "$(bytes_ok "$D")" "every file has its original bytes"
assert_eq 0 "$(find "$D/Screenshots" -type f 2>/dev/null | wc -l | tr -d ' ')" "nothing is left in Screenshots"

# stash and pop.
S="$W/stashed"; make_tree "$S" 120
OUT=$("$STASH" "$S" 2>&1); CODE=$?
assert_no_panic "stash" "$OUT"
assert_eq 0 "$CODE" "stash clears 120 files (exit $CODE)"
assert_eq 0 "$(home_count "$S")" "no file is left in the folder"
OUT=$("$STASH" pop "$S" 2>&1); CODE=$?
assert_no_panic "stash pop" "$OUT"
assert_eq 0 "$CODE" "pop restores them (exit $CODE)"
assert_eq 120 "$(home_count "$S")" "all 120 files came back"
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
