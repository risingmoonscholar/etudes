#!/usr/bin/env bash
# The safety holds now run in one place, before any rule that picks a folder or
# a tag. A folder plan and a tag plan of the same folder hold the same files
# for the same reasons, and applying the folder plan moves none of them.
#
# A tag moves nothing, so a file already inside a folder sweep made is tagged
# like any other, and a tag plan calls nothing in place (issue #221).
#
# etude-core is shared by all three tools: stash still clears and restores the
# same folder byte for byte, and unpack still extracts a synthetic archive to
# its exact bytes. Everything is synthetic, in a temp dir, and removed on exit.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
exec </dev/null

W=$(workdir) || exit 1; trap 'rm -rf "${W:?}"; eval "$_stress_state_cleanup"' EXIT
require python3 "these checks read JSON with Python" || exit 0
OLD=202001010101

json_get() {
  python3 - "$1" "$2" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))["details"]
print(eval(sys.argv[2], {"d": d}))
PY
}

# --- one folder, each hold once, plus files that sort -------------------------
D="$W/desk"; mkdir -p "$D"
for name in mix.flp kick.wav tax_return_2023.pdf movie.mp4.part a.pdf b.pdf c.pdf \
  "Screenshot 2026-01-01 at 10.00.00.png" "Screenshot 2026-01-01 at 10.00.01.png" \
  "Screenshot 2026-01-01 at 10.00.02.png"; do
  printf 'synthetic %s\n' "$name" > "$D/$name"
done
touch -t "$OLD" "$D"/*
HELD="mix.flp kick.wav tax_return_2023.pdf movie.mp4.part"

"$SWEEP" "$D" --json >"$W/folders.json" 2>/dev/null; CODE=$?
assert_eq 0 "$CODE" "a folder plan exits 0"
"$SWEEP" "$D" --scheme tags --map pdf=Read --map wav=Samples --json >"$W/tags.json" 2>/dev/null; CODE=$?
assert_eq 0 "$CODE" "a tag plan exits 0"

for plan in folders tags; do
  for key in looks_personal project_documents held_near_document in_flight; do
    assert_eq 1 "$(json_get "$W/$plan.json" "d['left_alone']['$key']")" "the $plan plan holds 1 file as $key"
  done
done
assert_eq "Screenshots:3 Documents:3" \
  "$(json_get "$W/folders.json" "' '.join(f\"{g['name']}:{g['count']}\" for g in d['groups'])")" \
  "the folder plan sorts the 3 screenshots and the 3 ordinary pdfs"
assert_eq "Read:3" \
  "$(json_get "$W/tags.json" "' '.join(f\"{g['name']}:{g['count']}\" for g in d['groups'])")" \
  "the tag plan tags the 3 ordinary pdfs only: the tax pdf and the wav beside the .flp are held"

"$SWEEP" apply "$D" --yes >"$W/apply.out" 2>&1; CODE=$?
assert_eq 0 "$CODE" "the folder plan applies (exit $CODE)"
for name in $HELD; do
  [ "$(cat "$D/$name" 2>/dev/null)" = "synthetic $name" ] \
    && pass "$name stayed where it was, unchanged" || fail "$name moved or changed"
done
[ -f "$D/Documents/a.pdf" ] && [ -f "$D/Screenshots/Screenshot 2026-01-01 at 10.00.00.png" ] \
  && pass "the ordinary files moved into their folders" || fail "the ordinary files did not move"
"$SWEEP" undo "$D" >"$W/undo.out" 2>&1; CODE=$?
assert_eq 0 "$CODE" "undo exits 0"
[ -f "$D/a.pdf" ] && [ ! -e "$D/Documents" ] && [ ! -e "$D/Screenshots" ] \
  && pass "undo put every file back and removed the folders it made" || fail "undo left the folder changed"

# --- a tag plan and a built-in folder that is already here --------------------
E="$W/shots"; mkdir -p "$E/Screenshots"
for n in 0 1 2; do
  printf 'loose %s\n' "$n" > "$E/Screenshot 2026-01-01 at 10.00.0$n.png"
  printf 'filed %s\n' "$n" > "$E/Screenshots/Screenshot 2026-01-01 at 11.00.0$n.png"
  printf 'other %s\n' "$n" > "$E/picture$n.png"
done
touch -t "$OLD" "$E"/*.png "$E"/Screenshots/*.png
"$SWEEP" "$E" --scheme tags --map png=Work --depth 2 --json >"$W/shots.json" 2>/dev/null; CODE=$?
assert_eq 0 "$CODE" "a depth-2 tag plan exits 0"
assert_eq 9 "$(json_get "$W/shots.json" "sum(g['count'] for g in d['groups'])")" \
  "all 9 pngs get the tag, the 3 inside Screenshots/ included"
assert_eq 0 "$(json_get "$W/shots.json" "d['left_alone']['in_place']")" \
  "a tag plan reports no file in place (issue #221)"

# --- stash and unpack, which share etude-core ---------------------------------
# stash moves files sweep holds as personal or in flight: clearing a folder for
# a screen share is pointless if the tax scan stays on the Desktop.
CODE=0; "$STASH" "$D" >/dev/null 2>&1 || CODE=$?
assert_eq 0 "$CODE" "stash runs on the same folder (exit $CODE)"
[ ! -e "$D/tax_return_2023.pdf" ] && [ ! -e "$D/movie.mp4.part" ] && [ ! -e "$D/a.pdf" ] \
  && pass "stash took the personal file, the download and the ordinary files" \
  || fail "stash left a file sweep holds, or an ordinary file, in the open folder"
CODE=0; "$STASH" pop "$D" >/dev/null 2>&1 || CODE=$?
assert_eq 0 "$CODE" "pop restores them (exit $CODE)"
[ "$(cat "$D/tax_return_2023.pdf" 2>/dev/null)" = "synthetic tax_return_2023.pdf" ] \
  && [ "$(cat "$D/mix.flp" 2>/dev/null)" = "synthetic mix.flp" ] \
  && pass "the files came back with their bytes" || fail "a file did not come back intact"

SRC="$W/source"; mkdir -p "$SRC/sub"
printf 'synthetic archive member\n' > "$SRC/member.txt"
printf 'synthetic nested member\n' > "$SRC/sub/nested.txt"
tar -czf "$W/fixture.tar.gz" -C "$SRC" member.txt sub
CODE=0; "$UNPACK" "$W/fixture.tar.gz" --into "$W/out" >/dev/null 2>&1 || CODE=$?
assert_eq 0 "$CODE" "unpack extracts the synthetic archive (exit $CODE)"
[ "$(cat "$W/out/member.txt" 2>/dev/null)" = 'synthetic archive member' ] \
  && [ "$(cat "$W/out/sub/nested.txt" 2>/dev/null)" = 'synthetic nested member' ] \
  && pass "unpack wrote both members with their exact bytes" \
  || fail "unpack did not write the expected members"
true
