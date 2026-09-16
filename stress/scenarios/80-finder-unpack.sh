#!/usr/bin/env bash
# A compact, visible unpack fixture for the recorded Finder demonstration.
#
# The scenario deliberately has the same shape as the live fixture: a ZIP
# holding two directories and three non-empty files.  Its list assertion
# captures stdout and the exit code from ONE `unpack --list` invocation, then
# compares the captured bytes with the complete expected listing.  Checking
# names with separate greps would let a failed listing (or extra members) pass.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir)
trap 'rm -rf "$W"' EXIT

SOURCE="$W/source"
ARCHIVE="$W/demo.zip"
EXTRACTED="$W/extracted"
mkdir -p "$SOURCE/docs" "$SOURCE/images"
printf 'agenda for the Finder unpack demonstration\n' > "$SOURCE/docs/agenda.txt"
printf 'photo bytes for the Finder unpack demonstration\n' > "$SOURCE/images/photo.jpg"
printf 'receipt bytes for the Finder unpack demonstration\n' > "$SOURCE/receipt.pdf"

(cd "$SOURCE" && zip -qr "$ARCHIVE" docs images receipt.pdf)

# Keep the stdout and status from this exact invocation together.  stderr is
# separate on purpose: merging it would make a diagnostic look like a member.
LIST_STDOUT="$W/list.stdout"
LIST_STDERR="$W/list.stderr"
LIST_EXIT=0
"$UNPACK" "$ARCHIVE" --list >"$LIST_STDOUT" 2>"$LIST_STDERR" || LIST_EXIT=$?
assert_eq 0 "$LIST_EXIT" "Finder fixture: one unpack --list invocation succeeds"

EXPECTED_LIST="$W/expected-list.stdout"
printf '%s\n' \
  '' \
  '5 entries in demo' \
  '  docs' \
  '  docs/agenda.txt' \
  '  images' \
  '  images/photo.jpg' \
  '  receipt.pdf' > "$EXPECTED_LIST"
if cmp -s "$EXPECTED_LIST" "$LIST_STDOUT"; then
  pass "Finder fixture: the successful listing is exactly the five planned members"
else
  fail "Finder fixture: unpack --list stdout differed from the complete expected plan"
fi

EXTRACT_EXIT=0
"$UNPACK" "$ARCHIVE" --into "$EXTRACTED" >/dev/null 2>&1 || EXTRACT_EXIT=$?
assert_eq 0 "$EXTRACT_EXIT" "Finder fixture: unpack extracts the planned archive"

EXPECTED_TREE="$W/expected-tree"
ACTUAL_TREE="$W/actual-tree"
printf '%s\n' \
  'docs' \
  'docs/agenda.txt' \
  'images' \
  'images/photo.jpg' \
  'receipt.pdf' > "$EXPECTED_TREE"
(cd "$EXTRACTED" && find . -mindepth 1 -print | sed 's#^./##' | LC_ALL=C sort) > "$ACTUAL_TREE"
if cmp -s "$EXPECTED_TREE" "$ACTUAL_TREE"; then
  pass "Finder fixture: extraction has exactly the planned directories and files"
else
  fail "Finder fixture: extraction paths differed from the complete planned tree"
fi

BYTES_OK=1
for path in docs/agenda.txt images/photo.jpg receipt.pdf; do
  cmp -s "$SOURCE/$path" "$EXTRACTED/$path" || BYTES_OK=0
done
if [ "$BYTES_OK" = 1 ]; then
  pass "Finder fixture: every extracted file has the source bytes"
else
  fail "Finder fixture: an extracted file differed from its archived source bytes"
fi

exit 0
