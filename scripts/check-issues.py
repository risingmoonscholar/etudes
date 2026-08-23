#!/usr/bin/env python3
"""Are the public issues written for the people who use the tool?

A public tracker is a product surface. Its readers are users deciding whether
a tool is trustworthy. What belongs there is a defect: what breaks, how to
reproduce it, what it costs. What does not belong is the working session that
produced it -- reviewer or model names, quoted decisions, open questions,
candidate designs, or a map of where the tool is weak before a fix exists.

All four kinds got published in this repo. The worst enumerated a tool's
undefended attack classes, in public, as a plan, before any of it was fixed.

EDIT HISTORY IS CHECKED, and that is the point. GitHub keeps every prior
version of an issue body and serves it to anyone through the GraphQL API:

    userContentEdits(first: 20) { diff }

So editing an issue to remove something does not remove it -- it publishes a
second version and keeps the first. The only removal is deletion. This checker
reads the history precisely because a clean-looking issue can have a dirty one
underneath it, which is exactly what was found here.

    check-issues.py                 # the current repo
    check-issues.py owner/name ...  # any number of repos

Exit 0 clean, 1 findings, 2 could not check. A check that cannot run is never
a pass.
"""
import json
import re
import subprocess
import sys

RULES = [
    ("reviewer-named", "blocker",
     r"\b(codex|claude|gpt-?\d|chatgpt|copilot|cursor|gemini)\b",
     "names the tool or model that did the work"),
    ("person-named", "blocker",
     r"(\bthe operator\b|\boperator'?s?\s+(refinement|framing|call|decision"
     r"|words|wants|said|asked|chose)|\brohan\b|maintainer's framing)",
     "names the maintainer"),
    ("quoted-decision", "blocker",
     r"(verbatim|'s framing:|his words|her words|their words|said:)",
     "quotes a private conversation"),
    ("undecided", "blocker",
     r"(open questions?|for discussion,? not decided|candidate surface"
     r"|not yet decided|do not build without|still held for)",
     "publishes an unresolved decision"),
    ("weakness-roadmap", "blocker",
     r"(attack surface|surface to scenario|not yet defended|undefended"
     r"|classes to scenario)",
     "maps weaknesses that are not yet fixed"),
    # Narrowed deliberately. A bare "I" is usually a REPRODUCTION -- "I ended
    # up with four ram disks" is exactly what a bug report should say -- and a
    # checker that flags those gets ignored, which is worse than not having
    # one. Only process narration is the smell: how the work was done, not
    # what the tool did.
    ("process-narration", "warn",
     r"\b(I (wrote|built|decided|reviewed|refactored|shipped|was wrong)"
     r"|my (fix|change|patch|review|reasoning)"
     r"|I'll (fix|file|open|build))\b",
     "narrates how the work was done rather than what breaks"),
    ("internal-crossref", "warn",
     r"^(split out of|follow-?up to)\b",
     "opens with internal bookkeeping"),
]

QUERY = """
query($owner:String!, $name:String!, $cursor:String) {
  repository(owner:$owner, name:$name) {
    issues(first:50, after:$cursor, states:[OPEN,CLOSED]) {
      pageInfo { hasNextPage endCursor }
      nodes {
        number state title body
        userContentEdits(first:20) { nodes { diff } }
        comments(first:50) {
          nodes { body userContentEdits(first:20) { nodes { diff } } }
        }
      }
    }
  }
}
"""


def gh(args, **kw):
    return subprocess.run(["gh"] + args, capture_output=True, text=True, **kw)


def fetch(repo):
    owner, name = repo.split("/", 1)
    out, cursor = [], None
    while True:
        cmd = ["api", "graphql", "-f", f"query={QUERY}",
               "-F", f"owner={owner}", "-F", f"name={name}"]
        if cursor:
            cmd += ["-F", f"cursor={cursor}"]
        r = gh(cmd)
        if r.returncode != 0:
            raise RuntimeError(r.stderr.strip() or "graphql failed")
        page = json.loads(r.stdout)["data"]["repository"]["issues"]
        out.extend(page["nodes"])
        if not page["pageInfo"]["hasNextPage"]:
            return out
        cursor = page["pageInfo"]["endCursor"]


# Quoted tool output and fenced code are the ISSUE'S EVIDENCE, not its prose.
# A tool that prints "I will not guess which files are yours" would otherwise
# trip the author-voice rule with its own words.
FENCE = re.compile(r"```.*?```", re.S)
QUOTED = re.compile(r"^\s*>.*$", re.M)
INLINE = re.compile(r"`[^`]*`")


