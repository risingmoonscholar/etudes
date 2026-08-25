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

# ATTRIBUTION IS NOT A LEAK. The site says so in as many words: "Night Watch
# is the agent that runs these against this repo and writes up what it finds.
# The open issues are its output. Each carries its own attribution." Naming
# the model that found a defect is table stakes and advertised here, and an
# earlier version of this checker treated it as a blocker -- which cost three
# perfectly good defect reports before the rule was corrected.
#
# What actually does not belong is the working session: an unresolved
# decision, a quoted private conversation, or a map of where the tool is weak
# before a fix exists.
RULES = [
    # "The operator" is the tell. It is an internal role-word: nobody writes it
    # in a public artifact unless they are narrating a private working
    # relationship -- "the operator chose X over the reviewer's
    # recommendation" is a session transcript wearing a PR body. The
    # maintainer's own NAME is not a leak in his own repository, and flagging
    # it was noise.
    ("role-narration", "blocker",
     r"(\bthe operator\b|\boperator'?s?\s+(refinement|framing|call|decision"
     r"|words|wants|said|asked|chose|diagnosed|overruled)"
     r"|\bthe maintainer (chose|wants|decided|asked)\b)",
     "narrates a private decision between the author and the maintainer", "prose"),
    # Bare "verbatim" is ordinary technical prose -- "undo's verbatim output",
    # "renders them verbatim" -- and flagging it produced five false positives
    # against one true one. Speech attribution is the real signal.
    ("quoted-decision", "blocker",
     r"('s framing:|his words|her words|their words|\bsaid:|\btold me\b"
     r"|quoted verbatim|verbatim quote)",
     "quotes a private conversation", "prose"),
    ("undecided", "blocker",
     r"(open questions?|for discussion,? not decided|candidate surface"
     r"|not yet decided|do not build without|still held for)",
     "publishes an unresolved decision", "prose"),
    ("weakness-roadmap", "blocker",
     r"(attack surface|surface to scenario|not yet defended|undefended"
     r"|classes to scenario)",
     "maps weaknesses that are not yet fixed", "prose"),
    # Narrowed deliberately. A bare "I" is usually a REPRODUCTION -- "I ended
    # up with four ram disks" is exactly what a bug report should say -- and a
    # checker that flags those gets ignored, which is worse than not having
    # one. Only process narration is the smell: how the work was done, not
    # what the tool did.
    ("process-narration", "warn",
     r"\b(I (wrote|built|decided|reviewed|refactored|shipped|was wrong)"
     r"|my (fix|change|patch|review|reasoning)"
     r"|I'll (fix|file|open|build))\b",
     "narrates how the work was done rather than what breaks", "prose"),
    ("internal-crossref", "warn",
     r"^(split out of|follow-?up to)\b",
     "opens with internal bookkeeping", "prose"),
    # ── Voice rules, added after the 2026-08-24 drafts. The six above catch
    # leaked deliberation; these catch the other recurring defect, a design
    # document wearing an issue's clothes. Scope matters here: prose_only()
    # strips inline code, and every citation in the motivating drafts was
    # written as `main.rs:497` -- a prose-scoped citation rule passes the
    # exact text it exists to reject. "raw" = full text minus fenced blocks,
    # so tool output stays exempt but inline code does not.
    ("source-citation", "blocker",
     r"(\b\w[\w./-]*\.[a-z]{1,4}:\d+\b|\bline \d+ of\b)",
     "cites source locations a user of the tool cannot act on", "raw"),
    # Expected-versus-actual is ALLOWED by maintainer ruling: "expected
    # nothing written, got a symlink" is the tool's contract stated from the
    # user's side. Prescription verbs are the violation, wherever they sit --
    # "Expected: the delete should be checked" still trips on "should be".
    ("prescribes-solution", "blocker",
     r"(suggested fix|the fix is\b|should be (changed|checked|stated|added"
     r"|refused|reported|separated|an? )|we should\b|\brecommend)",
     "prescribes the fix instead of describing the defect", "prose"),
    # The HEADING is the violation; the same words in running prose are fine.
    # Impact belongs in an issue -- as a sentence, not a section defending it.
    ("internal-structure", "blocker",
     r"^#{1,6}\s*(out of scope|on adding|on what|a note on|why (this|both|these)"
     r"|related gaps|two related|what \S+ (promises|does and does not)"
     r"|suggested fix)",
     "structures an argument rather than reporting a symptom", "raw"),
    ("design-table", "blocker",
     r"^\s*\|.+\|\s*$",
     "compares designs in a table where a user needs a sentence", "raw"),
    # Explicit identities only, never a general pattern over tool-shaped
    # words: a general pattern matches the system software a defect lives in
    # (tar, unzip) and gets the rule disabled within a week. The witness
    # persona "Night Watch" is the voice of every issue and is not a brand.
    ("brand-name", "blocker",
     r"\b(claude|codex|copilot|cursor|gemini|chatgpt|gpt-?[0-9o]|openai"
     r"|anthropic|devin|aider|windsurf)\b",
     "names the agent or vendor that did the work", "raw"),
]

