#!/usr/bin/env python3
"""Every change to a tool was driven by an agent before a person sees it.

Two claims, both mechanical:

1. A change under crates/ that touches a tool's code arrives with a stress
   scenario that drives that tool's binary and was added or changed in the
   same set of changes. A scenario is an agent running the real binary on a
   folder it made and asserting on what happened. Test-only and docs-only
   changes are exempt; a change to etude-core counts for every tool.
2. The set of tools that have at least one scenario never shrinks. That set
   is stress/demos.txt, one binary variable per line ($SWEEP, $STASH,
   $UNPACK), committed; this check refuses if a listed tool has no scenario
   left, and adds nothing on its own.

    check-agent-demo.py                 judge the working tree against HEAD's merge-base with main
    check-agent-demo.py --base <rev>    judge against another base
    check-agent-demo.py --accept        rewrite stress/demos.txt to the tools that have scenarios now (never fewer)
"""
import re
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
SCEN = REPO / "stress" / "scenarios"
DEMOS = REPO / "stress" / "demos.txt"
TOOLS = {"sweep-cli": "$SWEEP", "stash-cli": "$STASH", "unpack-cli": "$UNPACK"}


def sh(*a):
    return subprocess.run(a, cwd=REPO, capture_output=True, text=True).stdout


def changed(base):
    files = set(sh("git", "diff", "--name-only", base).split())
    files |= set(sh("git", "ls-files", "--others", "--exclude-standard").split())
    return files


def scenario_tools():
    out = {}
    for p in sorted(SCEN.glob("*.sh")):
        text = p.read_text()
        out[p.name] = {v for v in TOOLS.values() if v in text}
    return out


def main(argv):
    base = "main"
    if "--base" in argv:
        base = argv[argv.index("--base") + 1]
    mb = sh("git", "merge-base", base, "HEAD").strip() or base
    scen = scenario_tools()
    have = {v for s in scen.values() for v in s}
    if "--accept" in argv:
        old = set(DEMOS.read_text().split()) if DEMOS.is_file() else set()
        DEMOS.write_text("\n".join(sorted(old | have)) + "\n")
        print(f"ok   demos.txt holds {len(old | have)} tool(s)")
        return 0
    bad = 0
    floor = set(DEMOS.read_text().split()) if DEMOS.is_file() else set()
    for v in sorted(floor - have):
        print(f"FAIL {v} is in stress/demos.txt and no scenario drives it any more"); bad += 1
    ch = changed(mb)
    touched = set()
    for f in ch:
        m = re.match(r"crates/([^/]+)/src/", f)
        if not m:
            continue
        crate = m.group(1)
        if crate == "etude-core":
            touched |= set(TOOLS.values())
        elif crate in TOOLS:
            touched.add(TOOLS[crate])
    demoed = {v for name, vs in scen.items() if f"stress/scenarios/{name}" in ch for v in vs}
    for v in sorted(touched):
        if v in demoed:
            print(f"ok   {v}: a scenario added or changed in this change drives it")
        else:
            print(f"FAIL {v}: its code changed and no scenario in this change drives it; an agent has not run the change"); bad += 1
    if not touched:
        print("ok   no tool code changed")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
