#!/usr/bin/env bash
# .app, .rtfd, .bundle, .framework and .photoslibrary are directories the
# user experiences as one file. Recursing into one and scattering its
# contents doesn't just misfile things. For a .app it destroys a working
# application (code signature included). For a .photoslibrary it is a
# privacy catastrophe (every photo's real path exposed as a loose file).
#
# Attack: a package containing a nested package (a .framework inside an
# .app, the real shape of every non-trivial macOS app) and a file planted
# inside that would be refused as sensitive if sweep ever saw it loose.
# Checked at every depth 1..8: the package must never be entered, and its
# contents must never appear anywhere in a plan.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir); trap 'rm -rf "$W"' EXIT
D="$W/Desktop"
APP="$D/Northwind Deck.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
mkdir -p "$APP/Contents/Frameworks/Sparkle.framework/Versions/A"
: > "$APP/Contents/Info.plist"
echo "SYNTHETIC-SENSITIVE" > "$APP/Contents/MacOS/SSN_card_scan.jpg"
: > "$APP/Contents/Frameworks/Sparkle.framework/Versions/A/Sparkle"

# A .photoslibrary too: same opacity rule, different consequence.
LIB="$D/Northwind Deck Photos.photoslibrary"
mkdir -p "$LIB/originals/2024/03"
echo "SYNTHETIC-SENSITIVE" > "$LIB/originals/2024/03/passport_scan.png"

for i in 0 1 2 3 4; do : > "$D/deck_notes_$i.pdf"; done

BEFORE=$(find "$D" -type f | wc -l | tr -d ' ')

# --- Opacity holds at every depth sweep supports ---
for depth in 1 2 3 4 8; do
  json=$("$SWEEP" "$D" --depth "$depth" --json 2>&1)
  scanned=$(python3 -c "import json,sys; print(json.load(sys.stdin)['scanned'])" <<<"$json" 2>/dev/null)
  leaked=$(python3 -c "
import json, sys
d = json.load(sys.stdin)
paths = []
for g in d['groups']:
    paths += g['members']
paths += d['left_alone']['no_clear_group_paths']
leaks = [p for p in paths if 'Contents' in p or 'originals' in p]
print(len(leaks))
" <<<"$json" 2>/dev/null)
  assert_eq 7 "$scanned" "depth $depth: both packages count as ONE entry each (5 loose files + 2 packages)"
  assert_eq 0 "$leaked" "depth $depth: nothing from inside either package appears in the plan"
done

assert_intact "$D" "$BEFORE" "planning at any depth moved nothing"

# --- The package survives an apply byte-for-byte, moved as one atomic unit ---
before_app_count=$(find "$APP" -type f | wc -l | tr -d ' ')
before_lib_count=$(find "$LIB" -type f | wc -l | tr -d ' ')

assert_exit 0 "apply succeeds with a package in the accepted group" \
  -- "$SWEEP" apply "$D" --depth 4 --yes

moved_app=$(find "$D" -maxdepth 2 -iname "Northwind Deck.app" -type d)
moved_lib=$(find "$D" -maxdepth 2 -iname "Northwind Deck Photos.photoslibrary" -type d)
if [ -n "$moved_app" ] && [ -n "$moved_lib" ]; then
  pass "both packages relocated as whole units, not scattered"
  assert_eq "$before_app_count" \
            "$(find "$moved_app" -type f | wc -l | tr -d ' ')" \
            ".app internal file count unchanged by the move"
  assert_eq "$before_lib_count" \
            "$(find "$moved_lib" -type f | wc -l | tr -d ' ')" \
            ".photoslibrary internal file count unchanged by the move"
  content=$(cat "$moved_app/Contents/MacOS/SSN_card_scan.jpg" 2>/dev/null)
  assert_eq "SYNTHETIC-SENSITIVE" "$content" "file inside the package is byte-identical after the move (and was never independently classified as sensitive. the package itself was the unit)"
else
  fail "a package did not survive the apply intact: app=[$moved_app] lib=[$moved_lib]"
fi

AFTER=$(find "$D" -type f | wc -l | tr -d ' ')
assert_eq "$BEFORE" "$AFTER" "no file was lost across the whole run"