QUERY = """
query($owner:String!, $name:String!, $cursor:String) {
  repository(owner:$owner, name:$name) {
    issues(first:25, after:$cursor, states:[OPEN,CLOSED]) {
      pageInfo { hasNextPage endCursor }
      nodes {
        number state title body
        userContentEdits(first:100) { totalCount nodes { diff } }
        comments(first:100) {
          totalCount
          nodes { body userContentEdits(first:100) { totalCount nodes { diff } } }
        }
      }
    }
  }
}
"""

# Pull requests are NOT issues in GitHub's GraphQL schema -- `issues()` excludes
# them entirely. The first version of this checker read only `issues()` and
# called the repo clean while 13 of 44 pull requests named the review tool in
# their bodies, and one described an unfixed vulnerability. A PR body is as
# public as an issue body, and review comments live in a third place again.
PR_QUERY = """
query($owner:String!, $name:String!, $cursor:String) {
  repository(owner:$owner, name:$name) {
    pullRequests(first:25, after:$cursor, states:[OPEN,CLOSED,MERGED]) {
      pageInfo { hasNextPage endCursor }
      nodes {
        number state title body
        userContentEdits(first:100) { totalCount nodes { diff } }
        comments(first:100) {
          totalCount
          nodes { body userContentEdits(first:100) { totalCount nodes { diff } } }
        }
        reviews(first:50) {
          totalCount
          nodes { body comments(first:50) { totalCount nodes { body } } }
        }
      }
    }
  }
}
"""


def gh(args, **kw):
    return subprocess.run(["gh"] + args, capture_output=True, text=True, **kw)


def fetch(repo, query, key):
    """Every node, following pagination. Truncation is an error, not a pass."""
    owner, name = repo.split("/", 1)
    out, cursor = [], None
    while True:
        cmd = ["api", "graphql", "-f", f"query={query}",
               "-F", f"owner={owner}", "-F", f"name={name}"]
        if cursor:
            cmd += ["-F", f"cursor={cursor}"]
        r = gh(cmd)
        if r.returncode != 0:
            raise RuntimeError(r.stderr.strip() or "graphql failed")
        payload = json.loads(r.stdout)
        # A partial response carries `errors` alongside `data`. Reading only
        # `data` would scan whatever survived and report clean on the rest.
        if payload.get("errors"):
            raise RuntimeError(f"graphql errors: {payload['errors']}")
        page = payload["data"]["repository"][key]
        out.extend(page["nodes"])
        if not page["pageInfo"]["hasNextPage"]:
            return out
        cursor = page["pageInfo"]["endCursor"]


def assert_complete(kind, number, label, conn):
    """A connection whose totalCount exceeds what was returned is unchecked.

    The claims checker in this repo once passed because its pattern could only
    find a claim while the claim was correct. This is the same shape: an issue
    with 120 comments and a 100-comment page would report clean on the last 20.
    """
    got, total = len(conn["nodes"]), conn.get("totalCount", len(conn["nodes"]))
    if total > got:
        raise RuntimeError(
            f"{kind} #{number}: {label} returned {got} of {total}; "
            "raise the page size or paginate. Refusing to report clean on a "
            "partial read.")


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


