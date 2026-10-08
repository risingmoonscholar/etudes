#!/usr/bin/env bash
# Capture real terminal output from the real binaries, for the web demos.
#
# Nothing here is authored by hand. Every transcript in demo/transcripts.json is
# terminal output from binaries built out of this tree, run against the synthetic fixture
# that `mkfx` generates. A transcript that no longer matches the tool is a build
# failure, not a stale doc -- and scripts/check-transcripts-reproduce.sh is what
# makes that true. It said so here for weeks while nothing enforced it, which is
# the shape of defect this repo keeps finding in its own checkers.
#
# Fixture paths and volatile operation IDs are substituted as declared in substitution_rule.
# All other captured output remains unchanged.
#
# No file of yours is read. The fixture is generated, used, and deleted.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# The demonstration must not use the user's journals or depend on login-keychain
# access. Exercise the supported supplied-key path with disposable fixture state.
export ETUDE_STATE_DIR="$work/state"
export ETUDE_JOURNAL_KEY="$(python3 -c 'import secrets; print(secrets.token_hex(32))')"

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
  "$@" > "$d/output" 2>&1
  printf '%s' "$?" > "$d/exit"
  set -e
}

capture sweep-plan    'sweep ~/Desktop'                "$bin/sweep" "$home/Desktop"
capture sweep-explain 'sweep ~/Desktop --explain'      "$bin/sweep" "$home/Desktop" --explain
capture sweep-json    'sweep ~/Desktop --json'         "$bin/sweep" "$home/Desktop" --json
# The three helps are captured, not restated. The page used to carry a
# hand-written table of usage and flags beside these, and it drifted exactly
# the way a second copy always does: it omitted --since and --max-size, said
# `forget` destroys all journals and the key, and said stash moves EVERYTHING
# directly above a transcript showing a hidden item left behind. There is
# nothing to restate now.
capture sweep-help    'sweep help'                     "$bin/sweep" help
capture stash-help    'stash help'                     "$bin/stash" help
capture unpack-help   'unpack help'                    "$bin/unpack" help

# stash mutates the tree, so it runs last against its own copy.
cp -R "$home/Desktop" "$home/Stashable"
capture stash-put     'stash ~/Desktop --for 3d'       "$bin/stash" "$home/Stashable" --for 3d

mkdir -p demo
python3 - "$caps" "$home" "$work" demo/transcripts.json "$bin" <<'PY'
import json, os, re, subprocess, sys

caps, home, work, out = sys.argv[1:5]
# The version the captured binaries actually reported, asked of a binary
# rather than read from a manifest. The page used to hardcode "v0.3" in its
# template, so it went on saying 0.3 through the whole of 0.4 and nothing
# noticed -- a version is a claim like any other here.
# Per tool, asked of each binary. One ambient version stamped onto all three
# panels is how the site showed stash and unpack at a version they are frozen
# below -- the tools version separately now, and the page has to say what
# each binary says, not what the workspace happens to be.
versions = {}
for tool in ("sweep", "stash", "unpack"):
    versions[tool] = subprocess.run([os.path.join(sys.argv[5], tool), "--version"],
                                    capture_output=True, text=True).stdout.strip().split()[-1]
version = versions["sweep"]  # legacy field; per-tool is authoritative

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
    text = open(os.path.join(d, "output")).read()
    for frm, to in subs:
        text = text.replace(frm, to)
    label = name.split("-", 1)[1]
    if label.endswith("-json"):
        captured = json.loads(text)
        operation_id = json.dumps(captured["operation_id"])
        text = text.replace(operation_id, json.dumps("<per-invocation>"), 1)
    if label == "stash-put":
        match = re.search(r"  Operation id: ([0-9]+-[0-9]+-[0-9]+-[0-9a-f]+)\. Restore with: stash pop --id \1", text)
        if match is None:
            raise RuntimeError("stash-put did not emit its declared persistent operation id")
        text = text.replace(match.group(1), "<stash-operation>")
    transcripts.append({
        "label": label,
        "command": open(os.path.join(d, "command")).read(),
        "exit": int(open(os.path.join(d, "exit")).read()),
        "output": text,
    })

payload = {
    "generated_by": "scripts/capture-demos.sh",
    "commit": rev,
    "version": version,
    "versions": versions,
    "note": ("Real terminal output (stdout and stderr combined) from binaries "
             "built out of this tree, run against the "
             "synthetic mkfx fixture with temporary journal state and a disposable "
             "supplied key. Not hand-written; does not test login-keychain access."),
    "substitution_rule": ("The temporary directory the fixture was built in is "
                          "rendered as ~/Desktop. Only the volatile envelope operation_id "
                          "is additionally rendered as <per-invocation>; the stash-put persistent "
                          "operation id in its operation/restore line is rendered as <stash-operation>; all other "
                          "output bytes are retained. The real path is a mktemp name "
                          "and is not recorded."),
    "transcripts": transcripts,
}
json.dump(payload, open(out, "w"), indent=2)
print(f"wrote {out}: {len(transcripts)} transcripts at {rev}")

# Same payload, written a second place: an inline <script> in demo/index.html,
# between two HTML comment markers, so a double-clicked file:// copy of the
# page works without a server. fetch() stays the primary path in the page's
# own JS; this is only the fallback it reaches for when fetch() is blocked.
# "</" is escaped inside the JSON so no captured output can accidentally close
# the surrounding <script> tag early.
index_path = os.path.join(os.path.dirname(out), "index.html")
start, end = "<!-- TRANSCRIPTS_INLINE_START -->", "<!-- TRANSCRIPTS_INLINE_END -->"
html = open(index_path).read()
if start in html and end in html:
    before, rest = html.split(start, 1)
    _, after = rest.split(end, 1)
    inline_json = json.dumps(payload).replace("</", "<\\/")
    block = (
        f"{start}\n"
        "<!-- Written by scripts/capture-demos.sh alongside demo/transcripts.json, same\n"
        "     data, same step, so a double-clicked file:// copy works without a server.\n"
        "     fetch() is still tried first below; this is the fallback, not the source. -->\n"
        f'<script type="application/json" id="transcripts-inline">{inline_json}</script>\n'
        f"{end}"
    )
    open(index_path, "w").write(before + block + after)
    print(f"embedded the same {len(transcripts)} transcripts inline in {index_path}")
else:
    print(f"WARNING: {index_path} has no TRANSCRIPTS_INLINE markers; inline fallback not written")
PY
