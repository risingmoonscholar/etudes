#!/usr/bin/env bash
# stash clears tagged items too. sweep holds tagged items by default; if stash
# inherited that it would leave them in the folder and still call it clear.
# An agent builds a folder with a tagged file and a tagged folder, runs stash,
# and checks that the folder is clear because it is. Everything is synthetic.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir); trap 'rm -rf "$W"' EXIT
TAGATTR=com.apple.metadata:_kMDItemUserTags
F="$W/folder"; mkdir -p "$F/tagged-folder"
printf 'synthetic plain file\n' > "$F/plain.txt"
printf 'synthetic tagged report\n' > "$F/tagged.pdf"
printf 'synthetic nested file\n' > "$F/tagged-folder/inside.txt"
xattr -w "$TAGATTR" 'opaque-red' "$F/tagged.pdf"
xattr -w "$TAGATTR" 'opaque-blue' "$F/tagged-folder"

CODE=0; OUT=$("$STASH" "$F" 2>&1) || CODE=$?
printf '%s\n' "$OUT"
assert_eq 0 "$CODE" "stash exits 0 on a folder holding tagged items"
VISIBLE=$(find "$F" -mindepth 1 -maxdepth 1 ! -name '.stash-*' | wc -l | tr -d ' ')
assert_eq 0 "$VISIBLE" "no visible item is left in the folder"
grep -q 'The selected folder is clear\.' <<<"$OUT" \
  && pass "stash reported the folder clear, and it is" \
  || fail "stash did not report the folder clear: $OUT"
grep -q 'NOT clear' <<<"$OUT" && fail "stash called a cleared folder not clear" || pass "and did not hedge"

HOLD=$(find "$F" -mindepth 1 -maxdepth 1 -name '.stash-*' | head -1)
if [ -n "$HOLD" ] && [ "$(xattr -p "$TAGATTR" "$HOLD/tagged.pdf" 2>/dev/null)" = 'opaque-red' ] \
  && [ "$(xattr -p "$TAGATTR" "$HOLD/tagged-folder" 2>/dev/null)" = 'opaque-blue' ]; then
  pass "tagged file and tagged folder are in the holding folder with their tags"
else
  fail "the holding folder is missing a tagged item or its tag"
fi

JSON=$("$STASH" "$F" --json 2>&1) || true
grep -q '"status":"nothing_to_do"' <<<"$JSON" \
  && pass "a second stash of the cleared folder has nothing to do" \
  || fail "second stash was not nothing_to_do: $(head -c 200 <<<"$JSON")"

"$STASH" pop "$F" >/dev/null 2>&1 || true
[ "$(xattr -p "$TAGATTR" "$F/tagged.pdf" 2>/dev/null)" = 'opaque-red' ] \
  && [ -f "$F/tagged-folder/inside.txt" ] && [ -f "$F/plain.txt" ] \
  && pass "pop restored everything with the tags intact" \
  || fail "pop did not restore the tagged items intact"

G="$W/json-folder"; mkdir -p "$G"
printf 'synthetic\n' > "$G/tagged.txt"; xattr -w "$TAGATTR" 'opaque-green' "$G/tagged.txt"
JSON=$("$STASH" "$G" --json 2>&1) || true
grep -q '"clear":true' <<<"$JSON" && grep -q '"held_cross_volume":0' <<<"$JSON" \
  && pass "the JSON result says clear and counts nothing held" \
  || fail "JSON did not report a clear folder: $(head -c 300 <<<"$JSON")"
grep -q 'opaque-green' <<<"$JSON" && fail "the tag text reached the JSON" || pass "and carries no tag text"
