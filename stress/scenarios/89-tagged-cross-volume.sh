#!/usr/bin/env bash
# A Finder tag does not survive a cross-volume copy: the copy requests data and
# POSIX stat only. So a tagged item whose move would cross volumes is left in
# place, with its tag, and counted by name, in sweep and in stash. Untagged
# items on the same run still cross. Everything is synthetic, on a disk image
# this scenario makes and detaches.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

require hdiutil "cross-volume tag scenario needs a real disk image" || exit 0

cleanup() {
  detach_registered_mounts
  [ -n "${W:-}" ] && rm -rf "$W"
}
trap cleanup EXIT

TAGATTR=com.apple.metadata:_kMDItemUserTags
W=$(workdir)
OUTER="$W/outer"
IMG="$W/inner.dmg"
INNER="$OUTER/aaa_innervol"
mkdir -p "$OUTER" "$INNER"
WHAT="cross-volume: a tagged item stays put and is counted by name"

if ! create_disk_image "$IMG" 256m APFS TagVol; then
  unproven "$WHAT" "image creation failed on this host"; exit 0
fi
if ! hdiutil attach "$IMG" -mountpoint "$INNER" -nobrowse >/dev/null 2>&1; then
  unproven "$WHAT" "hdiutil attach failed on this host"; exit 0
fi
register_mount "$INNER"
if [ "$(stat -f %d "$OUTER")" = "$(stat -f %d "$INNER")" ]; then
  unproven "$WHAT" "the nested mount shares a device id with the outer tree"; exit 0
fi
pass "the outer tree and the nested mount are on different devices"
if ! xattr -w "$TAGATTR" probe "$INNER" 2>/dev/null; then
  unproven "$WHAT" "the nested volume cannot hold extended attributes"; exit 0
fi

# --- sweep -------------------------------------------------------------------
for f in a b c; do printf 'outer-%s\n' "$f" > "$OUTER/note_$f.txt"; done
for f in d e; do printf 'inner-%s\n' "$f" > "$INNER/note_$f.txt"; done
printf 'inner-tagged\n' > "$INNER/note_tagged.txt"
xattr -w "$TAGATTR" opaque-cross "$INNER/note_tagged.txt"

PLAN=$("$SWEEP" "$OUTER" --depth 2 --include-tagged --json 2>/dev/null) || true
grep -q '"tagged_skipped":0' <<<"$PLAN" \
  && pass "with --include-tagged the plan holds nothing for being tagged" \
  || fail "unexpected plan: $(head -c 300 <<<"$PLAN")"

OUT=$("$SWEEP" apply "$OUTER" --depth 2 --yes --include-tagged 2>&1); CODE=$?
printf '%s\n' "$OUT"
assert_eq 0 "$CODE" "sweep exits 0 when a tagged item stays behind"
grep -q '1 Finder-tagged item left in place: moving to another volume would drop the tag' <<<"$OUT" \
  && pass "sweep counted the tagged item by its named reason" \
  || fail "sweep did not name the cross-volume hold: $OUT"
[ -f "$INNER/note_tagged.txt" ] && [ "$(xattr -p "$TAGATTR" "$INNER/note_tagged.txt" 2>/dev/null)" = opaque-cross ] \
  && pass "the tagged file is still on its own volume with its tag" \
  || fail "the tagged file moved or lost its tag"
[ ! -f "$INNER/note_d.txt" ] && [ -n "$(find "$OUTER" -mindepth 2 -maxdepth 2 -name note_d.txt -not -path "$INNER/*" | head -1)" ] \
  && pass "an untagged file on the other volume still crossed" \
  || fail "the untagged file on the other volume did not cross"
"$SWEEP" undo "$OUTER" >/dev/null 2>&1 || true
[ -f "$INNER/note_d.txt" ] && [ -f "$INNER/note_tagged.txt" ] && [ -f "$OUTER/note_a.txt" ] \
  && pass "undo put every file back" || fail "undo left files behind"

# --- stash select ------------------------------------------------------------
# The holding folder goes beside the first object, on the inner volume; the
# second object is on the outer volume, so its move crosses.
S1="$INNER/first.txt"; S2="$OUTER/second_tagged.txt"; S3="$OUTER/third_plain.txt"
printf 'first\n' > "$S1"; printf 'second\n' > "$S2"; printf 'third\n' > "$S3"
xattr -w "$TAGATTR" opaque-second "$S2"
OUT=$("$STASH" select "$S1" "$S2" "$S3" 2>&1); CODE=$?
printf '%s\n' "$OUT"
assert_eq 3 "$CODE" "stash select finishes incomplete when a tagged object stays behind"
grep -q '1 Finder-tagged item was left in the folder: moving to another volume would drop the tag' <<<"$OUT" \
  && pass "stash counted the tagged object by its named reason" \
  || fail "stash did not name the cross-volume hold: $OUT"
grep -q 'Not every selected object was stashed' <<<"$OUT" \
  && pass "and said so instead of calling the selection done" || fail "stash select did not say a selected object was left: $OUT"
[ -f "$S2" ] && [ "$(xattr -p "$TAGATTR" "$S2" 2>/dev/null)" = opaque-second ] \
  && pass "the tagged object is still in place with its tag" || fail "the tagged object moved or lost its tag"
[ ! -e "$S3" ] && pass "the untagged object on the same volume was stashed" || fail "the untagged object was not stashed"
