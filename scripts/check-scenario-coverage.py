#!/usr/bin/env python3
"""A scenario sweep is checkable: did it sample every declared boundary?

Reads stress/ranges.toml (the declared space) and a run's coverage manifest
(the coordinates actually sampled), and fails if any named boundary was never
hit. A boundary declared and never sampled is a claim the sweep did not honor.
Same discipline as check-coverage.py: a surface with no watcher fails.

    check-scenario-coverage.py <coverage.jsonl>   fail on any unsampled boundary
    check-scenario-coverage.py --declared          just print the declared space
"""
import json
import sys
import tomllib
from pathlib import Path

RANGES = Path(__file__).resolve().parent.parent / "stress" / "ranges.toml"


def declared():
    d = tomllib.load(open(RANGES, "rb"))
    out = {}
    for axis, spec in d["axis"].items():
        out[axis] = [str(b) for b in spec.get("boundaries", [])]
    return out


def main(argv):
    want = declared()
    if "--declared" in argv:
        for axis, bs in want.items():
            print(f"  {axis}: {', '.join(bs)}")
        return 0
    paths = [a for a in argv[1:] if not a.startswith("-")]
    if not paths:
        print("FAIL pass a coverage manifest, or --declared", file=sys.stderr)
        return 2
    sampled = {axis: set() for axis in want}
    for line in open(paths[0]):
        if not line.strip():
            continue
        coord = json.loads(line)
        for axis, val in coord.get("coordinate", {}).items():
            if axis in sampled:
                sampled[axis].add(str(val))
    missing = []
    for axis, bs in want.items():
        for b in bs:
            if b not in sampled[axis]:
                missing.append((axis, b))
    total = sum(len(b) for b in want.values())
    hit = total - len(missing)
    print(f"{paths[0]}: {hit}/{total} declared boundaries sampled")
    if missing:
        for axis, b in missing:
            print(f"  UNSAMPLED  {axis} = {b}  — declared a boundary, never hit")
        print("\n  A sweep that skips a declared boundary is not the sweep it claims to be.")
        return 1
    print("  ok   every declared boundary was sampled")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