def raw_only(text):
    """Fenced blocks are tool output and stay exempt; inline code does not.

    The drafts that motivated the voice rules carried every source citation
    in backticks. A scope that strips inline code cannot see them."""
    return FENCE.sub(" ", text)


# The length bound is derived, not asserted: the accept fixtures are the
# corpus of issues accepted under the current standard, and the bound is the
# provisional floor until that corpus outgrows it. The pre-existing tracker
# is deliberately NOT the corpus -- it holds the contaminated issues this
# checker was built to catch, and measuring them would launder the defect
# into the rule. Floor chosen 2026-08-24; longest accept fixture was 125
# words at the time.
LENGTH_FLOOR = 200


# The six voice rules enforce the ISSUE genre -- what a user of the tool
# reads. A pull request body addresses a reviewer, and a line citation there
# is doing its job, so voice findings on PRs report and never gate. The
# deliberation rules above them gate everywhere, as they always have.
VOICE_RULES = {"source-citation", "prescribes-solution", "internal-structure",
               "design-table", "brand-name", "length"}


def check_text(text):
    """Every violation in one public text. The single judgment path: the
    tracker scan and the draft gate both call this, so they cannot diverge."""
    findings = []
    scoped = {"prose": prose_only(text), "raw": raw_only(text)}
    for rid, sev, pat, why, scope in RULES:
        m = re.search(pat, scoped[scope], re.I | re.M)
        if m:
            findings.append((sev, why, rid, m.group(0).strip()[:40]))
    words = len(prose_only(text).split())
    if words > LENGTH_FLOOR:
        findings.append(("blocker",
                         "runs past what a defect report needs",
                         "length", f"{words} words > {LENGTH_FLOOR}"))
    return findings


def sources_of(kind, i):
    """Every public text on one issue or PR: current, history, comments,
    comment history, and (for PRs) reviews and review comments."""
    num = i["number"]
    out = [("current", f"{i['title']}\n{i.get('body') or ''}")]
    assert_complete(kind, num, "edit history", i["userContentEdits"])
    for n, e in enumerate(i["userContentEdits"]["nodes"]):
        if e.get("diff"):
            out.append((f"edit -{n + 1}", e["diff"]))
    assert_complete(kind, num, "comments", i["comments"])
    for c, com in enumerate(i["comments"]["nodes"]):
        out.append((f"comment {c + 1}", com.get("body") or ""))
        assert_complete(kind, num, f"comment {c + 1} history",
                        com["userContentEdits"])
        for n, e in enumerate(com["userContentEdits"]["nodes"]):
            if e.get("diff"):
                out.append((f"comment {c + 1} edit -{n + 1}", e["diff"]))
    if "reviews" in i:
        assert_complete(kind, num, "reviews", i["reviews"])
        for r, rev in enumerate(i["reviews"]["nodes"]):
            out.append((f"review {r + 1}", rev.get("body") or ""))
            assert_complete(kind, num, f"review {r + 1} comments",
                            rev["comments"])
            for rc, com in enumerate(rev["comments"]["nodes"]):
                out.append((f"review {r + 1} comment {rc + 1}",
                            com.get("body") or ""))
    return out


