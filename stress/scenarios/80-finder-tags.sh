#!/usr/bin/env bash
# Finder tags: an agent drives the promise end to end on a folder it made.
#
# A tagged file is the user having already filed something. By default sweep
# leaves it alone and says how many it left, never which. With
# --include-tagged, loudly, it organises the item and the tag survives the
# move. undo brings it back. Everything here is synthetic and lives in a
# temp dir; nothing personal is touched. Each step is the real binary.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir); trap 'rm -rf "$W"' EXIT
F="$W/folder"; mkdir -p "$F"
TAG='<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><array><string>Work
6</string></array></plist>'
for i in 1 2 3 4 5 6; do printf 'synthetic report %s\n' "$i" > "$F/report-$i.pdf"; done
printf 'synthetic invoice text\n' > "$F/filed.pdf"
xattr -w com.apple.metadata:_kMDItemUserTags "$TAG" "$F/filed.pdf"

# --- the plan reports a count, never a name --------------------------------
MAP=(--since 0)
PLAN=$("$SWEEP" "$F" --json "${MAP[@]}" 2>&1) || true
grep -q '"tagged": *1' <<<"$PLAN" \
  && pass "the plan counts one tagged item held back" \
  || fail "the plan did not report one held tagged item: $(head -c 300 <<<"$PLAN")"
grep -q 'filed.pdf' <<<"$PLAN" \
  && fail "the plan named the tagged file; a count was promised" \
  || pass "and does not name it"

# --- apply leaves the tagged item where it is -------------------------------
"$SWEEP" apply "$F" --yes "${MAP[@]}" >/dev/null 2>&1 || true
[ -f "$F/filed.pdf" ] && pass "apply left the tagged file in place" || fail "apply moved a tagged file without being told to"
[ ! -f "$F/report-1.pdf" ] && pass "and organised the untagged ones" || fail "the untagged files were not organised"

# --- --include-tagged moves it, loudly, and the tag survives -----------------
# A fresh folder: a lone tagged file cannot form a group, and that is sweep's
# grouping rule, not the tag rule. The tag rule is tested with the file among
# its peers.
G="$W/again"; mkdir -p "$G"
for i in 1 2 3 4 5 6; do printf 'synthetic report %s\n' "$i" > "$G/report-$i.pdf"; done
printf 'synthetic invoice text\n' > "$G/filed.pdf"
xattr -w com.apple.metadata:_kMDItemUserTags "$TAG" "$G/filed.pdf"
OUT=$("$SWEEP" apply "$G" --yes "${MAP[@]}" --include-tagged 2>&1) || true
grep -q "WARNING: including Finder-tagged" <<<"$OUT" && pass "--include-tagged is loud" || fail "--include-tagged was silent: $OUT"
[ ! -f "$G/filed.pdf" ] && pass "and organised the tagged file with its group" || fail "--include-tagged left the tagged file: $OUT"
MOVED=$(find "$G" -mindepth 2 -name filed.pdf -type f | head -1)
[ -n "$MOVED" ] && pass "it exists inside a group folder" || fail "the tagged file vanished or never moved"
if [ -n "$MOVED" ]; then
  xattr -p com.apple.metadata:_kMDItemUserTags "$MOVED" 2>/dev/null | grep -q "Work" \
    && pass "its Finder tag survived the move" \
    || fail "the Finder tag was lost on move"
fi

# --- undo brings it back, tag intact -----------------------------------------
"$SWEEP" undo "$G" >/dev/null 2>&1 || true
[ -f "$G/filed.pdf" ] && pass "undo restored the tagged file" || fail "undo did not restore the tagged file"
xattr -p com.apple.metadata:_kMDItemUserTags "$G/filed.pdf" 2>/dev/null | grep -q "Work" \
  && pass "with its tag" || fail "undo lost the tag"
