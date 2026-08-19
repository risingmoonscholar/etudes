#!/usr/bin/env bash
# Real project layouts, from five engines people actually use.
#
# Every tree here is modelled on a real one. The Godot case is measured from
# an 18,724-file project on the author's machine: its .tscn files reference
# siblings as `res://scripts/main.gd`, absolute from the project root, and it
# keeps a `.import` sidecar beside every imported asset. The rest follow the
# layouts their vendors document -- Unreal's Content/Config/Saved/Intermediate,
# Premiere's Auto-Save and Preview Files, FL Studio's rendered audio beside the
# .flp, Blender's `//textures/` paths relative to the .blend.
#
# The point is not that these five are special. It is that a project folder is
# ALWAYS the same shape: one marker file, many file types, subdirectories, and
# references between them that a type-sorter would sever. A user who downloads
# an asset pack, clones a repo, or opens a client's project has one of these in
# their Downloads folder within a week.
#
# This scenario exists because the marker list was written from imagination and
# missed project.godot. A real Godot project on this machine was one
# `sweep` away from having its top-level PNGs and Markdown sorted away from the
# project.godot that indexes them. Blender was caught; Godot was not. Nothing
# in the suite would have noticed.
#
# Checks, for each engine:
#   - the project is refused as a scan root, naming its marker
#   - a folder CONTAINING projects is still sweepable, and the projects inside
#     it are not descended into
#   - no file anywhere inside any project moves
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir); trap 'rm -rf "$W"' EXIT

# --- Godot ---------------------------------------------------------------
# Measured shape: project.godot at the root, top-level captures with .import
# sidecars, scenes and scripts in subdirectories, a .godot/ cache.
build_godot() {  # $1 = dir
  local d="$1"
  mkdir -p "$d/scenes" "$d/scripts" "$d/assets/textures" "$d/.godot/imported"
  cat > "$d/project.godot" <<'EOF'
config_version=5
[application]
config/name="StressFixture"
run/main_scene="res://scenes/main.tscn"
EOF
  # A scene that references its siblings by res:// -- absolute from the root,
  # so ANY move inside the project breaks it.
  cat > "$d/scenes/main.tscn" <<'EOF'
[gd_scene format=4]
[ext_resource type="Script" path="res://scripts/main.gd" id="1"]
[ext_resource type="Texture2D" path="res://assets/textures/ground.png" id="2"]
EOF
  : > "$d/scenes/player.tscn"
  : > "$d/scripts/main.gd"
  : > "$d/scripts/player.gd"
  : > "$d/scripts/enemy.gd"
  # Imported assets carry a sidecar. Moving the asset without it orphans the
  # metadata; moving the sidecar without the asset orphans the reference.
  local t
  for t in ground rock sky; do
    : > "$d/assets/textures/$t.png"
    : > "$d/assets/textures/$t.png.import"
  done
  # Top-level captures, exactly as the real project has them.
  local c
  for c in 06 12 17; do
    : > "$d/capture_t$c.png"
    : > "$d/capture_t$c.png.import"
  done
  : > "$d/README.md"; : > "$d/CHANGELOG.md"; : > "$d/AGENTS.md"; : > "$d/CLAUDE.md"
  : > "$d/.godot/imported/cache.md5"
}

# --- Unreal --------------------------------------------------------------
# Content/ Config/ Source/ Saved/ Intermediate/, per Epic's documented layout.
build_unreal() {
  local d="$1"
  mkdir -p "$d/Content/Maps" "$d/Content/Meshes" "$d/Config" "$d/Source/Game" "$d/Saved/Logs" "$d/Intermediate/Build"
  : > "$d/StressFixture.uproject"
  : > "$d/Content/Maps/Level01.umap"
  : > "$d/Content/Meshes/rock.uasset"
  : > "$d/Content/Meshes/tree.uasset"
  : > "$d/Config/DefaultEngine.ini"
  : > "$d/Config/DefaultGame.ini"
  : > "$d/Source/Game/GameMode.cpp"
  : > "$d/Source/Game/GameMode.h"
  : > "$d/Saved/Logs/Game.log"
  : > "$d/Intermediate/Build/manifest.xml"
  : > "$d/README.md"
}

# --- Premiere ------------------------------------------------------------
# The .prproj beside its footage, plus the two folders Premiere writes itself.
build_premiere() {
  local d="$1"
  mkdir -p "$d/Footage" "$d/Audio" "$d/Adobe Premiere Pro Auto-Save" "$d/Adobe Premiere Pro Preview Files"
  : > "$d/ClientEdit.prproj"
  local c
  for c in 01 02 03 04; do : > "$d/Footage/A00$c.mp4"; done
  : > "$d/Audio/vo_take3.wav"; : > "$d/Audio/music_bed.wav"
  : > "$d/Adobe Premiere Pro Auto-Save/ClientEdit-1.prproj"
  : > "$d/notes.pdf"
}

