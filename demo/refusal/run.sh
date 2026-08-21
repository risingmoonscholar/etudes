#!/usr/bin/env bash
# The refusal demo, as evidence. No claim here is printed without being checked.
#
# Codex set the standard and then failed the first version of this script
# against it. Its findings, all acted on below: only two of the six numbers
# were asserted and the rest merely printed; the Python gates used `assert`,
# which vanishes under python3 -O; the fixture digest, the binary provenance
# and the licence were printed but never verified; the header claimed the
# default depth does nothing without ever running it; the refusal was matched
# with an unanchored grep whose "." was a wildcard; and the closing line said
# "every fact the demo states was just measured" when four of them had not
# been. That line was the same defect this repo keeps finding: a summary
# claiming more than the thing under it did.
#
# The largest change is what the demo demonstrates. The first version inferred
# damage: it showed v0.4.0 PROPOSING to move files the project references, and
# left the reader to assume that would break it. Codex was right that this
# proves less than it sounds. So this applies the plan to a throwaway copy and
# counts load targets that are no longer at their res:// path afterwards.
#
# Two facts that came out of doing so, both stated because they qualify the
# claim:
#
#   * v0.4.0 CANNOT apply the whole plan to this fixture. It refuses with a
#     destination collision -- two files that would land on the same name.
#     A different guard stops it, not the project guard, so "v0.4.0 would have
#     destroyed this project" is not a claim this fixture supports.
#   * One group applies cleanly, and moves six referenced files out from under
#     the scenes that load them. That is the honest demonstration: real
#     breakage, bounded, on a public fixture anyone can re-run.
#
# Depth is not hidden either. At the default depth v0.4.0 proposes nothing
# here, and that is asserted rather than asserted-in-a-comment.
set -euo pipefail

FIXTURE_REPO="https://github.com/orama-interactive/Pixelorama"
FIXTURE_SHA="98cab8f50ff79bb6575f9789e12e2532c70e5c60"
OLD_TAG="v0.4.0"; OLD_COMMIT="138f8b5a48b497ffd872e71365a8aa1ad79abf7a"
NEW_TAG="v0.5.0"; NEW_COMMIT="f90bdbfbd0f54bf516acaef5fde69381df014650"
DEPTH=4
GROUP="mirror"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
say()  { printf '%s\n' "$*"; }
fail() { printf 'FAIL %s\n' "$*" >&2; exit 1; }
need() { [ "$2" = "$3" ] || fail "$1: expected $3, measured $2"; }

# Null-delimited: a filename with a space or newline must not silently split
# the digest into agreement with something else.
digest() { (cd "$1" && find . \( -type f -o -type l \) -print0 | sort -z \
           | xargs -0 shasum -a 256 | shasum -a 256 | cut -d' ' -f1); }

say "== fixture =="
git clone -q "$FIXTURE_REPO" "$work/fx"
git -C "$work/fx" checkout -q "$FIXTURE_SHA" 2>/dev/null \
  || fail "the fixture repo has no commit $FIXTURE_SHA. It was force-pushed, or the pin is wrong; either way this demo cannot be re-run as recorded"
got_sha=$(git -C "$work/fx" rev-parse HEAD)
need "fixture commit" "$got_sha" "$FIXTURE_SHA"
rm -rf "$work/fx/.git"
grep -q "MIT License" "$work/fx/LICENSE" || fail "the fixture's LICENSE at this commit is not MIT"
say "  $FIXTURE_REPO @ ${FIXTURE_SHA:0:9}, MIT, verified at this commit"
# Without this the grace window holds every file and the demo proves nothing.
find "$work/fx" -type f -exec touch -t 202601010900 {} \;
BEFORE=$(digest "$work/fx")
say "  digest: $BEFORE"

