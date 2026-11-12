#!/usr/bin/env bash
# A tag moves nothing, so a file's folder has no bearing on whether it can be
# tagged (issue #221). The issue's own folder: 3 screenshots at the top, 3 more
# inside Screenshots/, and 3 other pngs, tagged with --map png=Work at depth 2.
#
# Held: the plan tags all 9 and says nothing about files being in place, in the
# text and in --json. Applying the exported plan writes the tag on the 3 files
# inside Screenshots/ too, and undo takes every tag off again.
#
# Two more effects the tag plan used to borrow from the folder rules are gone:
# a screenshot no tag takes is listed as matching no group, the same as any
# other file, and a locked Screenshots/ folder holds no file back from a tag.
#
# Tag values are read with xattr and Python's plistlib, never with the code
# under test. Everything is synthetic, in a temp dir, and removed on exit.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
exec </dev/null

if [ "$(uname -s)" != Darwin ]; then
  unproven "tags inside folders sweep made" "Finder tags are a macOS feature"
  exit 0
fi
require python3 "reads --json results and decodes tag values independently of the tool" || exit 0

W=$(workdir) || exit 1
trap 'chflags -R nouchg "$W" 2>/dev/null; rm -rf "${W:?}"; eval "$_stress_state_cleanup"' EXIT
export TAGATTR=com.apple.metadata:_kMDItemUserTags

tags_of() {
  python3 - "$1" <<'PY'
import json, os, plistlib, subprocess, sys
shown = subprocess.run(["xattr", "-px", os.environ["TAGATTR"], sys.argv[1]], capture_output=True, text=True)
print("none" if shown.returncode else json.dumps(
    plistlib.loads(bytes.fromhex(shown.stdout.replace("\n", "").replace(" ", ""))), ensure_ascii=False))
PY
}
json_get() {
  python3 - "$1" "$2" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
print(eval(sys.argv[2], {"d": d}))
PY
}

# --- the issue's folder ---------------------------------------------------------
D="$W/t"; mkdir -p "$D/Screenshots"
for i in 1 2 3; do
  for name in "Screenshot 2026-01-0$i at 9.00.0$i AM.png" "notes$i.png" \
    "Screenshots/Screenshot 2025-01-0$i at 9.00.0$i AM.png"; do
    printf 'synthetic %s\n' "$name" > "$D/$name"
    touch -t 202501010900 "$D/$name"
  done
done

"$SWEEP" "$D" --scheme tags --map png=Work --depth 2 >"$W/plan.out" 2>&1; CODE=$?
assert_eq 0 "$CODE" "the tag plan exits 0"
grep -q 'Work *9 files' "$W/plan.out" \
  && pass "the plan tags all 9 pngs" || fail "the plan does not tag all 9: $(grep Work "$W/plan.out")"
grep -q 'already in their destination' "$W/plan.out" \
  && fail "the plan still calls files already in their destination" \
  || pass "the plan calls no file already in its destination"

"$SWEEP" "$D" --scheme tags --map png=Work --depth 2 --export-plan "$W/plan.bin" --json \
  >"$W/plan.json" 2>/dev/null; CODE=$?
assert_eq 0 "$CODE" "the exported plan exits 0"
assert_eq 9 "$(json_get "$W/plan.json" "sum(g['count'] for g in d['details']['groups'])")" \
  "the JSON plan tags 9 files"
assert_eq "0 []" "$(json_get "$W/plan.json" \
  "f\"{d['details']['left_alone']['in_place']} {d['details']['left_alone']['in_place_paths']}\"")" \
  "the JSON plan keeps in_place, at 0 and with no paths"

