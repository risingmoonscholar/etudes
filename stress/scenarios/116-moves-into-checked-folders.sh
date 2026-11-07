#!/usr/bin/env bash
# etude-core now makes each move into the folder it opened and checked, not
# into whatever the path names at the moment of the move. All three tools go
# through that move, so all three run here end to end.
#
# sweep files (an extension it has no group for, so --map may route it) into an existing folder it may write into but not list (mode
# 0300), then undoes: both must work, since the folder is opened for search
# only. A destination that is a link to a folder outside the root is refused
# and nothing lands outside. stash holds and pops a folder, and unpack
# extracts a synthetic archive, both to the exact bytes. Everything is
# synthetic, in a temp dir, and removed on exit.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
exec </dev/null

if [ "$(uname -s)" != Darwin ]; then
  unproven "moves into the opened folder" "the descriptor-relative move is macOS only"
  exit 0
fi

W=$(workdir); trap 'chmod -R u+rwx "$W" 2>/dev/null; rm -rf "$W"; eval "$_stress_state_cleanup"' EXIT

# sweep into a write-only folder, then undo.
D="$W/root"; mkdir -p "$D/Docs"
for n in 1 2 3; do printf 'synthetic report %s\n' "$n" > "$D/r$n.rpx"; done
chmod 0300 "$D/Docs"
CODE=0; OUT=$("$SWEEP" apply "$D" --since 0 --map rpx=Docs --yes 2>&1) || CODE=$?
chmod 0700 "$D/Docs"
assert_no_panic "sweep apply into a write-only folder" "$OUT"
assert_eq 0 "$CODE" "sweep files into a folder it may write but not list (exit $CODE)"
[ "$(cat "$D/Docs/r1.rpx" 2>/dev/null)" = 'synthetic report 1' ] \
  && pass "r1.rpx is in Docs with its bytes" || fail "r1.rpx is not in Docs"
CODE=0; OUT=$("$SWEEP" undo "$D" 2>&1) || CODE=$?
assert_no_panic "sweep undo" "$OUT"
assert_eq 0 "$CODE" "sweep undo puts the files back (exit $CODE)"
[ "$(cat "$D/r2.rpx" 2>/dev/null)" = 'synthetic report 2' ] \
  && pass "r2.rpx is home with its bytes" || fail "r2.rpx is not home"

# A destination that links outside the root is refused; nothing lands outside.
mkdir -p "$W/outside"; ln -s "$W/outside" "$D/Linked"
CODE=0; OUT=$("$SWEEP" apply "$D" --since 0 --map rpx=Linked --yes 2>&1) || CODE=$?
assert_no_panic "sweep apply toward a link" "$OUT"
[ "$CODE" -ne 0 ] && pass "a destination linking outside the root is refused (exit $CODE)" \
  || fail "sweep moved through a link to outside the root"
LEAKED=$(find "$W/outside" -type f | wc -l | tr -d ' ')
assert_eq 0 "$LEAKED" "no file landed outside the root"
assert_eq 3 "$(find "$D" -maxdepth 1 -name 'r*.rpx' | wc -l | tr -d ' ')" "all three files are still home"
rm "$D/Linked"

# stash holds and pops the folder.
CODE=0; OUT=$("$STASH" "$D" 2>&1) || CODE=$?
assert_no_panic "stash" "$OUT"
assert_eq 0 "$CODE" "stash clears the folder (exit $CODE)"
CODE=0; OUT=$("$STASH" pop "$D" 2>&1) || CODE=$?
assert_no_panic "stash pop" "$OUT"
assert_eq 0 "$CODE" "pop restores it (exit $CODE)"
[ "$(cat "$D/r1.rpx" 2>/dev/null)" = 'synthetic report 1' ] \
  && pass "r1.rpx came back with its bytes" || fail "r1.rpx did not come back"

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