def scan(repo):
    items = [("issue", i) for i in fetch(repo, QUERY, "issues")]
    items += [("pr", p) for p in fetch(repo, PR_QUERY, "pullRequests")]
    findings = []
    for kind, i in items:
        sources = sources_of(kind, i)
        for where, text in sources:
            for sev, why, rid, hit in check_text(text):
                findings.append((sev, kind, i["number"],
                                 i["state"].lower(), where, why, rid, hit))
    return len(items), findings


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
        print(f"{repo}: checked {count} issues and pull requests, "
              "with comments, reviews and every prior version")
        if not findings:
            print("  ok   every issue reads as a defect report")
            continue
        # What is fixable and what is not are different reports. Current text
        # can be corrected; a prior version cannot -- GitHub keeps and serves
        # it forever, and a pull request cannot be deleted at all. Failing the
        # build on immutable history would mean a check that can never pass,
        # and a check that can never pass is one somebody switches off. So
        # history is REPORTED and does not gate; only live text gates.
        # Voice rules gate only where the genre applies and the text is
        # actionable: the current body of an OPEN issue. On PRs and closed
        # issues they report -- a check that can never pass is one somebody
        # switches off, and rewriting the settled tracker is a choice for a
        # person, not a build failure.
        def gates(f):
            if f[4] != "current":
                return False
            if f[6] in VOICE_RULES:
                return f[1] == "issue" and f[3] == "open"
            return True
        live = [f for f in findings if gates(f)]
        history = [f for f in findings if not gates(f)]

        for sev, kind, num, state, where, why, rid, hit in sorted(live):
            label = "BLOCKER" if sev == "blocker" else "warn"
            print(f"  {label:8} {kind} #{num} [{state}] {why}")
            print(f"           {hit!r} ({rid})")
        if history:
            hist_items = sorted({(f[1], f[2]) for f in history})
            print(f"  note     {len(history)} finding(s) in text that is no longer "
                  f"live, across {len(hist_items)} item(s):")
            print("           " + ", ".join(f"{k} #{n}" for k, n in hist_items))
            print("           Prior versions and merged pull requests cannot be")
            print("           removed. These are reported, not gated.")
        if any(f[0] == "blocker" for f in live):
            print("\n  Fix the live text. Note that editing an issue or PR does not")
            print("  erase what was there: the old version stays public, so the")
            print("  edit is a correction, not a removal.")
            worst = max(worst, 1)
    return worst


SAMPLES = {
    "role-narration": "The operator chose the narrower option.",
    "quoted-decision": "His words: this over-refuses badly.",
    "undecided": "Open questions for the maintainer before building.",
    "weakness-roadmap": "The attack surface to scenario, roughly by nastiness.",
    "process-narration": "I decided to split this out after my review.",
    "internal-crossref": "Split out of #45, because it needs its own issue.",
    # The citation sits in INLINE CODE on purpose: this sample is the direct
    # regression on the scope trap, where prose_only() strips backticks and a
    # prose-scoped rule passes the exact text that motivated it.
    "source-citation": "The parser skips it at `main.rs:497` silently.",
    "prescribes-solution": "Suggested fix: check the removal and report it.",
    "internal-structure": "## Out of scope",
    "design-table": "| path | judgment | containment |",
    # Attribution flipped by maintainer ruling 2026-08-24: every issue speaks
    # as Night Watch, and agent identity stays out of public artifacts. This
    # line was previously in CLEAN as deliberate attribution.
    "brand-name": "Found by Codex reviewing the project guard.",
}

CLEAN = [
    # The witness persona is the voice of every issue, not a brand.
    "Night Watch found this while running the stress suite.",
    # Expected-versus-actual is allowed by ruling: contract, not prescription.
    "Expected nothing written; a symlink landed on disk instead.",
    # Argument words in PROSE are fine; only the heading form violates.
    "It refuses the entry as out of scope for a sort.",
    # System software a defect lives in is not a brand.
    "tar and unzip disagree about this member's type.",
    "Killing a run leaves ram disks attached. I ended up with four of them.",
    "```\nsweep: refused: I will not guess which files are yours\n```",
    "> the tool printed: I cannot judge this file",
    "Sweep sorts the textures into Images/ and the project cannot find them.",
    "The ? operator propagates the error to the caller.",
    "undo's verbatim output is compared against the committed transcript.",
    "The API panel renders the captured help verbatim.",
]


# What each reject fixture MUST trip, by filename. Naming the rules keeps
# the corpus honest: a fixture that merely trips "length" would let five
# rules rot while the test stayed green.
REJECT_MUST_FIRE = {
    "unpack-zip-symlink-long.md": {
        "source-citation", "internal-structure", "design-table",
        "prescribes-solution", "length"},
    "unpack-cleanup-claim-long.md": {
        "source-citation", "prescribes-solution", "length"},
}


