#!/usr/bin/env bash
# One command to start learning these tools. Nothing to set up.
#
#   scripts/lesson.sh
#
# It builds the binaries, puts them on PATH, generates a practice folder, and
# drops you into a shell where `sweep`, `stash`, `unpack` and `mkfx` just work.
# Type `reset` at any point to rebuild the practice folder from scratch, and
# `exit` to leave. Nothing outside the practice folder is touched.
#
# No file of yours is read. The practice folder is generated.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
REPO="$PWD"

PRACTICE="${ETUDES_PRACTICE:-/tmp/etudes-lesson}"

echo "building the tools"
cargo build --workspace --quiet

# Journals go somewhere disposable, so practising never writes to your real
# undo history and `forget` here cannot touch anything that matters.
STATE="$(mktemp -d "${TMPDIR:-/tmp}/etudes-lesson-state-XXXXXX")"

rm -rf "$PRACTICE"
"$REPO/target/debug/mkfx" "$PRACTICE" >/dev/null
echo "practice folder ready: $PRACTICE"

cat <<BANNER

  ─────────────────────────────────────────────
   sweep    look at a folder, and decide nothing
   stash    clear a folder now, bring it back later
   unpack   open any archive, safely
   mkfx     build a fresh practice folder

   reset    start over with a clean practice folder
   exit     leave

   Start with:  sweep $PRACTICE
  ─────────────────────────────────────────────

BANNER

RC="$(mktemp "${TMPDIR:-/tmp}/etudes-lesson-rc-XXXXXX")"
cat > "$RC" <<RCEOF
export BASH_SILENCE_DEPRECATION_WARNING=1
export PATH="$REPO/target/debug:\$PATH"
export XDG_STATE_HOME="$STATE"
export PRACTICE="$PRACTICE"
reset() {
  rm -rf "\$PRACTICE"
  "$REPO/target/debug/mkfx" "\$PRACTICE" >/dev/null
  rm -rf "$STATE"/etudes 2>/dev/null || true
  echo "practice folder rebuilt: \$PRACTICE"
}
PS1='etudes \w \$ '
cd "\$PRACTICE"
RCEOF

trap 'rm -rf "$STATE" "$RC"' EXIT
bash --rcfile "$RC" -i
echo "left the lesson. practice folder is still at $PRACTICE if you want it."
