#!/usr/bin/env bash
# A core change is exercised through unpack on an archive made in this scenario.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir); trap 'rm -rf "$W"' EXIT
SRC="$W/source"; OUT="$W/out"; ARCHIVE="$W/fixture.tar.gz"
mkdir -p "$SRC"
printf 'synthetic archive member\n' > "$SRC/member.txt"
tar -czf "$ARCHIVE" -C "$SRC" member.txt

CODE=0; TEXT=$("$UNPACK" "$ARCHIVE" --into "$OUT" 2>&1) || CODE=$?
printf '%s\n' "$TEXT"
assert_eq 0 "$CODE" "unpack extracts the synthetic archive (exit $CODE)"
[ "$(cat "$OUT/member.txt" 2>/dev/null)" = 'synthetic archive member' ] \
  && pass "unpack wrote the expected synthetic member" \
  || fail "unpack did not write the expected member"
