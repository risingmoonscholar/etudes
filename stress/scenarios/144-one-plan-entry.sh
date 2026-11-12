#!/usr/bin/env bash
# Every scheme now builds its plan through one entry point: the evidence, then
# the safety holds, then the scheme's own rules in a fixed order. The plans are
# what they were.
#
# - One folder planned four ways: sweep's own rules, the same with a --map
#   route (claimed last), a saved existing-folder profile (claimed first), and
#   a tag plan (which moves nothing and calls nothing in place).
# - Recent files: the plan still asks whether they would group once the window
#   passes, and says --since 0 helps only when they would.
# - A folder of files no group claims plans in time that grows in line with
#   the file count (issue #229): 20,000 files take no more than 8 times as long
#   as 5,000. A search per file made it about 12 times.
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

# --- one folder, every scheme -------------------------------------------------
D="$W/desk"; mkdir -p "$D/Documents"
for n in 1 2 3; do
  for name in "Screenshot 2026-01-01 at 10.00.0$n.png" "IMG_000$n.JPG" "app$n.dmg" \
    "doc$n.pdf" "model$n.bpy"; do
    printf 'synthetic %s\n' "$name" > "$D/$name"
  done
done
printf 'synthetic old.pdf\n' > "$D/Documents/old.pdf"
touch -t "$OLD" "$D"/* "$D"/Documents/*

"$SWEEP" "$D" --depth 2 --json >"$W/folders.json" 2>/dev/null; CODE=$?
assert_eq 0 "$CODE" "a folder plan exits 0"
assert_eq "Screenshots:3 Photos, Jan 1:3 Installers:3 Documents:3" \
  "$(json_get "$W/folders.json" "$GROUP_COUNTS")" \
  "screenshots, the camera burst, installers, then the type family, 3 each"
assert_eq "1 3" "$(json_get "$W/folders.json" "f\"{d['left_alone']['in_place']} {d['left_alone']['no_clear_group']}\"")" \
  "the pdf already in Documents stays in place, and the 3 .bpy files match no group"

"$SWEEP" "$D" --depth 2 --map bpy=Blender --json >"$W/mapped.json" 2>/dev/null; CODE=$?
assert_eq 0 "$CODE" "a folder plan with a --map route exits 0"
assert_eq "Screenshots:3 Photos, Jan 1:3 Installers:3 Documents:3 Blender:3" \
  "$(json_get "$W/mapped.json" "$GROUP_COUNTS")" \
  "the --map route claims last, after every built-in rule"
assert_eq 0 "$(json_get "$W/mapped.json" "d['left_alone']['no_clear_group']")" \
  "the route takes the 3 files no built-in rule claimed"

PROFILE="$ETUDE_STATE_DIR/sweep-existing-profile.toml"
printf 'schema_version = 1\n[[routes]]\nextension = "pdf"\nfolder = "Reading"\n[[routes]]\nextension = "png"\nfolder = "Pics"\n' > "$PROFILE"
"$SWEEP" "$D" --depth 2 --scheme existing --json >"$W/existing.json" 2>/dev/null; CODE=$?
assert_eq 0 "$CODE" "an existing-folder plan exits 0"
assert_eq "Reading:4 Pics:3 Photos, Jan 1:3 Installers:3" \
  "$(json_get "$W/existing.json" "$GROUP_COUNTS")" \
  "saved routes claim first, in profile order: the pdfs (the one in Documents too) and the screenshots"
rm -f "$PROFILE"

"$SWEEP" "$D" --depth 2 --scheme tags --map pdf=Work --map bpy=Red --json >"$W/tags.json" 2>/dev/null; CODE=$?
assert_eq 0 "$CODE" "a tag plan exits 0"
assert_eq "Work:4 Red:3" "$(json_get "$W/tags.json" "$GROUP_COUNTS")" \
  "the tag plan tags all 4 pdfs and the 3 .bpy files"
assert_eq "0 9" "$(json_get "$W/tags.json" "f\"{d['left_alone']['in_place']} {d['left_alone']['no_clear_group']}\"")" \
  "nothing in a tag plan is in place, and the 9 files no tag names match no group"

"$SWEEP" apply "$D" --depth 2 --map bpy=Blender --yes >"$W/apply.out" 2>&1; CODE=$?
assert_eq 0 "$CODE" "the mapped plan applies (exit $CODE)"
[ -f "$D/Blender/model1.bpy" ] && [ -f "$D/Documents/doc1.pdf" ] && [ -f "$D/Documents/old.pdf" ] \
  && pass "each file moved into its rule's folder, and the one in place stayed" \
  || fail "a file is not where its rule put it"
"$SWEEP" undo "$D" >"$W/undo.out" 2>&1; CODE=$?
assert_eq 0 "$CODE" "undo exits 0"
[ "$(cat "$D/model1.bpy" 2>/dev/null)" = "synthetic model1.bpy" ] && [ ! -e "$D/Blender" ] \
  && [ -f "$D/Documents/old.pdf" ] && pass "undo put every file back and removed the folders it made" \
  || fail "undo left the folder changed"

# --- recent files: would they group once the window passes? ------------------
# The suite runs with no window; these runs set the usual one-day window.
R="$W/recent"; mkdir -p "$R"
for n in 1 2 3; do printf 'fresh %s\n' "$n" > "$R/fresh$n.pdf"; done
printf 'old\n' > "$R/lone.xyz"; touch -t "$OLD" "$R/lone.xyz"
OUT=$(SWEEP_GRACE_SECS=86400 "$SWEEP" "$R" 2>&1)
case "$OUT" in *"--since 0 would include the"*) pass "3 recent pdfs would group, so --since 0 is offered" ;;
  *) fail "the plan does not offer --since 0 for 3 recent pdfs: ${OUT##*$'\n'}" ;; esac
rm "$R/fresh3.pdf"
OUT=$(SWEEP_GRACE_SECS=86400 "$SWEEP" "$R" 2>&1)
case "$OUT" in *"on their own they would not form a group"*) pass "2 recent pdfs would not group, and the plan says so" ;;
  *) fail "the plan does not say 2 recent pdfs would not group" ;; esac

# --- planning time grows in line with the files (issue #229) ------------------
best_seconds() {  # DIR N: the faster of two plans of DIR; checks no group formed
  python3 - "$SWEEP" "$1" "$2" <<'PY'
import json, subprocess, sys, time
sweep, folder, n = sys.argv[1], sys.argv[2], int(sys.argv[3])
best = None
for _ in range(2):
    start = time.monotonic()
    run = subprocess.run([sweep, folder, "--json"], capture_output=True, text=True)
    took = time.monotonic() - start
    details = json.loads(run.stdout)["details"]
    # A few numbered names read as tax forms (f01099) and are held as
    # personal; every other file matches no group.
    left = details["left_alone"]["no_clear_group"] + details["left_alone"]["looks_personal"]
    if details["groups"] or left != n:
        print(f"bad {left}")
        sys.exit(0)
    best = took if best is None else min(best, took)
print(f"{best:.3f}")
PY
}
make_flat() {  # DIR N: N empty .xyz files, old enough for any window
  mkdir -p "$1"
  python3 - "$1" "$2" <<'PY'
import os, sys
folder, n = sys.argv[1], int(sys.argv[2])
for i in range(n):
    path = os.path.join(folder, f"f{i:05d}.xyz")
    open(path, "w").close()
    os.utime(path, (1_577_880_000, 1_577_880_000))
PY
}
make_flat "$W/flat5" 5000
make_flat "$W/flat20" 20000
T5=$(best_seconds "$W/flat5" 5000)
T20=$(best_seconds "$W/flat20" 20000)
case "$T5 $T20" in
  *bad*) fail "a flat folder plan formed a group or lost a file ($T5 / $T20)" ;;
  *) python3 -c "import sys; sys.exit(0 if float('$T20') <= 8 * max(float('$T5'), 0.05) else 1)" \
       && pass "20,000 files plan in at most 8 times the time of 5,000 (${T5}s, ${T20}s)" \
       || fail "planning grew faster than the file count: ${T5}s for 5,000, ${T20}s for 20,000" ;;
esac

# --- stash and unpack, which share etude-core ---------------------------------
CODE=0; "$STASH" "$D" >/dev/null 2>&1 || CODE=$?
assert_eq 0 "$CODE" "stash runs on the folder (exit $CODE)"
[ ! -e "$D/doc1.pdf" ] && pass "stash took the ordinary files" || fail "stash left an ordinary file in the open folder"
CODE=0; "$STASH" pop "$D" >/dev/null 2>&1 || CODE=$?
assert_eq 0 "$CODE" "pop restores them (exit $CODE)"
[ "$(cat "$D/doc1.pdf" 2>/dev/null)" = "synthetic doc1.pdf" ] \
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
