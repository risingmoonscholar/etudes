#!/usr/bin/env bash
# Five real project types, and the promise this guard makes: pointed AT a
# project, or at a folder holding one, sweep steps over the project rather
# than sorting its files in with the loose ones.
#
# That promise has a known hole, and stating it wider than it is would be the
# same kind of untruth this repo keeps fixing. A project DOCUMENT (.blend,
# .flp, .als, .song, .ptx) references its assets upward, out of its own
# folder, so stepping over that folder does not save assets sitting beside it.
# Issue #49 carries the four reproductions. Nothing in this scenario asserts
# the current behaviour is correct there.
#
# Every layout here is MEASURED, not imagined. An earlier version of this
# scenario included Unreal and Premiere shapes built from vendor docs; they
# were dropped because researched-not-measured is exactly the mistake that let
# project.godot go missing from the guard in the first place. These five were
# each read off a real disk:
#
#   godot    -- an 18,724-file project on this machine. .tscn files reference
#               siblings as res://scripts/main.gd, absolute from the root, and
#               a .import sidecar sits beside every imported asset.
#   blender  -- a downloaded asset pack in Downloads. // paths relative to the
#               .blend, plus a .blend1 backup written beside it.
#   flp      -- FL Studio, folder-per-project, with a Backup/ of timestamped
#               autosaves: "NAME (autosaved at 16h00).flp". That is crash
#               recovery -- version history -- and sorting it is the worst
#               thing a tool could do to it.
#   song     -- Studio One, folder-per-song, with Media/ Cache/ History/
#               Bounces/ Stems/ subdirectories. Media/ is a name THIS tool
#               would create, so a project that already has one is a direct
#               collision hazard.
#   ptx      -- Pro Tools, where ONE folder can hold several .ptx sessions,
#               beside Audio Files/ and Session File Backups/, each session
#               shadowed by an AppleDouble ._ twin.
#
# The guard does not know what any of these programs are. It refuses a folder
# that holds a project marker, promptly, and says which file made it refuse.
# That is the whole mechanism. Format-specific cleverness belongs in a fork.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir); trap 'rm -rf "$W"' EXIT

build_godot() {
  local d="$1"
  mkdir -p "$d/scenes" "$d/scripts" "$d/assets/textures" "$d/.godot/imported"
  printf 'config_version=5\n[application]\nrun/main_scene="res://scenes/main.tscn"\n' > "$d/project.godot"
  printf '[gd_scene format=4]\n[ext_resource path="res://scripts/main.gd"]\n' > "$d/scenes/main.tscn"
  : > "$d/scripts/main.gd"; : > "$d/scripts/player.gd"
  local c
  for c in ground rock sky; do : > "$d/assets/textures/$c.png"; : > "$d/assets/textures/$c.png.import"; done
  for c in 06 12 17; do : > "$d/capture_t$c.png"; : > "$d/capture_t$c.png.import"; done
  : > "$d/README.md"; : > "$d/CHANGELOG.md"
}

build_blender() {
  local d="$1" c
  mkdir -p "$d/textures"
  : > "$d/Scene.blend"; : > "$d/Scene.blend1"
  for c in Color Normal Roughness Displacement; do : > "$d/textures/Ground_$c.png"; done
  : > "$d/render_final.png"; : > "$d/notes.txt"
}

build_flp() {
  # Folder-per-project with a Backup/ of timestamped autosaves -- the version
  # history. Note: Backup/ ALSO holds .flp, so it is refused too, which is
  # correct: it is the crash-recovery record for this project.
  local d="$1" c
  mkdir -p "$d/Backup" "$d/Audio"
  : > "$d/anthem.flp"
  for c in "16h00" "15h42" "14h30"; do : > "$d/Backup/anthem (autosaved at $c).flp"; done
  : > "$d/Audio/render.wav"; : > "$d/reference.mp3"
}

build_song() {
  # Studio One. Media/ Cache/ History/ Bounces/ Stems/ -- and Media/ is a
  # folder name THIS tool creates. A project holding one must still be refused
  # whole; the collision must never happen because the project is never
  # entered.
  local d="$1" c
  mkdir -p "$d/Media" "$d/Cache" "$d/History" "$d/Bounces" "$d/Stems"
  : > "$d/closer.song"
  : > "$d/Media/vocal.wav"; : > "$d/Media/guitar.wav"
  for c in 1 2 3; do : > "$d/History/closer-$c.song"; done
  : > "$d/Bounces/mixdown.wav"; : > "$d/Stems/drums.wav"
}

build_ptx() {
  # Pro Tools. TWO sessions in one folder, so folder name != session name --
  # the marker still fires on either. Each session shadowed by an AppleDouble
  # ._ twin, which is hidden and must never be treated as a loose file.
  local d="$1"
  mkdir -p "$d/Audio Files" "$d/Bounced Files" "$d/Session File Backups"
  : > "$d/Ever Green.ptx"; : > "$d/._Ever Green.ptx"
  : > "$d/Song 2.ptx"; : > "$d/._Song 2.ptx"
  : > "$d/Audio Files/kick.wav"; : > "$d/Audio Files/._kick.wav"
  : > "$d/WaveCache.wfm"
}

