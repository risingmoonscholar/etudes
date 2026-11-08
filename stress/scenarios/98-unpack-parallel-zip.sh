#!/usr/bin/env bash
# unzip calls stat on the path it is given, and on macOS stat on /dev/fd/N can
# fail while another process uses the same N. One run must not refuse a safe
# zip because other runs are extracting at the same time.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
require zip "the fixture needs zip" || exit 0

W=$(workdir) || exit 1; trap 'rm -rf "$W"' EXIT
RUNS=1000; WORKERS=4
mkdir -p "$W/src/nested" "$W/out"
printf 'nested' > "$W/src/nested/ spaced  name.txt"
printf 'plain' > "$W/src/plain.txt"
(cd "$W/src" && zip -r -q "$W/input.zip" nested plain.txt)

cat > "$W/one.sh" <<'WORKER'
#!/bin/bash
target="$3/$4"
if ! "$1" "$2" --into "$target" >/dev/null 2>"$target.err"; then
  head -1 "$target.err" >> "$3/refused"
elif [ "$(cat "$target/plain.txt")" != plain ]; then
  echo "wrong bytes" >> "$3/refused"
fi
rm -rf "$target" "$target.err"
WORKER
chmod +x "$W/one.sh"

seq 1 "$RUNS" | xargs -P "$WORKERS" -I{} "$W/one.sh" "$UNPACK" "$W/input.zip" "$W/out" {}
REFUSED=0
[ -f "$W/out/refused" ] && REFUSED=$(wc -l < "$W/out/refused" | tr -d ' ')
assert_eq 0 "$REFUSED" "$RUNS parallel runs extract the same safe zip with none refused"
[ "$REFUSED" -eq 0 ] || fail "first refusal: $(head -1 "$W/out/refused")"
