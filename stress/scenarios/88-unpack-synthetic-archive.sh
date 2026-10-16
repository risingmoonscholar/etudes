#!/usr/bin/env bash
# etude-core is shared by all three tools. After a core change, unpack must
# still extract an archive made here to its exact bytes.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir); trap 'rm -rf "$W"' EXIT
SRC="$W/source"; OUT="$W/out"; ARCHIVE="$W/fixture.tar.gz"
mkdir -p "$SRC/sub"
printf 'synthetic archive member\n' > "$SRC/member.txt"
printf 'synthetic nested member\n' > "$SRC/sub/nested.txt"
tar -czf "$ARCHIVE" -C "$SRC" member.txt sub

CODE=0; TEXT=$("$UNPACK" "$ARCHIVE" --into "$OUT" 2>&1) || CODE=$?
printf '%s\n' "$TEXT"
assert_eq 0 "$CODE" "unpack extracts the synthetic archive (exit $CODE)"
[ "$(cat "$OUT/member.txt" 2>/dev/null)" = 'synthetic archive member' ] \
  && [ "$(cat "$OUT/sub/nested.txt" 2>/dev/null)" = 'synthetic nested member' ] \
  && pass "unpack wrote both synthetic members with their exact bytes" \
  || fail "unpack did not write the expected members"
[ -f "$ARCHIVE" ] && pass "and kept the source archive" || fail "the source archive is gone"
