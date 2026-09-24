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
export ETUDE_JOURNAL_KEY="$(python3 -c 'import secrets; print(secrets.token_hex(32))')"
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

BEFORE="$W/before.json"
AFTER_PLAN="$W/after-plan.json"
APP_BEFORE="$W/app-before.json"
APP_AFTER="$W/app-after.json"
LIB_BEFORE="$W/library-before.json"
LIB_AFTER="$W/library-after.json"
snapshot_tree "$D" "$BEFORE"

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

snapshot_tree "$D" "$AFTER_PLAN"
assert_snapshot_eq "$BEFORE" "$AFTER_PLAN" "planning at every depth changed no path or byte"

# --- Apply leaves packages in place and byte-identical ----------------------
snapshot_tree "$APP" "$APP_BEFORE"
snapshot_tree "$LIB" "$LIB_BEFORE"

assert_exit 0 "apply succeeds with a package in the accepted group" \
  -- "$SWEEP" apply "$D" --depth 4 --yes

snapshot_tree "$APP" "$APP_AFTER"
snapshot_tree "$LIB" "$LIB_AFTER"
assert_snapshot_eq "$APP_BEFORE" "$APP_AFTER" ".app package stayed at its original path with every byte intact"
assert_snapshot_eq "$LIB_BEFORE" "$LIB_AFTER" ".photoslibrary stayed at its original path with every byte intact"
[ ! -e "$D/deck_notes_0.pdf" ] && find "$D" -mindepth 2 -type f -name deck_notes_0.pdf | grep -q . \
  && pass "a representative loose document moved while both packages stayed opaque" \
  || fail "the loose document did not move independently of the packages"
