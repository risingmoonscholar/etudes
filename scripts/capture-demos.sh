#!/usr/bin/env bash
# Capture real terminal output from the real binaries, for the web demos.
#
# Nothing here is authored by hand. Every transcript in demo/transcripts.json is
# stdout from a binary built out of this tree, run against the synthetic fixture
# that `mkfx` generates. Regenerate and diff: a transcript that no longer matches
# the tool is a build failure, not a stale doc.
#
# The one edit made to captured output is a path substitution, declared in the
# `substitution_rule` field of the output file. The fixture lives in a temporary
# directory whose real name is machine-specific noise; the demos show `~/Desktop`
# instead, and the real path is deliberately not recorded. Nothing else is
# touched.
#
# No file of yours is read. The fixture is generated, used, and deleted.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

echo "building release binaries"
cargo build --release --quiet

bin="$root/target/release"
caps="$work/captures"
mkdir -p "$caps"

# The fixture is built inside a directory named Desktop so the captured output
# reads naturally. Only its parent path is substituted at render time.
home="$work/home"
mkdir -p "$home"
"$bin/mkfx" "$home/Desktop" >/dev/null

n=0
capture() {
  local label="$1" display="$2"; shift 2
  n=$((n + 1))
  local d="$caps/$(printf '%02d' "$n")-$label"
  mkdir -p "$d"
  printf '%s' "$display" > "$d/command"
  set +e
  "$@" > "$d/stdout" 2>&1
  printf '%s' "$?" > "$d/exit"
  set -e
}

capture sweep-plan    'sweep ~/Desktop'                "$bin/sweep" "$home/Desktop"
capture sweep-explain 'sweep ~/Desktop --explain'      "$bin/sweep" "$home/Desktop" --explain
capture sweep-json    'sweep ~/Desktop --json'         "$bin/sweep" "$home/Desktop" --json
capture unpack-help   'unpack help'                    "$bin/unpack" help

# stash mutates the tree, so it runs last against its own copy.
cp -R "$home/Desktop" "$home/Stashable"
capture stash-put     'stash ~/Desktop --for 3d --yes' "$bin/stash" "$home/Stashable" --for 3d --yes

mkdir -p demo
python3 - "$caps" "$home" "$work" demo/transcripts.json <<'PY'
import json, os, subprocess, sys

caps, home, work, out = sys.argv[1:5]
rev = subprocess.run(["git", "rev-parse", "--short", "HEAD"],
                     capture_output=True, text=True).stdout.strip() or "unknown"

# Declared, reversible substitutions. Longest first so nested paths win.
subs = [
    (os.path.realpath(f"{home}/Stashable"), "~/Desktop"),
    (os.path.realpath(f"{home}/Desktop"),   "~/Desktop"),
    (f"{home}/Stashable",                   "~/Desktop"),
    (f"{home}/Desktop",                     "~/Desktop"),
    (os.path.realpath(work),                "~"),
    (work,                                  "~"),
]

transcripts = []
for name in sorted(os.listdir(caps)):
    d = os.path.join(caps, name)
    text = open(os.path.join(d, "stdout")).read()
    for frm, to in subs:
        text = text.replace(frm, to)
    transcripts.append({
        "label": name.split("-", 1)[1],
        "command": open(os.path.join(d, "command")).read(),
        "exit": int(open(os.path.join(d, "exit")).read()),
        "stdout": text,
    })

json.dump({
    "generated_by": "scripts/capture-demos.sh",
    "commit": rev,
    "note": ("Real stdout from binaries built out of this tree, run against the "
             "synthetic mkfx fixture. Not hand-written."),
    "substitution_rule": ("The temporary directory the fixture was built in is "
                          "rendered as ~/Desktop. No other edit is made to captured "
                          "output. The real path is a mktemp name and is not recorded."),
    "transcripts": transcripts,
}, open(out, "w"), indent=2)
print(f"wrote {out}: {len(transcripts)} transcripts at {rev}")
PY
