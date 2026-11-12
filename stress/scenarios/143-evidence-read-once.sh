#!/usr/bin/env bash
# What sweep knows about a file (its type family, and whether its name says
# screenshot, camera or installer) is now worked out once per file, before the
# safety holds, and every hold and grouping rule reads that one answer. The
# folders, their order and the holds are what they were.
#
# - Every built-in rule still groups, in the same order, under the same names.
# - A loose project document holds the images beside it but not the
#   screenshots, in a folder plan and a tag plan.
# - A project folder found while scanning holds the images beside it but not
#   the screenshots: the walk and the plan use the same list of families a
#   project can reference.
# - --map still refuses a built-in extension and a built-in folder name.
#
# etude-core is shared by all three tools: stash still clears and restores the
# folder byte for byte, and unpack still extracts a synthetic archive to its
# exact bytes. Everything is synthetic, in a temp dir, and removed on exit.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
exec </dev/null

W=$(workdir) || exit 1; trap 'rm -rf "${W:?}"; eval "$_stress_state_cleanup"' EXIT
require python3 "these checks read JSON with Python" || exit 0
# Noon, so the camera group's date is the same day in every time zone.
OLD=202001011200

json_get() {
  python3 - "$1" "$2" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))["details"]
print(eval(sys.argv[2], {"d": d}))
PY
}
GROUP_COUNTS="' '.join(f\"{g['name']}:{g['count']}\" for g in d['groups'])"

# --- every built-in rule, once ------------------------------------------------
D="$W/desk"; mkdir -p "$D"
for n in 1 2 3; do
  for name in "Screenshot 2026-01-01 at 10.00.0$n.png" "IMG_000$n.JPG" "app$n.dmg" \
    "doc$n.pdf" "song$n.mp3" "table$n.csv" "bundle$n.zip" "tool$n.py" "picture$n.gif"; do
    printf 'synthetic %s\n' "$name" > "$D/$name"
  done
