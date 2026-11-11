#!/usr/bin/env bash
# Terminal-only beats for the demo, one cast per beat. No narration in the
# terminal: the program's own output is the whole frame, and the story lives
# in Rohan's script in the video project, laid over these.
#
# Every command here also appears, asserted, in run.sh -- the recording stays
# true to the program because the same facts are checked in CI.
set -uo pipefail

# OLD_SWEEP is sweep 0.4.0, which takes its journal key from the login keychain
# and ignores ETUDE_JOURNAL_KEY. The apply and undo beats run it, so on a
# person's Mac this would read, and can create, a key in their real keychain.
# It runs only in CI. The check comes first: nothing runs before it.
if [ "${GITHUB_ACTIONS:-}" != "true" ]; then
  cat >&2 <<'REFUSE'
beats.sh: refusing to start. This script runs sweep 0.4.0, including apply
and undo. Sweep 0.4.0 uses the login keychain for its journal key, so on this
machine it would read, and could create, a key in your real keychain. For that
reason it runs only in CI (GITHUB_ACTIONS=true).
Nothing was run.
REFUSE
  exit 2
fi
OLD="${OLD_SWEEP:?}"; NEW="${NEW_SWEEP:?}"; FX="${FIXTURE:?}"
M="   "
run() { printf '%s\033[33m>\033[0m %s\n\n' "$M" "$1"; sleep 0.8
        eval "$2" 2>&1 | head -"${3:-14}" | sed "s/^/$M/"; sleep 2.2; }
beat="${1:?beat name}"
clear
case "$beat" in
  plan)    run 'sweep ~/pixelorama --depth 4' "\"$OLD\" \"$FX\" --depth 4" 12 ;;
  apply)   run 'sweep apply ~/pixelorama --only mirror --depth 4' "\"$OLD\" apply \"$FX\" --only mirror --depth 4" 3 ;;
  godot)   run "godot --headless --quit | grep 'not found'" "cd \"$FX\" && godot --headless --quit 2>&1 | grep 'Resource file not found'" 4 ;;
  refuse)  run 'sweep ~/pixelorama --depth 4' "\"$NEW\" \"$FX\" --depth 4 | fold -s -w 80; printf 'exit 2\n'" 6 ;;
  undo)    run 'sweep undo ~/pixelorama' "\"$OLD\" undo \"$FX\"" 3 ;;
  *) echo "beats: plan apply godot refuse undo" >&2; exit 2 ;;
esac
sleep 1.5