DIGEST=$(json_get "$W/plan.json" 'd["details"]["binding"]["plan_digest"]')
"$SWEEP" apply --plan "$W/plan.bin" --plan-digest "$DIGEST" --yes >"$W/apply.out" 2>&1; CODE=$?
assert_eq 0 "$CODE" "the plan applies (exit $CODE)"
inside=0
for f in "$D"/Screenshots/*.png; do
  [ "$(tags_of "$f")" = '["Work"]' ] && inside=$((inside+1))
done
assert_eq 3 "$inside" "the 3 files inside Screenshots/ carry Work"
assert_eq '["Work"]' "$(tags_of "$D/Screenshot 2026-01-01 at 9.00.01 AM.png")" \
  "a loose screenshot carries Work"
assert_eq '["Work"]' "$(tags_of "$D/notes2.png")" "an ordinary png carries Work"

"$SWEEP" undo "$D" >"$W/undo.out" 2>&1; CODE=$?
assert_eq 0 "$CODE" "undo exits 0"
left=0
for f in "$D"/*.png "$D"/Screenshots/*.png; do
  [ "$(tags_of "$f")" = none ] || left=$((left+1))
done
assert_eq 0 "$left" "undo took the tag off all 9 files"
[ "$(cat "$D/Screenshots/Screenshot 2025-01-02 at 9.00.02 AM.png")" = \
  'synthetic Screenshots/Screenshot 2025-01-02 at 9.00.02 AM.png' ] \
  && pass "no file's bytes changed" || fail "a file's bytes changed"

# --- a screenshot no tag takes matches no group ---------------------------------
E="$W/plain"; mkdir -p "$E/Screenshots"
for i in 1 2 3; do
  printf 'synthetic shot %s\n' "$i" > "$E/Screenshot 2026-01-0$i at 9.00.0$i AM.png"
done
printf 'synthetic notes\n' > "$E/notes.pdf"
touch -t 202501010900 "$E"/*.png "$E"/*.pdf
"$SWEEP" "$E" --json >"$W/plain-folders.json" 2>/dev/null
assert_eq "Screenshots:3" \
  "$(json_get "$W/plain-folders.json" "' '.join(f\"{g['name']}:{g['count']}\" for g in d['details']['groups'] if g['name'] == 'Screenshots')")" \
  "a folder plan of the same folder files the 3 screenshots"
"$SWEEP" "$E" --scheme tags --map pdf=Read --json >"$W/plain.json" 2>/dev/null; CODE=$?
assert_eq 0 "$CODE" "a pdf tag plan exits 0"
assert_eq 3 "$(json_get "$W/plain.json" \
  "sum('Screenshot' in p for p in d['details']['left_alone']['no_clear_group_paths'])")" \
  "the tag plan lists the 3 screenshots as matching no group"

# --- a locked Screenshots/ folder holds nothing back from a tag -----------------
L="$W/locked"; mkdir -p "$L/Screenshots"
for i in 1 2 3; do
  printf 'synthetic shot %s\n' "$i" > "$L/Screenshot 2026-01-0$i at 9.00.0$i AM.png"
done
touch -t 202501010900 "$L"/*.png
chflags uchg "$L/Screenshots"
"$SWEEP" "$L" --json >"$W/locked-folders.json" 2>/dev/null
"$SWEEP" "$L" --scheme tags --map png=Work --json >"$W/locked.json" 2>/dev/null; CODE=$?
assert_eq 0 "$CODE" "a tag plan beside a locked Screenshots/ exits 0"
FOLDER_LOCKED=$(json_get "$W/locked-folders.json" "d['details']['skipped']['locked_skipped']")
TAG_LOCKED=$(json_get "$W/locked.json" "d['details']['skipped']['locked_skipped']")
[ "$FOLDER_LOCKED" -ge 3 ] \
  && pass "a folder plan holds the 3 screenshots for the locked folder ($FOLDER_LOCKED locked)" \
  || fail "the folder plan did not hold the screenshots ($FOLDER_LOCKED locked)"
assert_eq "$((FOLDER_LOCKED - 3))" "$TAG_LOCKED" "the tag plan does not count the screenshots as locked"
assert_eq 3 "$(json_get "$W/locked.json" "sum(g['count'] for g in d['details']['groups'])")" \
  "the tag plan tags the 3 screenshots"
true
