#!/usr/bin/env python3
"""Is anything this project exposes watched by nothing, silently?

The failure that survives every other check is a surface nobody looks at. A
four-valued assessment can report UNKNOWN about a criterion it holds; it can
say nothing at all about a criterion nobody wrote, and that gap does not look
like a gap. It looks like everything passing.

Measured instance, this repo: scripts/check-issues.py reported it clean while
19 of 44 pull requests carried exactly what the checker existed to find,
because GitHub's schema excludes pull requests from issues(). No judgement
inside the checker could have found that. Only an inventory could.

So surfaces.toml lists what the project exposes and what watches each thing,
and this fails when a surface has neither a check nor a stated reason for
having none. "Unwatched" is a legitimate answer. Silence is not.

It also refuses to let a check exist without saying how it is proven failable,
because a check nobody has ever seen fail is indistinguishable from a check
that cannot.

    scripts/check-coverage.py
    scripts/check-coverage.py path/to/surfaces.toml

Exit 0 covered, 1 gaps, 2 could not check.
"""
import sys
import tomllib
from pathlib import Path


def main(argv):
    path = Path(argv[1] if len(argv) > 1 else "surfaces.toml")
    if not path.exists():
        print(f"FAIL no {path}. A project with no inventory of its own "
              "surfaces cannot know what is unwatched.", file=sys.stderr)
        return 2
    try:
        doc = tomllib.loads(path.read_text())
    except Exception as e:
        print(f"FAIL {path} does not parse: {e}", file=sys.stderr)
        return 2

    surfaces = doc.get("surface", {})
    checks = doc.get("check", {})
    if not surfaces:
        print(f"FAIL {path} lists no surfaces", file=sys.stderr)
        return 2

    bad = 0
    unwatched = []
    for name, s in sorted(surfaces.items()):
        watchers = s.get("watched_by", [])
        if not watchers:
            if not s.get("unwatched"):
                print(f"FAIL surface {name!r} has no check and no stated "
                      "reason for having none. Name a check, or say why "
                      "there is not one.")
                bad = 1
            else:
                unwatched.append((name, s["unwatched"]))
            continue
        for w in watchers:
            if w not in checks:
                print(f"FAIL surface {name!r} names check {w!r}, which is "
                      "not declared. A watcher nobody described is a watcher "
                      "nobody can audit.")
                bad = 1

    for name, c in sorted(checks.items()):
        if not c.get("guards"):
            print(f"FAIL check {name!r} does not say what it guards")
            bad = 1
        if not c.get("failable"):
            print(f"FAIL check {name!r} does not say how it is proven "
                  "failable. A check never seen to fail is indistinguishable "
                  "from one that cannot.")
            bad = 1

    watched = sum(1 for s in surfaces.values() if s.get("watched_by"))
    print(f"{watched} of {len(surfaces)} surfaces are watched by "
          f"{len(checks)} declared checks")
    for name, why in unwatched:
        print(f"  unwatched  {name}: {why}")
    if not bad:
        print("  ok   every surface is either watched or knowingly not")
    return bad


if __name__ == "__main__":
    sys.exit(main(sys.argv))