done
printf 'synthetic model.bpy\n' > "$D/model.bpy"
touch -t "$OLD" "$D"/*

"$SWEEP" "$D" --json >"$W/desk.json" 2>/dev/null; CODE=$?
assert_eq 0 "$CODE" "a folder plan exits 0"
assert_eq "Screenshots:3 Photos, Jan 1:3 Installers:3 Archives:3 Data:3 Documents:3 Images:3 Media:3 Scripts:3" \
  "$(json_get "$W/desk.json" "$GROUP_COUNTS")" \
  "screenshots, the camera burst, installers, then each type family by folder name, 3 each"
assert_eq "{'bpy': 1}" "$(json_get "$W/desk.json" "d['left_alone']['unknown_extensions']")" \
  "the one file with an extension sweep does not know is left alone"

"$SWEEP" apply "$D" --yes >"$W/apply.out" 2>&1; CODE=$?
assert_eq 0 "$CODE" "the plan applies (exit $CODE)"
[ -f "$D/Screenshots/Screenshot 2026-01-01 at 10.00.01.png" ] && [ -f "$D/Photos, Jan 1/IMG_0001.JPG" ] \
  && [ -f "$D/Installers/app1.dmg" ] && [ -f "$D/Data/table1.csv" ] && [ -f "$D/model.bpy" ] \
  && pass "each file moved into its rule's folder, and the unknown one stayed" \
  || fail "a file is not where its rule put it"
"$SWEEP" undo "$D" >"$W/undo.out" 2>&1; CODE=$?
assert_eq 0 "$CODE" "undo exits 0"
[ "$(cat "$D/IMG_0001.JPG" 2>/dev/null)" = "synthetic IMG_0001.JPG" ] && [ ! -e "$D/Screenshots" ] \
  && [ ! -e "$D/Data" ] && pass "undo put every file back and removed the folders it made" \
  || fail "undo left the folder changed"

# --- a loose project document: images held, screenshots not -------------------
P="$W/loose"; mkdir -p "$P"
printf 'synthetic mix.flp\n' > "$P/mix.flp"
for n in 1 2 3; do
  printf 'shot %s\n' "$n" > "$P/Screenshot 2026-01-01 at 10.00.0$n.png"
  printf 'texture %s\n' "$n" > "$P/texture$n.png"
  printf 'doc %s\n' "$n" > "$P/doc$n.pdf"
done
touch -t "$OLD" "$P"/*
"$SWEEP" "$P" --json >"$W/loose.json" 2>/dev/null; CODE=$?
assert_eq 0 "$CODE" "a folder plan beside a project document exits 0"
assert_eq "Screenshots:3 Documents:3" "$(json_get "$W/loose.json" "$GROUP_COUNTS")" \
  "the screenshots and the pdfs still sort beside the .flp"
assert_eq 3 "$(json_get "$W/loose.json" "d['left_alone']['held_near_document']")" \
  "the 3 textures, which the .flp could reference, are held"
"$SWEEP" "$P" --scheme tags --map png=Work --json >"$W/loose-tags.json" 2>/dev/null; CODE=$?
assert_eq 0 "$CODE" "a tag plan beside a project document exits 0"
assert_eq "Work:3" "$(json_get "$W/loose-tags.json" "$GROUP_COUNTS")" \
  "the tag plan tags the 3 screenshots only"
assert_eq 3 "$(json_get "$W/loose-tags.json" "d['left_alone']['held_near_document']")" \
  "the tag plan holds the same 3 textures"

# --- a project folder found by the walk: the same families held ---------------
N="$W/nest"; mkdir -p "$N/work/scenes"
printf 'synthetic main.blend\n' > "$N/work/scenes/main.blend"
for n in 1 2 3; do
  printf 'wood %s\n' "$n" > "$N/work/wood_$n.png"
  printf 'shot %s\n' "$n" > "$N/work/Screenshot 2026-01-01 at 10.00.0$n.png"
  printf 'doc %s\n' "$n" > "$N/work/doc$n.pdf"
done
touch -t "$OLD" "$N"/work/* "$N"/work/scenes/*
"$SWEEP" "$N" --depth 2 --json >"$W/nest.json" 2>/dev/null; CODE=$?
assert_eq 0 "$CODE" "a depth-2 plan over a project folder exits 0"
assert_eq "Screenshots:3 Documents:3" "$(json_get "$W/nest.json" "$GROUP_COUNTS")" \
  "the screenshots and pdfs beside the project folder still sort"
assert_eq 3 "$(json_get "$W/nest.json" "d['left_alone']['held_near_document']")" \
  "the 3 wood textures beside scenes/main.blend are held by the walk"

# --- --map still knows the built-in extensions and folder names ---------------
CODE=0; "$SWEEP" "$D" --map csv=Tables >"$W/map1.out" 2>&1 || CODE=$?
assert_eq 2 "$CODE" "--map of a built-in extension is refused"
grep -q "already belongs to a built-in group" "$W/map1.out" \
  && pass "and says the extension belongs to a built-in group" || fail "the refusal does not say why"
for folder in media DATA Screenshots installers; do
  CODE=0; "$SWEEP" "$D" --map bpy="$folder" >"$W/map2.out" 2>&1 || CODE=$?
  assert_eq 2 "$CODE" "--map into the built-in folder name $folder is refused"
done

# --- stash and unpack, which share etude-core ---------------------------------
CODE=0; "$STASH" "$P" >/dev/null 2>&1 || CODE=$?
assert_eq 0 "$CODE" "stash runs on the project-document folder (exit $CODE)"
[ ! -e "$P/doc1.pdf" ] && pass "stash took the ordinary files" || fail "stash left an ordinary file in the open folder"
CODE=0; "$STASH" pop "$P" >/dev/null 2>&1 || CODE=$?
assert_eq 0 "$CODE" "pop restores them (exit $CODE)"
[ "$(cat "$P/doc1.pdf" 2>/dev/null)" = "doc 1" ] && [ "$(cat "$P/mix.flp" 2>/dev/null)" = "synthetic mix.flp" ] \
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