declare -a NAMES=(godot blender flp song ptx)
declare -a MARKERS=("project.godot" ".blend" ".flp" ".song" ".ptx")

REFUSED=0
for i in "${!NAMES[@]}"; do
  eng="${NAMES[$i]}"; d="$W/roots/$eng"; "build_$eng" "$d"
  BEFORE=$(find "$d" -type f | wc -l | tr -d ' ')
  OUT=$("$SWEEP" "$d" 2>&1); CODE=$?
  AFTER=$(find "$d" -type f | wc -l | tr -d ' ')
  if [ "$CODE" = "2" ] && grep -qi "looks like a project" <<<"$OUT"; then
    REFUSED=$((REFUSED + 1))
  else
    fail "$eng: a real project layout was not refused as a root (exit $CODE). Marker ${MARKERS[$i]}: $OUT"
  fi
  assert_eq "$BEFORE" "$AFTER" "$eng: nothing inside the project moved when it was scanned as a root"
done
assert_eq 5 "$REFUSED" "all five measured project layouts (godot, blender, flp, song, ptx) are refused as scan roots"

# The refusal names the file, so a user knows why rather than being stonewalled.
if grep -q "closer.song" <<<"$("$SWEEP" "$W/roots/song" 2>&1)"; then
  pass "the refusal names the marker file, so the user can see why their folder was left alone"
else
  fail "the Studio One refusal did not name closer.song"
fi

# Studio One's Media/ is a name sweep itself uses. The project must be refused
# BEFORE any grouping, so the collision cannot arise.
SONG_OUT=$("$SWEEP" "$W/roots/song" 2>&1)
if grep -qE '^  Media' <<<"$SONG_OUT"; then
  fail "sweep tried to build a Media group inside a Studio One project that already has a Media/ folder: $SONG_OUT"
else
  pass "a project with its own Media/ folder is refused whole; no group is built to collide with it"
fi