def prose_only(text):
    for pat in (FENCE, QUOTED, INLINE):
        text = pat.sub(" ", text)
    return text


def scan(repo):
    issues = fetch(repo)
    findings = []
    for i in issues:
        # Current text, and every version that came before it. A prior version
        # is as public as the current one.
        sources = [("current", f"{i['title']}\n{i.get('body') or ''}")]
        for n, e in enumerate(i["userContentEdits"]["nodes"]):
            if e.get("diff"):
                sources.append((f"edit -{n + 1}", e["diff"]))
        # Comments were the hole this checker had on its first run: the worst
        # thing published in this repo -- an acceptance bar quoting a private
        # decision -- was a COMMENT, and a body-only check called it clean.
        for c, com in enumerate(i["comments"]["nodes"]):
            sources.append((f"comment {c + 1}", com.get("body") or ""))
            for n, e in enumerate(com["userContentEdits"]["nodes"]):
                if e.get("diff"):
                    sources.append((f"comment {c + 1} edit -{n + 1}", e["diff"]))
        for where, text in sources:
            body = prose_only(text)
            for rid, sev, pat, why in RULES:
                m = re.search(pat, body, re.I | re.M)
                if m:
                    findings.append((sev, i["number"], i["state"].lower(),
                                     where, why, rid, m.group(0).strip()[:40]))
    return len(issues), findings


def main(argv):
    repos = argv[1:]
    if not repos:
        r = gh(["repo", "view", "--json", "nameWithOwner", "-q", ".nameWithOwner"])
        if r.returncode != 0:
            print("FAIL cannot determine the repo; pass owner/name", file=sys.stderr)
            return 2
        repos = [r.stdout.strip()]

    worst = 0
    for repo in repos:
        try:
            count, findings = scan(repo)
        except Exception as e:  # auth, network, bad name
            print(f"FAIL {repo}: could not check ({e})", file=sys.stderr)
            worst = max(worst, 2)
            continue
        print(f"{repo}: checked {count} issues, including edit history")
        if not findings:
            print("  ok   every issue reads as a defect report")
            continue
        for sev, num, state, where, why, rid, hit in sorted(findings):
            label = "BLOCKER" if sev == "blocker" else "warn"
            print(f"  {label:8} #{num} [{state}, {where}] {why}")
            print(f"           {hit!r} ({rid})")
        if any(f[0] == "blocker" for f in findings):
            print("\n  Editing does not remove these: GitHub keeps and serves the")
            print("  old version. Delete the issue and re-file it clean.")
            worst = max(worst, 1)
    return worst


SAMPLES = {
    "reviewer-named": "Found by Codex reviewing the guard.",
    "person-named": "Operator refinement, and it is the acceptance bar.",
    "quoted-decision": "His words: this over-refuses badly.",
    "undecided": "Open questions for the maintainer before building.",
    "weakness-roadmap": "The attack surface to scenario, roughly by nastiness.",
    "process-narration": "I decided to split this out after my review.",
    "internal-crossref": "Split out of #45, because it needs its own issue.",
}

CLEAN = [
    "Killing a run leaves ram disks attached. I ended up with four of them.",
    "```\nsweep: refused: I will not guess which files are yours\n```",
    "> the tool printed: I cannot judge this file",
    "Sweep sorts the textures into Images/ and the project cannot find them.",
    "The ? operator propagates the error to the caller.",
]


def selftest():
    """Every rule fires on a known-bad line; no rule fires on a clean one.

    A checker that has only ever passed is the thing that got this repo into
    trouble, so it proves itself before it judges anything.
    """
    bad = 0
    for rid, sev, pat, _why in RULES:
        sample = SAMPLES.get(rid)
        if sample is None:
            print(f"FAIL self-test: no sample for rule {rid}", file=sys.stderr)
            bad = 1
            continue
        if not re.search(pat, prose_only(sample), re.I | re.M):
            print(f"FAIL self-test: rule {rid} did not fire on {sample!r}",
                  file=sys.stderr)
            bad = 1
    for line in CLEAN:
        for rid, sev, pat, _why in RULES:
            m = re.search(pat, prose_only(line), re.I | re.M)
            if m:
                print(f"FAIL self-test: rule {rid} fired on clean text "
                      f"{line!r} (matched {m.group(0)!r})", file=sys.stderr)
                bad = 1
    return bad


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.exit(selftest())
    if selftest():
        print("FAIL the checker failed its own self-test; not judging anything",
              file=sys.stderr)
        sys.exit(2)
    sys.exit(main([a for a in sys.argv if a != "--self-test"]))
