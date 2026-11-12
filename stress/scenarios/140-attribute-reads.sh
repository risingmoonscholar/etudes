#!/usr/bin/env bash
# Extended attributes are read and written through one place in etude-core.
# An agent drives every tool path that touches them, on folders it made, and
# checks what each one did and what it counted.
#
# sweep: a tagged file is held and counted, a Finder alias is skipped by its
# FinderInfo flag, a file whose FinderInfo has no alias flag is planned and
# moved with that FinderInfo intact, a symlink to a tagged file is a symlink,
# not a tagged file, and the observation receipt counts one tag-presence read
# per probed file with no failure. --scheme tags reads each file's tags once,
# adds a tag after the ones already there, and undo puts the old tags back
# and removes the attribute from a file that had none. stash and pop keep a
# tag's bytes. unpack extracts a synthetic archive to its exact bytes.
# Everything is synthetic, in a temp dir, and removed on exit.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
exec </dev/null

if [ "$(uname -s)" != Darwin ]; then
  unproven "attribute reads and writes" "Finder tags and FinderInfo are macOS features"
  exit 0
fi
require python3 "writes and decodes attribute values independently of the tool" || exit 0

W=$(workdir) || exit 1; trap 'rm -rf "$W"; eval "$_stress_state_cleanup"' EXIT
export TAGATTR=com.apple.metadata:_kMDItemUserTags
INFOATTR=com.apple.FinderInfo

tags_of() {
  python3 - "$1" <<'PY'
import json, os, plistlib, subprocess, sys
shown = subprocess.run(["xattr", "-px", os.environ["TAGATTR"], sys.argv[1]], capture_output=True, text=True)
print("none" if shown.returncode else json.dumps(
    plistlib.loads(bytes.fromhex(shown.stdout.replace("\n", "").replace(" ", ""))), ensure_ascii=False))
PY
}
set_tags() {
  python3 - "$@" <<'PY'
import os, plistlib, subprocess, sys
path, *tags = sys.argv[1:]
subprocess.run(["xattr", "-wx", os.environ["TAGATTR"], plistlib.dumps(tags, fmt=plistlib.FMT_BINARY).hex(), path], check=True)
PY
}
json_get() {
  python3 - "$1" "$2" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
print(eval(sys.argv[2], {"d": d}))
PY
}
# observed FILE CATEGORY: "attempted/observed/failed" from the receipt, or none
observed() {
  json_get "$1" "next(('%d/%d/%d' % (c['attempted'], c['observed'], c['failed']) for c in d['observations']['categories'] if c['category'] == '$2'), 'none')"
}
finder_info() {  # finder_info FILE ALIAS(0|1): a 32-byte FinderInfo
  local hex
  hex=$(python3 -c "import sys; b = bytearray(32); b[8] = 0x80 * int(sys.argv[1]); b[0:4] = b'TEXT'; print(b.hex())" "$2")
  xattr -wx "$INFOATTR" "$hex" "$1"
}

# --- sweep: tag presence, FinderInfo and symlinks ----------------------------
F="$W/folder"; mkdir -p "$F"
for i in 1 2 3 4 5 6; do printf 'synthetic report %s\n' "$i" > "$F/report-$i.pdf"; done
printf 'synthetic filed invoice\n' > "$F/filed.pdf"
xattr -w "$TAGATTR" 'opaque-tag-bytes' "$F/filed.pdf"
printf 'synthetic alias payload\n' > "$F/alias.pdf"; finder_info "$F/alias.pdf" 1
printf 'synthetic typed report\n' > "$F/typed.pdf"; finder_info "$F/typed.pdf" 0
INFO_BEFORE=$(xattr -px "$INFOATTR" "$F/typed.pdf")
ln -s filed.pdf "$F/link.pdf"

"$SWEEP" "$F" --json >"$W/plan.json" 2>/dev/null; CODE=$?
assert_eq 0 "$CODE" "sweep plans the folder (exit $CODE)"
assert_eq 1 "$(json_get "$W/plan.json" 'd["details"]["left_alone"]["tagged_skipped"]')" "one tagged file is held"
assert_eq 1 "$(json_get "$W/plan.json" 'd["details"]["skipped"]["aliases_skipped"]')" "one Finder alias is skipped"
assert_eq 1 "$(json_get "$W/plan.json" 'd["details"]["skipped"]["symlinks"]')" "the link to the tagged file is counted as a symlink"
assert_eq 0 "$(json_get "$W/plan.json" 'd["details"]["skipped"]["unreadable"]')" "no attribute read made a file unreadable"
# Six reports, the typed report and the tagged file are probed; the alias is
# skipped before the probe and the symlink is never probed.
assert_eq 8/8/0 "$(observed "$W/plan.json" finder_tag_presence)" "the receipt counts 8 tag-presence reads, all observed"
assert_eq 0 "$(json_get "$W/plan.json" '[c["failed"] for c in d["observations"]["categories"] if c["category"] == "metadata"][0]')" \
  "no metadata read failed, FinderInfo included"
