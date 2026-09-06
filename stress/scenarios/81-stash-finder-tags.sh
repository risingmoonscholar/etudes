#!/usr/bin/env bash
# Stash clears tagged items too: a screen-share clear report must mean clear.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir); trap 'rm -rf "$W"' EXIT
F="$W/folder"; mkdir -p "$F"
printf 'synthetic tagged report\n' > "$F/tagged.pdf"
xattr -w com.apple.metadata:_kMDItemUserTags 'Work\n6' "$F/tagged.pdf"

CODE=0; OUT=$("$STASH" "$F" 2>&1) || CODE=$?
if [ "$CODE" = 2 ] && grep -q 'could not store the key' <<<"$OUT"; then
  unproven "stash can clear a tagged item" "the host keychain refused the journal key"
elif [ "$CODE" != 0 ]; then
  fail "stash refused the synthetic tagged folder (exit $CODE): $OUT"
else
  [ ! -e "$F/tagged.pdf" ] \
    && pass "stash moved the tagged item" \
    || fail "stash left the tagged item behind"
  find "$F" -mindepth 1 -maxdepth 1 ! -name '.stash-*' -print -quit | grep -q . \
    && fail "stash reported clear while a visible item remained" \
    || pass "the folder contains no visible items"
  grep -Fq "$F is clear." <<<"$OUT" \
    && pass "stash reported the folder clear only after moving the tagged item" \
    || fail "stash did not report the cleared folder: $OUT"
fi
