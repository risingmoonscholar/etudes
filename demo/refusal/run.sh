#!/usr/bin/env bash
# The refusal demo, as evidence rather than pixels.
#
# The claim: pointed at a real Godot project, sweep v0.4.0 proposed
# reorganising files the project references by exact res:// path, and v0.5.0
# refuses the same folder, naming project.godot. A video of that is pixels;
# this script is the claim itself. It clones a pinned public fixture, builds
# both binaries from their tags, runs them, and asserts every fact the demo
# states. If any assertion stops being true, this exits non-zero and the
# demo is not made.
#
# The fixture is Pixelorama (MIT, github.com/orama-interactive/Pixelorama),
# pinned by commit. Public on purpose: an earlier draft of this demo used a
# private project of the author's, which a video would have named. Anyone
# can clone the same SHA and get the same numbers.
#
# Depth matters and is not hidden: at the default depth v0.4.0 proposes
# NOTHING on this fixture. The damage needs --depth 4, and the demo must
# show the flag. A recording that trimmed it would be staged.
set -euo pipefail

FIXTURE_REPO="https://github.com/orama-interactive/Pixelorama"
FIXTURE_SHA="98cab8f50ff79bb6575f9789e12e2532c70e5c60"
OLD_TAG="v0.4.0"
NEW_TAG="v0.5.0"
DEPTH=4

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
say()  { printf '%s\n' "$*"; }
fail() { printf 'FAIL %s\n' "$*"; exit 1; }

say "fixture: $FIXTURE_REPO @ ${FIXTURE_SHA:0:9}"
git clone -q "$FIXTURE_REPO" "$work/fx"
git -C "$work/fx" checkout -q "$FIXTURE_SHA"
rm -rf "$work/fx/.git"
# mtimes in the past, or the grace window holds everything and proves nothing
find "$work/fx" -type f -exec touch -t 202601010900 {} \;

digest() { (cd "$work/fx" && find . -type f | sort | xargs shasum -a 256 | shasum -a 256 | cut -d' ' -f1); }
BEFORE=$(digest)
say "fixture digest: $BEFORE"

say "building sweep $OLD_TAG and $NEW_TAG from their tags..."
for tag in "$OLD_TAG" "$NEW_TAG"; do
  git -C "$root" worktree add -q "$work/src-$tag" "$tag"
  (cd "$work/src-$tag" && cargo build --release -q -p sweep-cli)
  got=$("$work/src-$tag/target/release/sweep" --version | awk '{print $2}')
  [ "v$got" = "$tag" ] || fail "the $tag build reports $got"
  shasum -a 256 "$work/src-$tag/target/release/sweep" | awk '{print "  sweep '"$tag"' sha256 " $1}'
done
OLD="$work/src-$OLD_TAG/target/release/sweep"
NEW="$work/src-$NEW_TAG/target/release/sweep"

say ""
say "== $OLD_TAG on the fixture, --depth $DEPTH =="
"$OLD" "$work/fx" --depth $DEPTH --json > "$work/old.json" 2>/dev/null \
  || fail "$OLD_TAG did not produce a plan (exit $?)"

python3 - "$work/old.json" "$work/fx" <<'PY'
import json, os, re, sys
plan, root = sys.argv[1], os.path.realpath(sys.argv[2])
d = json.load(open(plan))
members = [os.path.realpath(f) for g in d["groups"] for f in g.get("members", [])]

refs = set()
for r, dirs, files in os.walk(root):
    for fn in files:
        if fn.endswith((".tscn", ".tres", ".godot", ".gd", ".cfg")):
            try:
                txt = open(os.path.join(r, fn), encoding="utf-8", errors="ignore").read()
            except OSError:
                continue
            refs.update(m.strip() for m in re.findall(r"res://([A-Za-z0-9_/. -]+)", txt))

hit = [m for m in members if os.path.relpath(m, root) in refs]
sidecar = [m for m in members if m.endswith(".uid") and os.path.relpath(m[:-4], root) in refs]

print(f"  groups proposed:               {len(d['groups'])}")
print(f"  files it would move:           {len(members)}")
print(f"  referenced by exact res://:    {len(hit)}")
print(f"  .uid sidecars of referenced:   {len(sidecar)}")
for m in hit[:5]:
    print(f"    would break: res://{os.path.relpath(m, root)}")

# The demo's load-bearing numbers. Not >=1 -- a threshold low enough to pass
# on noise would let the claim decay silently. These are the measured values;
# if the fixture or the binaries change them, a human re-reads the claim.
assert len(members) == 356, f"member count moved: {len(members)}"
assert len(hit) == 105, f"exact-path hits moved: {len(hit)}"
PY

say ""
say "== $NEW_TAG on the same folder, same flag =="
set +e
out=$("$NEW" "$work/fx" --depth $DEPTH 2>&1); code=$?
set -e
say "  exit: $code"
say "  $(head -1 <<<"$out")"
[ "$code" = "2" ] || fail "$NEW_TAG did not refuse (exit $code)"
grep -q "project.godot" <<<"$out" || fail "the refusal does not name project.godot: $out"

AFTER=$(digest)
[ "$BEFORE" = "$AFTER" ] || fail "the fixture changed during the runs"
say ""
say "fixture digest after both runs: unchanged"
say "ok   every fact the demo states was just measured"
