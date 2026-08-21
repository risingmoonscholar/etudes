#!/usr/bin/env bash
# The script the recording follows. Committed so the clip is auditable: every
# command in the GIF is here, in order, and nothing else runs.
#
# This is the ILLUSTRATION. The evidence is demo/refusal/run.sh and the CI job
# that runs it -- that is what asserts the numbers. A recording proves what
# pixels a renderer emitted, which is why it is not the artifact anyone is
# asked to trust.
#
# Paced with sleeps so it reads at human speed. No output is edited; if a
# number here disagrees with run.sh, run.sh is right and this is stale.
set -uo pipefail
OLD="${OLD_SWEEP:?set OLD_SWEEP to a v0.4.0 binary}"
NEW="${NEW_SWEEP:?set NEW_SWEEP to a v0.5.0 binary}"
FX="${FIXTURE:?set FIXTURE to a Pixelorama checkout at 98cab8f}"
PRISTINE="${PRISTINE:?set PRISTINE to an untouched copy of the same checkout}"
# Before recording: run `godot --headless --quit` once inside $FX while it is
# still intact. Godot's first import of 1,248 files takes minutes; with the
# cache warm the recorded run takes under a second. The cache is Godot's own,
# and the errors shown are produced live during the recording.

# Margins and pacing are part of the point: a terminal recording that fills
# every column reads as noise, and this one is meant to be read once, calmly.
M="   "                     # left margin
# Narration is dim chrome; commands are the material; the accent is only ever
# the instrument -- lila's rule, and sweep's own output already obeys it by
# colouring nothing but what matters.
p()   { printf '\n%s\033[2m%s\033[0m\n' "$M" "$*"; sleep 0.9; }
run() { printf '%s\033[33m>\033[0m %s\n\n' "$M" "$*"; sleep 0.8
        eval "$@" 2>&1 | head -"${LINES_:-14}" | sed "s/^/$M/"; sleep 1.2; }

clear
p "a real godot project folder. 1,248 files. pinned commit 98cab8f."
p "moving files will break the project. lets see how sweep 0.4.0 handles this."
run "\"\$OLD\" \"\$FX\" --depth 4 | head -8"

p "sweep cli v0.4.0 gives us a plan with 44 groups. It wants to move 356 files"
p "out of the project. lets accept its suggestion and load the project to see"
p "what we did."
LINES_=2 run "\"\$OLD\" apply \"\$FX\" --only mirror --depth 4"

run "cd \"\$FX\" && godot --headless --quit 2>&1 | grep 'Resource file not found' | head -2"

p "the project can no longer find its own files. godot's words, not ours."
p "same folder, sweep v0.5.0:"
sleep 0.6
"$NEW" "$FX" --depth 4 2>&1 | fold -s -w 78 | head -5 | sed "s/^/$M/"
printf '%sexit \033[33m2\033[0m\n' "$M"
sleep 1.4
p "and the 18 files v0.4.0 moved? every move was journaled."
LINES_=2 run "\"\$OLD\" undo \"\$FX\""
sleep 1.2
