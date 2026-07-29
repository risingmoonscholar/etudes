# etudes

Small studies in local-first software.

An étude is a short piece written to master one technique, and in the right
hands also worth performing. These are tools built the same way: each removes
one recurring friction, each is small enough to read in an afternoon, and each
can prove its own claims rather than asking you to trust them.

| Tool | Does | Status |
|---|---|---|
| **`sweep`** | Organises the obvious and leaves the private alone | v0.3 |
| **`stash`** | Clears a folder now, decides nothing, brings it all back | v0.3 |
| **`unpack`** | One command for every archive format, safely | v0.3 |

## The claims, and how to check them in a minute

Every étude ships the same two witnesses. Neither is a promise; both are
commands you can run.

```sh
cargo test                     # 88 tests
scripts/no-network-test.sh     # the same suite, with socket(2) denied by the OS
```

The second one is the load-bearing claim. It proves the sandbox works *before*
trusting it — a control program that opens a TCP connection must succeed
unsandboxed and be denied under the profile — so a network call anywhere in
these tools is a test failure rather than a code-review finding.

Beyond that, `etude-core` has **zero dependencies**, asserted by a test so it
cannot drift. There is no third-party code in the path that decides what happens
to your files.

## For agents as well as people

These are built to be driven by both. The agent-facing surface is deliberate,
not incidental.

**Structured output.** `--json` on every tool, emitting the same data the human
rendering is drawn from — a tool that tells a person one thing and an agent
another is the worst kind of interface.

```sh
sweep ~/Desktop --json          # the plan
stash ~/Desktop --for 3d --json # what moved, and when it is due
stash status --json             # what is held, and whether it is overdue
unpack a.zip --list --json      # inspect an archive without extracting
unpack a.zip --json             # what happened, or why it was refused
```

**Meaningful exit codes**, uniform across the tools: `0` done · `1` nothing to
do · `2` refused · `3` error. "Refused" is distinct from "error" on purpose —
an agent must be able to tell a safety stop from a crash.

**The refusals are the guardrail, not the operator's judgment.** Every safety
property holds no matter who is driving:

| Gate | Effect on an agent |
|---|---|
| `--inspect-content` needs a TTY | an agent **cannot** make sweep read file contents |
| `review` needs a TTY | an agent cannot rename a group into a revealing name |
| sensitive-name refusal | an agent cannot move a tax document, even with `--yes` |
| per-tool journals | an agent cannot undo the other tool's work by accident |

**`--json` discloses less, not more.** For files that look like personal
records, the JSON carries counts by category and **never the paths**. An agent
gets "3 tax documents were left alone", not a list of which files those are —
handing over that index is exactly what the naming rule exists to prevent.

## Two rules the tools share

**Never coin a label the filesystem did not already contain.** A folder named
`Tax return 2024` is itself a disclosure — visible in Finder, indexed by
Spotlight, captured by every backup. Group names come only from words your own
filenames already carry. If *you* want a revealing name, `sweep review` will let
you choose one after telling you what it costs.

**Reading more must mean acting less.** `sweep --inspect-content` is off by
default and needs consent separate from `--yes`. What it reads can only ever
move a file into "left alone" — it never influences a destination.

## Layout

```
crates/
  etude-core/    scan, plan, apply, journal-first undo — zero dependencies
  etude-keep/    journal encryption (XChaCha20-Poly1305, key in the keychain)
  etude-read/    content inspection — mlock'd, zeroed, never persisted
  sweep-cli/     bin: sweep
  stash-cli/     bin: stash
  unpack-cli/    bin: unpack — dispatches to system tools, parses nothing
  fixtures/      synthetic adversarial trees; no real file is read in testing
```

Journals are namespaced per tool and share `~/.local/state/etudes`, so
`sweep undo` and `stash pop` cannot reverse each other's work.

## Development

No real file of yours is read during development or testing. Everything runs
against a generated adversarial tree.

```sh
cargo run -p fixtures --bin mkfx -- /tmp/demo    # build a fake messy folder
cargo run -p sweep-cli --bin sweep -- /tmp/demo
cargo run -p stash-cli --bin stash -- /tmp/demo --for 3d
```

Apache-2.0.
