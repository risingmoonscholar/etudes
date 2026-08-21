#!/usr/bin/env bash
# Do the committed transcripts still come out of the current binaries?
#
# capture-demos.sh has always said, in its own header, that "a transcript that
# no longer matches the tool is a build failure, not a stale doc." Nothing
# enforced it. check-claims.sh verifies the page and the file agree with each
# other and that the declared version matches the manifest -- internal
# consistency, which two equally stale copies also have.
#
# So the guarantee was "captured once, with stated provenance" while reading as
# "reproduces from this code". Found by Codex while arguing about whether a
# demo GIF is checkable: it is not, and neither was this.
#
# What this does NOT check is the `commit` field. That records the HEAD the
# capture ran against, and it necessarily lags by at least the commit that
# records the capture. Requiring it to match would fail every time and get
# switched off, which is worse than not having it.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

committed="demo/transcripts.json"
if [ ! -f "$committed" ]; then
  echo "FAIL no $committed to check"
  exit 1
fi

# capture-demos.sh rewrites the demo/ files in place, so keep the originals and
# put them back whichever way this goes.
tmp="$(mktemp -d)"
cp "$committed" "$tmp/committed.json"
cp demo/index.html "$tmp/index.html"
restore() {
  cp "$tmp/committed.json" "$committed"
  cp "$tmp/index.html" demo/index.html
  rm -rf "$tmp"
}
trap restore EXIT

echo "regenerating transcripts from the current tree..."
if ! bash scripts/capture-demos.sh > "$tmp/capture.log" 2>&1; then
  echo "FAIL scripts/capture-demos.sh did not complete:"
  sed 's/^/    /' "$tmp/capture.log"
  exit 1
fi
cp "$committed" "$tmp/fresh.json"

python3 - "$tmp/committed.json" "$tmp/fresh.json" <<'PY'
import json, sys

old = json.load(open(sys.argv[1]))
new = json.load(open(sys.argv[2]))

def by_label(d):
    return {t["label"]: t for t in d["transcripts"]}

o, n = by_label(old), by_label(new)
problems = []

for label in sorted(set(o) | set(n)):
    if label not in o:
        problems.append(f"{label}: the tools now produce this transcript and none is committed")
        continue
    if label not in n:
        problems.append(f"{label}: committed, but the current tools no longer produce it")
        continue
    if o[label]["command"] != n[label]["command"]:
        problems.append(f"{label}: command changed\n    was: {o[label]['command']}\n    now: {n[label]['command']}")
    if o[label]["exit"] != n[label]["exit"]:
        problems.append(f"{label}: exit code {o[label]['exit']} -> {n[label]['exit']}")
    if o[label]["stdout"] != n[label]["stdout"]:
        ol, nl = o[label]["stdout"].splitlines(), n[label]["stdout"].splitlines()
        diff = []
        for i in range(max(len(ol), len(nl))):
            a = ol[i] if i < len(ol) else "<absent>"
            b = nl[i] if i < len(nl) else "<absent>"
            if a != b:
                diff.append(f"    line {i+1}\n      committed: {a}\n      now:       {b}")
                if len(diff) == 3:
                    break
        problems.append(f"{label}: stdout differs\n" + "\n".join(diff))

if old.get("version") != new.get("version"):
    problems.append(f"version {old.get('version')} -> {new.get('version')}")

if problems:
    print("FAIL the committed transcripts are not what these binaries print now.")
    print("     The site is showing output the code no longer produces.")
    print("     Re-run scripts/capture-demos.sh and commit the result.\n")
    for p in problems:
        print("  " + p)
    sys.exit(1)

print(f"ok   all {len(n)} transcripts still reproduce from this tree")
PY