# AppleDouble ._ twins are hidden and must never be filed as loose files. Test
# it on a plain folder that DOES group -- a refused project would prove nothing
# because it forms no groups at all. Six .wav that group, each shadowed by a
# ._ twin: the twins must be skipped, the reals grouped. Grace off, or the
# fresh files would be held back and nothing would group.
TW="$W/twins"; mkdir -p "$TW"
for c in 1 2 3 4 5 6; do : > "$TW/take_$c.wav"; : > "$TW/._take_$c.wav"; done
TWIN_GROUPED=$(SWEEP_GRACE_SECS=0 "$SWEEP" "$TW" --json 2>/dev/null | python3 -c "
import json,sys
d=json.load(sys.stdin)
print(any('._' in f for g in d['groups'] for f in g.get('members',[])))
")
assert_eq "False" "$TWIN_GROUPED" "no AppleDouble ._ twin is placed in a group (they are hidden)"
REAL_GROUPED=$(SWEEP_GRACE_SECS=0 "$SWEEP" "$TW" --json 2>/dev/null | python3 -c "
import json,sys
d=json.load(sys.stdin)
print(sum(g['count'] for g in d['groups']))
")
assert_eq 6 "$REAL_GROUPED" "the six real .wav files still group; only the twins are skipped"

# --- the case the guard exists for: a Downloads folder AROUND projects ------
#
# Split by marker kind, because the two halves are genuinely different and
# asserting one behaviour for both asserted something untrue.
#
# A project.godot marks its project's ROOT, so the project is exactly that
# folder and the Downloads around it is ordinary: sweep the invoices, step
# over the project.
DL="$W/Downloads"; mkdir -p "$DL"
build_godot "$DL/ad-astra"
for n in 1 2 3 4; do : > "$DL/invoice_$n.pdf"; done

INSIDE_BEFORE=$(find "$DL/ad-astra" -type f | wc -l | tr -d ' ')
assert_exit 0 "a Downloads folder containing a ROOT-marked project is sweepable" -- "$SWEEP" "$DL"

APPLY=$("$SWEEP" apply "$DL" --yes 2>&1); assert_eq 0 "$?" "apply succeeds on a folder holding a root-marked project"

INSIDE_AFTER=$(find "$DL/ad-astra" -type f 2>/dev/null | wc -l | tr -d ' ')
assert_eq "$INSIDE_BEFORE" "$INSIDE_AFTER" "not one file inside the Godot project moved during an apply of the folder around it"
[ -f "$DL/ad-astra/capture_t06.png" ] && pass "untouched: ad-astra/capture_t06.png" \
  || fail "a project's internal file moved: ad-astra/capture_t06.png"


# --- the same folder, WITH --depth ---------------------------------------
# The arm above runs at the default depth of 1, where sweep never descends
# into anything and the projects are safe for a reason that has nothing to do
# with the guard. That made it a weak test of exactly the thing it claims.
# With --depth, sweep does descend -- and before the nested-project guard it
# grouped a Godot project's captures into Images while project.godot sat
# listed as ungrouped. Applying that plan would have reorganised the project.
# A project.godot marks the project ROOT, so the project is exactly that
# folder: step over it and sort the invoices beside it. That is what this arm
# proves at depth.
#
# Document markers (.blend, .flp, .als) behave the same way today, and that is
# knowingly incomplete -- see issue #49. Nothing here asserts it is correct.
DL2="$W/Downloads2"; mkdir -p "$DL2"
build_godot "$DL2/ad-astra"
for n in 1 2 3 4; do : > "$DL2/invoice_$n.pdf"; done

D2_BEFORE=$(find "$DL2/ad-astra" -type f | wc -l | tr -d ' ')
D2_OUT=$("$SWEEP" "$DL2" --depth 4 2>&1)

# Nothing from inside the project may appear in any proposed group.
LEAKED=$("$SWEEP" "$DL2" --depth 4 --json 2>/dev/null | python3 -c "
import json,sys
d=json.load(sys.stdin)
mem=[f for g in d['groups'] for f in g.get('members',[])]
print(sum(1 for f in mem if '/ad-astra/' in f))
")
assert_eq 0 "$LEAKED" "at --depth 4, not one file from inside the Godot project appears in any proposed group"

if grep -qE '^  Documents' <<<"$D2_OUT"; then
  pass "the folder's own loose invoices still group at --depth 4; only the project is off limits"
else
  fail "the loose invoices formed no group at depth, so this arm proves nothing: $D2_OUT"
fi

if grep -qE "holds? a project file|hold project files" <<<"$D2_OUT"; then
  pass "the output says the project folder was left alone, rather than skipping it silently"
else
  fail "the project folder was stepped over with nothing in the output saying so: $D2_OUT"
fi

assert_exit 0 "apply at depth succeeds on a folder containing a root-marked project" -- "$SWEEP" apply "$DL2" --yes --depth 4
D2_AFTER=$(find "$DL2/ad-astra" -type f 2>/dev/null | wc -l | tr -d ' ')
assert_eq "$D2_BEFORE" "$D2_AFTER" "at --depth 4, an APPLY moved nothing inside the Godot project"




# --- a folder named like a project file is not a project ---------------------
NP="$W/NotAProject/ordinary.flp"; mkdir -p "$NP"
for n in 1 2 3; do : > "$NP/receipt_$n.pdf"; done
for n in 1 2 3; do : > "$W/NotAProject/loose_$n.pdf"; done
# Scan the PARENT. Scanning ordinary.flp itself never reaches the
# parent-marker detection, which is where the bug lived.
NP_OUT=$("$SWEEP" "$W/NotAProject" 2>&1)
if grep -qE '^  Documents' <<<"$NP_OUT"; then
  pass "a plain folder named ordinary.flp sweeps normally; over-refusal is a cost too"
else
  fail "a plain folder named ordinary.flp was refused or grouped nothing: $NP_OUT"
fi

# KNOWN GAP, issue #49: a project DOCUMENT (.blend, .flp, .als, .song, .ptx)
# references its assets upward, so stepping over the folder holding one still
# loses assets sitting beside it. The complete rule -- refuse any scan with a
# document marker below it -- was built and rejected: one .flp made a whole
# Downloads folder unsweepable. Nothing here asserts the incomplete behaviour
# is correct; the unit test pins it so changing it is deliberate.

# --- a bundle is stepped over, its neighbours are not ------------------------
FCP="$W/Movies"; mkdir -p "$FCP/MyMovie.fcpbundle/Original Media"
for c in 1 2 3; do : > "$FCP/MyMovie.fcpbundle/Original Media/clip$c.mov"; done
for n in 1 2 3; do : > "$FCP/invoice_$n.pdf"; done
FCP_OUT=$("$SWEEP" "$FCP" --depth 4 2>&1)

if grep -qE '^  Documents' <<<"$FCP_OUT"; then
  pass "invoices beside a Final Cut library still group; the library is not contagious"
else
  fail "a folder holding a .fcpbundle refused to sweep its own invoices: $FCP_OUT"
fi
FCP_MANIFEST_BEFORE=$(find "$FCP/MyMovie.fcpbundle" -type f | sort)
assert_exit 0 "apply beside a Final Cut library succeeds" -- "$SWEEP" apply "$FCP" --yes --depth 4
FCP_MANIFEST_AFTER=$(find "$FCP/MyMovie.fcpbundle" -type f 2>/dev/null | sort)
# Paths, not counts. Equal counts would also hold if a file moved WITHIN the
# bundle, which is exactly the damage this is meant to rule out.
if [ "$FCP_MANIFEST_BEFORE" = "$FCP_MANIFEST_AFTER" ]; then
  pass "every file inside the Final Cut library is at the same path it started at"
else
  fail "the Final Cut library's contents changed path:
$(diff <(echo "$FCP_MANIFEST_BEFORE") <(echo "$FCP_MANIFEST_AFTER") || true)"
fi