def fixture_dir():
    import os.path
    return os.path.join(os.path.dirname(os.path.abspath(__file__)),
                        "issue-fixtures")


def selftest():
    """Every rule fires on a known-bad text; none fires on a clean one; the
    committed corpus is judged the way the maintainer judged it.

    A checker that has only ever passed is the thing that got this repo into
    trouble, so it proves itself before it judges anything. Samples run
    through check_text -- the same path as the tracker and the draft gate --
    so a rule whose scope hides its own sample fails here, not in the field.
    """
    import glob
    import os.path
    bad = 0
    for rid, _sev, _pat, _why, _scope in RULES:
        sample = SAMPLES.get(rid)
        if sample is None:
            print(f"FAIL self-test: no sample for rule {rid}", file=sys.stderr)
            bad = 1
            continue
        if rid not in {f[2] for f in check_text(sample)}:
            print(f"FAIL self-test: rule {rid} did not fire on {sample!r}",
                  file=sys.stderr)
            bad = 1
    for line in CLEAN:
        hits = [f for f in check_text(line) if f[2] != "length"]
        if hits:
            print(f"FAIL self-test: {[f[2] for f in hits]} fired on clean "
                  f"text {line!r}", file=sys.stderr)
            bad = 1
    # The corpus: the actual documents this gate exists to reject, and the
    # actual documents the maintainer accepted. Committed files, so the
    # proof outlives the session that wrote it.
    rejects = sorted(glob.glob(os.path.join(fixture_dir(), "reject", "*.md")))
    accepts = sorted(glob.glob(os.path.join(fixture_dir(), "accept", "*.md")))
    if not rejects or not accepts:
        print("FAIL self-test: fixture corpus missing from "
              f"{fixture_dir()!r}", file=sys.stderr)
        return 1
    for path in rejects:
        fired = {f[2] for f in check_text(open(path).read())}
        need = REJECT_MUST_FIRE.get(os.path.basename(path), set())
        missing = need - fired
        if not fired:
            print(f"FAIL self-test: reject fixture {path} passed clean",
                  file=sys.stderr)
            bad = 1
        elif missing:
            print(f"FAIL self-test: reject fixture {path} did not trip "
                  f"{sorted(missing)}", file=sys.stderr)
            bad = 1
    for path in accepts:
        hits = check_text(open(path).read())
        if hits:
            print(f"FAIL self-test: accept fixture {path} was rejected: "
                  f"{[f[2] for f in hits]}", file=sys.stderr)
            bad = 1
    return bad


def draft(path):
    """The gate. A draft that does not pass this does not get filed.

    Mandatory by maintainer ruling: there is no force flag, no allow flag,
    and no suppression comment. A wrong rule gets fixed, with its fixture
    updated -- nothing bypasses the gate for one document."""
    try:
        text = open(path).read()
    except OSError as e:
        print(f"FAIL cannot read draft: {e}", file=sys.stderr)
        return 2
    findings = check_text(text)
    if not findings:
        print(f"ok   {path} reads as a defect report")
        return 0
    lines = text.splitlines()
    for sev, why, rid, hit in findings:
        at = next((n for n, l in enumerate(lines, 1) if hit[:20] in l), None)
        where = f"line {at}: " if at else ""
        print(f"BLOCKER {rid}: {where}{why}")
        print(f"        {hit!r}")
    print()
    print("The draft does not pass. Fix the text, not the gate.")
    return 1


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.exit(selftest())
    if "--draft" in sys.argv:
        i = sys.argv.index("--draft")
        if i + 1 >= len(sys.argv):
            print("FAIL --draft needs a path", file=sys.stderr)
            sys.exit(2)
        if selftest():
            print("FAIL the checker failed its own self-test; not judging "
                  "anything", file=sys.stderr)
            sys.exit(2)
        sys.exit(draft(sys.argv[i + 1]))
    if selftest():
        print("FAIL the checker failed its own self-test; not judging anything",
              file=sys.stderr)
        sys.exit(2)
    sys.exit(main([a for a in sys.argv if a != "--self-test"]))