say ""
say "== binaries, pinned by commit and not only by tag =="
for pair in "$OLD_TAG:$OLD_COMMIT" "$NEW_TAG:$NEW_COMMIT"; do
  tag="${pair%%:*}"; want="${pair##*:}"
  git -C "$root" rev-parse -q --verify "refs/tags/$tag" >/dev/null \
    || git -C "$root" fetch -q --depth 1 origin "refs/tags/$tag:refs/tags/$tag" \
    || fail "tag $tag exists neither locally nor on origin"
  # A tag can be moved. The commit it must resolve to is pinned here, so a
  # moved tag is a failure rather than a silently different demo.
  at=$(git -C "$root" rev-list -n1 "$tag")
  need "$tag resolves to" "$at" "$want"
  git -C "$root" worktree add -q "$work/src-$tag" "$tag"
  (cd "$work/src-$tag" && cargo build --release -q -p sweep-cli)
  v=$("$work/src-$tag/target/release/sweep" --version | awk '{print $2}')
  need "$tag reports version" "v$v" "$tag"
  say "  $tag = ${want:0:9}  sha256 $(shasum -a 256 "$work/src-$tag/target/release/sweep" | cut -c1-16)..."
done
OLD="$work/src-$OLD_TAG/target/release/sweep"
NEW="$work/src-$NEW_TAG/target/release/sweep"

say ""
say "== $OLD_TAG at the DEFAULT depth =="
# Exit 1 is "nothing to do", which is exactly what is expected here, so a
# bare || fail would reject the very outcome being asserted.
set +e
"$OLD" "$work/fx" --json > "$work/default.json" 2>"$work/default.err"; dcode=$?
set -e
case "$dcode" in 0|1) ;; *) fail "default-depth scan exited $dcode: $(cat "$work/default.err")";; esac
dg=$(python3 -c "import json,sys; print(len(json.load(open(sys.argv[1]))['groups']))" "$work/default.json")
need "groups at default depth" "$dg" "0"
say "  proposes 0 groups. The damage in this demo needs --depth $DEPTH, and"
say "  a recording that hid the flag would be staged."

say ""
say "== $OLD_TAG at --depth $DEPTH =="
set +e
"$OLD" "$work/fx" --depth $DEPTH --json > "$work/old.json" 2>"$work/old.err"; scode=$?
set -e
need "scan exit at --depth $DEPTH" "$scode" "0"
[ -s "$work/old.err" ] && fail "$OLD_TAG wrote to stderr, which this demo would have hidden: $(cat "$work/old.err")"

python3 - "$work/old.json" "$work/fx" "$work/sites.json" <<'PY' || exit 1
import json, os, re, sys
plan, root, sites_out = sys.argv[1], os.path.realpath(sys.argv[2]), sys.argv[3]
d = json.load(open(plan))
members = [os.path.realpath(f) for g in d["groups"] for f in g.get("members", [])]

# LOAD SITES, not any text that happens to contain res://. A res:// in a
# comment is not a thing Godot resolves; these three forms are.
pats = [re.compile(r'\[ext_resource[^\]]*\bpath="res://([^"]+)"'),
        re.compile(r'\bpreload\(\s*"res://([^"]+)"'),
        re.compile(r'\bload\(\s*"res://([^"]+)"')]
sites = {}
for r, _, files in os.walk(root):
    for fn in files:
        if not fn.endswith(('.tscn', '.tres', '.gd', '.godot')):
            continue
        p = os.path.join(r, fn)
        try:
            txt = open(p, encoding='utf-8', errors='ignore').read()
        except OSError:
            continue
        for pat in pats:
            for m in pat.finditer(txt):
                sites.setdefault(m.group(1), []).append(os.path.relpath(p, root))

present = {t: v for t, v in sites.items() if os.path.exists(os.path.join(root, t))}
json.dump({"present": {k: sorted(set(v)) for k, v in present.items()}}, open(sites_out, "w"))

groups, files = len(d["groups"]), len(members)
print(f"  groups proposed:            {groups}")
print(f"  files it would move:        {files}")
print(f"  load targets in the project:{len(present):4}")

# Explicit failure, not `assert`: python3 -O strips assert statements, which
# would have left both of the first version's only two gates vacuous.
bad = []
if groups != 44: bad.append(f"groups: expected 44, measured {groups}")
if files != 356: bad.append(f"planned files: expected 356, measured {files}")
if len(present) != 375: bad.append(f"load targets present: expected 375, measured {len(present)}")
if bad:
    for b in bad: print("FAIL " + b, file=sys.stderr)
    sys.exit(1)
