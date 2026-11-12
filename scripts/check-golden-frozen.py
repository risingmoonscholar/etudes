#!/usr/bin/env python3
"""The plan golden files change only when a commit says why.

crates/etude-core/tests/golden/ holds the plans every plan entry point
produced for a set of synthetic folders (crates/etude-core/tests/plan_golden.rs
compares against them). A change that only moves code must leave them byte
for byte the same. Regenerating them is easy, which is why this check exists:
a test that passes because its expected output was rewritten has tested
nothing.

So every golden file that differs between the base and HEAD must be named in
a commit trailer in base..HEAD:

    Golden-Change: crates/etude-core/tests/golden/<file>: <reason>

one trailer per file, with a reason. A deleted or renamed file counts as
changed.

One exception, for the first baseline: a golden file that does not exist at
the base is accepted without a trailer when it was added in the same commit
that adds crates/etude-core/tests/plan_golden.rs. That commit is the one that
creates the set, so there is nothing earlier it could differ from. Any golden
file added later needs a trailer like any other change.

Only commits are judged, because trailers live in commits. The working tree
is not read.

    check-golden-frozen.py --base <rev>    judge HEAD against <rev>

A base that cannot be read is a failure. A base equal to HEAD passes: nothing
changed, so nothing is unexplained.
"""
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
GOLDEN = "crates/etude-core/tests/golden/"
TEST = "crates/etude-core/tests/plan_golden.rs"


def git(*args):
    r = subprocess.run(["git", *args], cwd=REPO, capture_output=True, text=True)
    if r.returncode != 0:
        raise SystemExit(f"FAIL git {' '.join(args)} exited {r.returncode}: {r.stderr.strip()}")
    return r.stdout


def changed_golden(base):
    """{path: status} for golden files that differ between base and HEAD."""
    out = git("diff", "--no-renames", "--name-status", base, "HEAD", "--", GOLDEN)
    changed = {}
    for line in out.splitlines():
        status, _, path = line.partition("\t")
        changed[path] = status[:1]
    return changed


def trailers(base):
    """{path: reason} from every Golden-Change trailer in base..HEAD."""
    out = git("log", "--format=%(trailers:key=Golden-Change,valueonly,unfold)", f"{base}..HEAD")
    named = {}
    for line in out.splitlines():
        path, _, reason = line.strip().partition(":")
        if path:
            named[path.strip()] = reason.strip()
    return named


def baseline_files(base):
    """Golden files added by a commit in base..HEAD that also adds the test."""
    out = git("log", "--diff-filter=A", "--name-only", "--format=%x00%H", f"{base}..HEAD")
    accepted = set()
    for block in out.split("\0")[1:]:
        files = [line for line in block.splitlines()[1:] if line]
        if TEST in files:
            accepted |= {f for f in files if f.startswith(GOLDEN)}
    return accepted


def main(argv):
    if "--base" not in argv or argv.index("--base") + 1 >= len(argv):
        print("usage: check-golden-frozen.py --base <rev>")
        return 2
    base = argv[argv.index("--base") + 1]
    git("rev-parse", "--verify", "--quiet", base + "^{commit}")
    mb = git("merge-base", base, "HEAD").strip()
    changed = changed_golden(mb)
    if not changed:
        print("ok   no golden file changed")
        return 0
    named = trailers(mb)
    baseline = baseline_files(mb)
    bad = 0
    first = 0
    for path, status in sorted(changed.items()):
        reason = named.get(path)
        if reason:
            print(f"ok   {path}: {reason}")
        elif path in named:
            print(f"FAIL {path}: its Golden-Change trailer gives no reason"); bad += 1
        elif status == "A" and path in baseline:
            first += 1
        else:
            what = {"A": "added", "D": "deleted"}.get(status, "changed")
            print(f"FAIL {path}: {what} with no 'Golden-Change: {path}: <reason>' trailer in {base}..HEAD"); bad += 1
    if first:
        print(f"ok   {first} golden file(s) added with {TEST} (baseline)")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
