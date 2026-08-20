# Contributing

## Specifications carry their sources

Before code that depends on how a platform behaves, the specification for that
work names the document that says so. Syntax, environment, permissions,
endpoints and state all come from the platform's own documentation. None of
them are derived from what seems likely.

A specification section that rests on platform behaviour has a **Sources**
list. Each entry is a document, a standard, or a manual page, with the
specific claim taken from it. "Camera files are usually named `IMG_` plus a
number" is not sourceable and does not belong in a spec. "DCF (JEITA CP-3461)
names an image file as four alphanumeric characters followed by a number in
0001..9999" is.

Where the primary document is paywalled or unavailable, cite the best
secondary source **and say which it is**. A secondary source is enough to
build on and not enough to call verified.

An unsourced platform claim does not block thinking. It blocks the spec
being called finished, and it blocks a test corpus being called evidence.

### Why this is a rule and not advice

A guess about platform behaviour is the most expensive kind, because it
survives every reviewer who has not read the document either, and it survives
tests written from the same guess.

This was written after an actual failure in this repository. The argument for
matching `1099` as a bare substring rested on the claim that filenames have no
reliable delimiters, with a set of macOS copy-name variants invented to support
it. macOS names duplicates predictably and Apple documents it. The proposed fix
was then measured against a corpus of thirty filenames that had also been
invented, and it passed cleanly, which proved nothing at all: a corpus drawn
from the same guesses as the rule will always agree with the rule.

Reading the actual standard produced a better rule and a different bug. DCF
specifies four digits; the fixture that reported the bug generates five. The
filename in the issue title cannot come from a camera.

### Test corpora

A corpus is evidence when its entries come from the platform's documented
behaviour or from real observed files. A corpus assembled from imagination is
a restatement of the author's assumptions, and its passing is not a result.

Fixtures that stand in for a platform's output follow that platform's format.
A camera fixture that does not conform to DCF is testing a filename no camera
will produce.

## Every change goes through a pull request

Including one-line documentation commits. `main` has no branch protection, the
pre-push hook only checks for orphaned merges, and nothing else will stop a
direct push -- so this is a rule people keep rather than one a machine enforces.

The size of a change is not the test. The commit that prompted writing this down
was a README edit pinning the install command to a release tag: small, and also
the kind of change a reviewer should see, and it moved `main` past a tag that had
just been cut. If a change feels too trivial to justify a branch, that is the
moment the habit is being tested rather than an exception to it.

This file is its own evidence. It was written on 2026-08-14, committed to a
branch, and never merged -- four days sitting where nobody would find it, holding
a rule that had been asked for explicitly. Work that does not go through a pull
request does not reliably arrive.

Two things worth having next to the rule:

- **The stress suite runs on pull requests, and always reports.** It did not
  once: the ratchet job was skipped on PRs, so a green page said nothing about
  the scenarios, and one pull request existed *because* the ratchet was red
  while its own page showed all-green throughout. It now runs whenever a change
  touches code or the harness, and reports a skip -- rather than nothing -- for
  documentation-only diffs. A required check that never reports leaves a pull
  request pending forever, which is why it reports either way.
- **A rebased branch is re-tested on the resolved tree.** Running the suite
  during a rebase with unresolved conflicts still prints a passing count. That is
  a number shaped like a result.
