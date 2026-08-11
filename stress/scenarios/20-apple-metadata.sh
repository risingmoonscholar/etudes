#!/usr/bin/env bash
# The debris every real macOS folder accumulates: .DS_Store, AppleDouble
# resource-fork shadows (._name), .localized, and the custom-folder-icon
# file whose name is literally "Icon" followed by a raw carriage return
# (no dot prefix — Finder hides it via an attribute flag, not by naming
# convention). None of this is the user's data; none of it should be
# grouped, moved as if meaningful, or allowed to corrupt output formatting.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

if ! require python3 "writes the literal Icon<CR> filename unambiguously"; then
  exit 0
fi

W=$(workdir); trap 'rm -rf "$W"' EXIT
D="$W/Desktop"
mkdir -p "$D"

: > "$D/.DS_Store"
: > "$D/._SomeFile.pdf"
: > "$D/.localized"
python3 -c "
import os
open(os.path.join('$D', 'Icon\r'), 'wb').write(b'ICONBYTES')
"
for i in 0 1 2 3 4; do : > "$D/deck_notes_$i.pdf"; done

# --- Apple's own dotfiles are ignored, not grouped, not counted as scanned ---
json=$("$SWEEP" "$D" --json 2>&1)
scanned=$(python3 -c "import json,sys; print(json.load(sys.stdin)['scanned'])" <<<"$json")
hidden=$(python3 -c "import json,sys; print(json.load(sys.stdin)['skipped']['hidden'])" <<<"$json")
assert_eq 6 "$scanned" "scanned excludes .DS_Store, ._SomeFile.pdf and .localized (5 deck files + Icon<CR>)"
assert_eq 3 "$hidden" "all three dotfiles counted as skipped-hidden"

# --- Icon<CR> does not break JSON output ---
# Read as raw stdin bytes, never through a Python string literal — a source
# embedding of "\r" would get double-interpreted by Python's own escape
# handling before json even saw it, which would test this script's
# quoting instead of sweep's escaping.
valid=$(python3 -c "
import json, sys
try:
    json.load(sys.stdin)
    print('ok')
except Exception as e:
    print(f'INVALID: {e}')
" <<<"$json" 2>&1)
assert_eq "ok" "$valid" "--json stays valid JSON with a carriage-return in a filename"

icon_present=$(python3 -c "
import json, sys
d = json.load(sys.stdin)
paths = d['left_alone']['no_clear_group_paths']
print(any('Icon' in p for p in paths))
" <<<"$json")
assert_eq "True" "$icon_present" "Icon<CR> is reported (left alone, no clear group) rather than silently vanishing"

# --- Round-trip Icon<CR> through a real move (stash) and check the exact
#     bytes, not a terminal rendering of them — a bare \r moves the cursor,
#     so 'ls' output alone would lie about whether the name survived intact.
S="$W/ScreenShareFolder"
mkdir -p "$S"
python3 -c "
import os
open(os.path.join('$S', 'Icon\r'), 'wb').write(b'ICONBYTES')
"
: > "$S/other.txt"

assert_exit 0 "stash moves a folder containing Icon<CR> without erroring" -- "$STASH" "$S"

byte_exact=$(python3 -c "
import os
names = os.listdir('$S')
holding = [n for n in names if n.startswith('.stash-')]
if not holding:
    print('NO-HOLDING-DIR')
else:
    inner = os.listdir(os.path.join('$S', holding[0]))
    print('Icon\r' in inner)
")
assert_eq "True" "$byte_exact" "the stashed name is byte-exact Icon+CR, not truncated or reinterpreted"

assert_exit 0 "stash pop restores it" -- "$STASH" pop "$S"
popped_exact=$(python3 -c "
import os
print('Icon\r' in os.listdir('$S'))
")
assert_eq "True" "$popped_exact" "Icon<CR> round-trips through stash/pop byte-exact"
popped_content=$(python3 -c "
import os
print(open(os.path.join('$S','Icon\r'),'rb').read().decode())
")
assert_eq "ICONBYTES" "$popped_content" "content survived the round trip untouched"
