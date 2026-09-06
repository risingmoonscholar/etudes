#!/usr/bin/env bash
# A core change is exercised through unpack on an archive made in this scenario.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir); trap 'rm -rf "$W"' EXIT
SRC="$W/source"; OUT="$W/out"; ARCHIVE="$W/fixture.tar.gz"
mkdir -p "$SRC"
printf 'synthetic archive member\n' > "$SRC/member.txt"
tar -czf "$ARCHIVE" -C "$SRC" member.txt

assert_exit 0 "unpack extracts the synthetic archive" -- "$UNPACK" "$ARCHIVE" --into "$OUT"
[ "$(cat "$OUT/member.txt" 2>/dev/null)" = 'synthetic archive member' ] \
  && pass "unpack wrote the expected synthetic member" \
  || fail "unpack did not write the expected member"