PY

say ""
say "== end to end: apply one group with $OLD_TAG, on a copy =="
cp -R "$work/fx" "$work/applied"
set +e
apply_out=$("$OLD" apply "$work/applied" --only "$GROUP" --depth $DEPTH 2>&1); apply_code=$?
set -e
need "apply exit" "$apply_code" "0"
moved=$(sed -n 's/^Moved \([0-9]*\) files\./\1/p' <<<"$apply_out")
need "files moved" "$moved" "18"
say "  applied --only $GROUP: moved $moved files, exit $apply_code"
# Whole-plan apply is refused for an unrelated reason, and saying so is the
# difference between a demonstration and a claim.
set +e
whole=$("$OLD" apply "$work/fx" --yes --depth $DEPTH 2>&1); whole_code=$?
set -e
need "whole-plan apply exit" "$whole_code" "2"
grep -qF "would move to the same destination" <<<"$whole" \
  || fail "the whole-plan refusal is no longer the collision guard: $whole"
say "  (the WHOLE plan refuses, exit 2, on a destination collision -- a"
say "   different guard. This demo does not claim v0.4.0 would have applied it.)"

python3 - "$work/sites.json" "$work/fx" "$work/applied" "$work/broken.json" <<'PY' || exit 1
import json, os, sys
sites = json.load(open(sys.argv[1]))["present"]
src, dst, out = os.path.realpath(sys.argv[2]), os.path.realpath(sys.argv[3]), sys.argv[4]
broken = {t: refs for t, refs in sites.items() if not os.path.exists(os.path.join(dst, t))}
json.dump(broken, open(out, "w"), indent=1, sort_keys=True)
print(f"  load targets no longer at their res:// path: {len(broken)}")
for t in sorted(broken)[:4]:
    print(f"    res://{t}")
    print(f"      loaded by {broken[t][0]}")
if len(broken) != 6:
    print(f"FAIL broken load targets: expected 6, measured {len(broken)}", file=sys.stderr)
    sys.exit(1)
PY

say ""
say "== $NEW_TAG on the same folder, same flag =="
set +e
out=$("$NEW" "$work/fx" --depth $DEPTH 2>&1); code=$?
set -e
need "exit" "$code" "2"
# Fixed-string, and the sentence rather than a token: "project.godot" as a
# regex matches "projectXgodot", and could appear anywhere in any output.
grep -qF "looks like a project (project.godot is in it)" <<<"$out" \
  || fail "the refusal no longer names project.godot in its own sentence: $out"
say "  exit $code"
say "  $(head -1 <<<"$out")"

AFTER=$(digest "$work/fx")
need "fixture digest after both scans" "$AFTER" "$BEFORE"

cat > "$work/evidence.json" <<JSON
{
  "runner_commit": "$(git -C "$root" rev-parse HEAD)",
  "fixture": {"repo": "$FIXTURE_REPO", "commit": "$FIXTURE_SHA", "licence": "MIT", "digest": "$BEFORE"},
  "binaries": {"$OLD_TAG": "$OLD_COMMIT", "$NEW_TAG": "$NEW_COMMIT"},
  "measured": {
    "groups_at_default_depth": 0, "groups_at_depth_$DEPTH": 44,
    "files_planned": 356, "load_targets_present": 375,
    "files_moved_by_one_group": 18, "load_targets_broken": 6,
    "new_binary_exit": 2, "whole_plan_apply_exit": 2
  },
  "host": {"os": "$(uname -sr)", "rustc": "$(rustc --version)"}
}
JSON
cp "$work/old.json" "$work/broken.json" "$work/evidence.json" "${EVIDENCE_DIR:-$work}/" 2>/dev/null || true
[ -n "${EVIDENCE_DIR:-}" ] && say "" && say "evidence written to $EVIDENCE_DIR"

say ""
say "ok   nine measurements, nine assertions, all of them just checked"