assert_eq none "$(observed "$W/plan.json" finder_tag_names)" "a plain plan reads no tag names"
grep -q 'opaque-tag-bytes' "$W/plan.json" && fail "the plan carries the tag's bytes" || pass "the plan never carries the tag's bytes"

OUT=$("$SWEEP" apply "$F" --yes 2>&1); CODE=$?
assert_eq 0 "$CODE" "apply exits 0 with held items (exit $CODE)"
[ -f "$F/filed.pdf" ] && [ "$(xattr -p "$TAGATTR" "$F/filed.pdf" 2>/dev/null)" = opaque-tag-bytes ] \
  && pass "the tagged file stays where it was, with its tag" || fail "the tagged file moved or lost its tag: $OUT"
[ -f "$F/alias.pdf" ] && pass "the alias stays where it was" || fail "the alias moved: $OUT"
[ -L "$F/link.pdf" ] && pass "the symlink stays where it was" || fail "the symlink moved: $OUT"
TYPED=$(find "$F" -mindepth 2 -name typed.pdf -type f | head -1)
if [ -n "$TYPED" ]; then
  pass "the file with a plain FinderInfo moved with its group"
  assert_eq "$INFO_BEFORE" "$(xattr -px "$INFOATTR" "$TYPED")" "and its FinderInfo is byte for byte the same"
else
  fail "the file with a plain FinderInfo did not move: $OUT"
fi

# --- sweep --scheme tags: read, add, undo -----------------------------------
T="$W/tags"; mkdir -p "$T"
for n in 1 2 3; do printf 'synthetic tagged report %s\n' "$n" > "$T/r$n.pdf"; done
set_tags "$T/r1.pdf" Old
"$SWEEP" "$T" --scheme tags --map pdf=Filed --export-plan "$W/tags.bin" --json >"$W/tags.json" 2>/dev/null; CODE=$?
assert_eq 0 "$CODE" "sweep --scheme tags plans and exports (exit $CODE)"
assert_eq 3/3/0 "$(observed "$W/tags.json" finder_tag_names)" "the receipt counts one tag-name read per file, all observed"
DIGEST=$(json_get "$W/tags.json" 'd["details"]["binding"]["plan_digest"]')
"$SWEEP" apply --plan "$W/tags.bin" --plan-digest "$DIGEST" --yes >"$W/apply.out" 2>&1; CODE=$?
assert_eq 0 "$CODE" "apply adds the tag (exit $CODE)"
assert_eq '["Old", "Filed"]' "$(tags_of "$T/r1.pdf")" "r1.pdf has Old, then Filed"
assert_eq '["Filed"]' "$(tags_of "$T/r2.pdf")" "r2.pdf has Filed"
"$SWEEP" undo "$T" >"$W/undo.out" 2>&1; CODE=$?
assert_eq 0 "$CODE" "undo restores the earlier tags (exit $CODE)"
assert_eq '["Old"]' "$(tags_of "$T/r1.pdf")" "r1.pdf is back to Old"
assert_eq none "$(tags_of "$T/r2.pdf")" "r2.pdf has no tag attribute again"
assert_eq none "$(tags_of "$T/r3.pdf")" "r3.pdf has no tag attribute again"

# --- stash keeps a tag's bytes ------------------------------------------------
S="$W/stash"; mkdir -p "$S"
printf 'synthetic stashed note\n' > "$S/note.txt"
set_tags "$S/note.txt" Keep
BYTES_BEFORE=$(xattr -px "$TAGATTR" "$S/note.txt")
CODE=0; "$STASH" "$S" >/dev/null 2>&1 || CODE=$?
assert_eq 0 "$CODE" "stash clears the folder with a tagged file (exit $CODE)"
CODE=0; "$STASH" pop "$S" >/dev/null 2>&1 || CODE=$?
assert_eq 0 "$CODE" "pop restores it (exit $CODE)"
assert_eq "$BYTES_BEFORE" "$(xattr -px "$TAGATTR" "$S/note.txt" 2>/dev/null)" "the tag attribute came back byte for byte"

# --- unpack extracts to the exact bytes ---------------------------------------
SRC="$W/source"; mkdir -p "$SRC"
printf 'synthetic archive member\n' > "$SRC/member.txt"
tar -czf "$W/fixture.tar.gz" -C "$SRC" member.txt
CODE=0; "$UNPACK" "$W/fixture.tar.gz" --into "$W/out" >/dev/null 2>&1 || CODE=$?
assert_eq 0 "$CODE" "unpack extracts the synthetic archive (exit $CODE)"
assert_eq 'synthetic archive member' "$(cat "$W/out/member.txt" 2>/dev/null)" "the member has its exact bytes"
assert_eq none "$(tags_of "$W/out/member.txt")" "and no tag attribute"
