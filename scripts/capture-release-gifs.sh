#!/usr/bin/env bash
# Capture the release candidate's bespoke terminal demos from the real binaries.
# Requires asciinema 3.x and agg. Fixtures and journal state are disposable.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
command -v asciinema >/dev/null || { echo "asciinema is required" >&2; exit 2; }
command -v agg >/dev/null || { echo "agg is required" >&2; exit 2; }
command -v zip >/dev/null || { echo "zip is required" >&2; exit 2; }

work="$(mktemp -d "${TMPDIR:-/tmp}/etudes-release-demos.XXXXXX")"
trap 'rm -rf "$work"' EXIT

export ETUDE_STATE_DIR="$work/state"
export ETUDE_JOURNAL_KEY="$(python3 -c 'import secrets; print(secrets.token_hex(32))')"
export ETUDES_DEMO_BIN="$root/target/release"
mkdir -p "$root/demo/recordings" "$root/demo/gifs"
cd "$root"

echo "building the three candidate binaries"
cargo build --release -p sweep-cli -p stash-cli -p unpack-cli

record() {
  local name="$1" title="$2" speed="$3"
  ETUDES_DEMO_WORK="$work/$name" asciinema rec --quiet --overwrite --title "$title" --window-size 100x14 \
    --command "bash demo/recordings/$name.sh" \
    "$root/demo/recordings/$name.cast"
  # asciinema succeeds even when the demo fails. A recording of an error is
  # not a demo: require the demo's own exit status to be 0 before rendering.
  python3 - "$root/demo/recordings/$name.cast" <<'PY'
import json, sys
events = [json.loads(line) for line in open(sys.argv[1], encoding="utf-8").read().splitlines()[1:] if line.strip()]
exits = [e for e in events if len(e) >= 3 and e[1] == "x"]
if not exits or str(exits[-1][2]) != "0":
    sys.exit(f"{sys.argv[1]}: the recorded demo did not exit 0 ({exits[-1][2] if exits else 'no exit event'}); refusing to render it")
PY
  agg --quiet --theme github-light --font-size 14 --cols 100 --rows 14 \
    --speed "$speed" --idle-time-limit 1.3 --last-frame-duration 4 \
    "$root/demo/recordings/$name.cast" "$root/demo/gifs/$name.gif"
  echo "wrote demo/gifs/$name.gif"
}

record sweep-progress "sweep 0.5.4 — progress and undo" 2.4
record stash-custody "stash 0.5.4 — clear now, restore later" 2.5
record unpack-safety "unpack 0.5.4 — accept and refuse" 1.2