# --- FL Studio -----------------------------------------------------------
build_flstudio() {
  local d="$1"
  mkdir -p "$d/Rendered" "$d/Samples"
  : > "$d/Track.flp"
  local c
  for c in 1 2 3; do : > "$d/Rendered/bounce_v$c.wav"; done
  for c in kick snare hat; do : > "$d/Samples/$c.wav"; done
  : > "$d/reference.mp3"
}

# --- Blender -------------------------------------------------------------
# // paths are relative to the .blend, so a texture that moves is a texture
# the file cannot find. Modelled on a real downloaded asset pack.
build_blender() {
  local d="$1"
  mkdir -p "$d/textures"
  : > "$d/Scene.blend"
  : > "$d/Scene.blend1"   # Blender's own backup, written beside the file
  local c
  for c in Color Normal Roughness Displacement; do : > "$d/textures/Ground_$c.png"; done
  : > "$d/render_final.png"
  : > "$d/notes.txt"
}

declare -a NAMES=(godot unreal premiere flstudio blender)
declare -a MARKERS=("project.godot" ".uproject" ".prproj" ".flp" ".blend")

# --- each project is refused as a root ------------------------------------
REFUSED=0
for i in "${!NAMES[@]}"; do
  eng="${NAMES[$i]}"
  d="$W/roots/$eng"
  "build_$eng" "$d"
  BEFORE=$(find "$d" -type f | wc -l | tr -d ' ')

  OUT=$("$SWEEP" "$d" 2>&1); CODE=$?
  AFTER=$(find "$d" -type f | wc -l | tr -d ' ')

  if [ "$CODE" = "2" ] && grep -qi "looks like a project" <<<"$OUT"; then
    REFUSED=$((REFUSED + 1))
  else
    fail "$eng: a project root was not refused (exit $CODE). Its ${MARKERS[$i]} is right there: $OUT"
  fi
  assert_eq "$BEFORE" "$AFTER" "$eng: nothing moved when the project was scanned as a root"
done
assert_eq 5 "$REFUSED" "all five real project layouts are refused as scan roots"

# --- the marker is named, so a user knows WHY ----------------------------
GODOT_OUT=$("$SWEEP" "$W/roots/godot" 2>&1)
if grep -q "project.godot" <<<"$GODOT_OUT"; then
  pass "the refusal names the file that made it a project, rather than refusing opaquely"
else
  fail "the refusal did not name project.godot, so a user cannot tell why their folder was refused: $GODOT_OUT"
fi

# --- a folder CONTAINING projects is still sweepable ---------------------
# The case this guard must not break: someone's Downloads folder with a
# cloned repo, a client project and an asset pack in it, plus loose files.
DL="$W/Downloads"
mkdir -p "$DL"
build_godot    "$DL/ad-astra"
build_premiere "$DL/ClientEdit"
build_blender  "$DL/Ground031_4K-PNG"
for n in 1 2 3 4; do : > "$DL/invoice_$n.pdf"; done
for n in 1 2 3; do : > "$DL/script_$n.sh"; done

INSIDE_BEFORE=$(find "$DL/ad-astra" "$DL/ClientEdit" "$DL/Ground031_4K-PNG" -type f | wc -l | tr -d ' ')
DL_OUT=$("$SWEEP" "$DL" 2>&1); DL_CODE=$?

assert_eq 0 "$DL_CODE" "a Downloads folder that merely CONTAINS projects is still sweepable"
if grep -qE '^  Documents' <<<"$DL_OUT"; then
  pass "the loose files in it still form groups"
else
  fail "the loose invoices formed no group, so this arm proves nothing about the guard: $DL_OUT"
fi

APPLY_OUT=$("$SWEEP" apply "$DL" --yes 2>&1); APPLY_CODE=$?
assert_eq 0 "$APPLY_CODE" "apply succeeds on a folder containing projects"

INSIDE_AFTER=$(find "$DL/ad-astra" "$DL/ClientEdit" "$DL/Ground031_4K-PNG" -type f 2>/dev/null | wc -l | tr -d ' ')
assert_eq "$INSIDE_BEFORE" "$INSIDE_AFTER" "not one file inside any of the three projects moved during an apply of the folder around them"

# The projects' own top-level files are the dangerous ones: a Godot project's
# capture_t06.png is a .png sitting where sweep can see it.
for f in "$DL/ad-astra/capture_t06.png" "$DL/ad-astra/project.godot" \
         "$DL/ad-astra/README.md" "$DL/Ground031_4K-PNG/render_final.png" \
         "$DL/ClientEdit/notes.pdf"; do
  if [ -f "$f" ]; then
    pass "still in place: ${f#$DL/}"
  else
    fail "a file inside a project moved during an apply of the folder around it: ${f#$DL/}"
  fi
done

# --- companion files travel together or not at all -----------------------
# Godot writes a .import beside every asset. If one moved without the other,
# the pair is broken even though both files still exist.
ORPHANS=0
for c in 06 12 17; do
  a="$DL/ad-astra/capture_t$c.png"; b="$DL/ad-astra/capture_t$c.png.import"
  if [ -f "$a" ] != [ -f "$b" ]; then ORPHANS=$((ORPHANS + 1)); fi
done
assert_eq 0 "$ORPHANS" "no asset was separated from its .import sidecar"
