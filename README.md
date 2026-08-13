# etudes

Three small command-line tools that tidy a folder without reading your private files.

**macOS only.**
```console
$ sweep ~/Desktop

Scanned 100 items  ·  names, sizes and dates only  ·  no contents read

  Screenshots      34 files   named "Screenshot ..."
  Photos, Aug 10   27 files   camera names, taken within 3 days
  Installers        3 files   .dmg and .pkg
  notes             7 files   7 filenames contain "notes"
  acme              5 files   5 filenames contain "acme"

  Left alone       24 files
    14 look like personal records. sweep does not touch these
    10 no clear group

  skipped 1 hidden item and 3 symlinks

Nothing has been moved.  Nothing left this machine.
```

Nothing moved, because nothing has been confirmed yet. The tax documents, the
medical records and the driver's licence are in that "left alone" count, and no
flag moves them.

An étude is a short piece written to master one technique, and in the right
hands also worth performing. These are tools built the same way: each removes
one recurring friction, each is small enough to read in an afternoon, and each
can prove its own claims rather than asking you to trust them.

| Tool | Does | Status |
|---|---|---|
| **`sweep`** | Organises the obvious and leaves the private alone | v0.3 |
| **`stash`** | Clears a folder now, decides nothing, brings it all back | v0.3 |
| **`unpack`** | One command for every archive format, safely | v0.3 |

## Install

```sh
cargo install --git https://github.com/risingmoonscholar/etudes sweep-cli
cargo install --git https://github.com/risingmoonscholar/etudes stash-cli
cargo install --git https://github.com/risingmoonscholar/etudes unpack-cli
```

The crates are named `*-cli`; the binaries they install are `sweep`, `stash` and
`unpack`, in `~/.cargo/bin`.

## Check the claims in a minute

Every étude ships the same two witnesses. Neither is a promise; both are
commands you can run.

```sh
cargo test --all                # 162 tests
scripts/no-network-test.sh      # the same suite, with socket(2) denied by the OS
```

The second one proves the sandbox works *before* running the suite: a control
program that opens a TCP connection must succeed unsandboxed and be denied
under the profile. The suite witnesses that its exercised code paths open no
sockets; it does not exercise the `security`, `unzip`, `gunzip`, or `tar`
subprocess call sites.

It uses `sandbox-exec`, so it is macOS like everything else here. Run it
anywhere else and it exits `2` saying the claim cannot be made on this host,
rather than passing. A witness that quietly passes where it cannot observe
anything is worse than no witness.

Beyond that, `etude-core` has **zero dependencies**, asserted by a test. There is no third-party code in the path that decides what happens
to your files.

## Try it without risking a real folder

The fixture generator builds a deliberately adversarial tree: tax forms,
medical records, an identity document, a filename containing a tab, a 200-character
filename, symlinks that point outside the directory. No real file of yours is
read during development or testing.

```sh
cargo run -p fixtures --bin mkfx -- /tmp/demo
cargo run -p sweep-cli --bin sweep -- /tmp/demo
cargo run -p stash-cli --bin stash -- /tmp/demo --for 3d
```

## Two rules the tools share

**Never coin a label the filesystem did not already contain.** A folder named
`Tax return 2024` is itself a disclosure, visible in Finder, indexed by
Spotlight, captured by every backup. Group names come only from words your own
filenames already carry. If *you* want a revealing name, `sweep review` will let
you choose one after telling you what it costs.

**Reading more must mean acting less.** `sweep --inspect-content` is off by
default and needs consent separate from `--yes`. What it reads can only ever
move a file into "left alone". It never influences a destination.

## For agents as well as people

These are built to be driven by both. The agent-facing surface is deliberate,
not incidental.

**Structured output.** `--json` on every tool, emitting the same data the human
rendering is drawn from. A tool that tells a person one thing and an agent
another is the worst kind of interface.

```sh
sweep ~/Desktop --json          # the plan
stash ~/Desktop --for 3d --json # what moved, and when it is due
stash status --json             # what is held, and whether it is overdue
unpack a.zip --list --json      # inspect an archive without extracting
unpack a.zip --json             # what happened, or why it was refused
```

**Meaningful exit codes**, uniform across the tools: `0` done · `1` nothing to
do · `2` refused · `3` error. "Refused" is distinct from "error" on purpose.
An agent must be able to tell a safety stop from a crash.

**The refusals are the guardrail, not the operator's judgment.** Every safety
property holds no matter who is driving:

| Gate | Effect on an agent |
|---|---|
| `--inspect-content` needs a TTY | convenience gate against accidental non-interactive use, not a security boundary against a process driving a pty |
| `review` needs a TTY | convenience gate against accidental non-interactive use, not a security boundary against a process driving a pty |
| sweep sensitive-name refusal | sweep leaves a tax document alone even with `--yes`; stash still moves everything |
| per-tool journals | an agent cannot undo the other tool's work by accident |

**`--json` discloses less, not more.** For files that look like personal
records, the JSON carries counts by category and **never the paths**. An agent
gets "3 tax documents were left alone", not a list of which files those are.
Handing over that index is exactly what the naming rule exists to prevent.

## What is broken

I wrote an adversarial harness and pointed it at my own tools: 33 scenarios
covering macOS filesystem hazards, crashes mid-apply, races between plan and
apply, 50,000-file trees, and real disk images for full, read-only and
case-sensitive volumes.

```sh
bash stress/run.sh        # 33 scenarios, 2 of them failing
```

The 2 failing scenarios are real and they are [filed](../../issues), each with a
reproduction. They fail on purpose so the reproductions do not rot, and CI
fails only when the number gets worse. The ones you are most likely to meet:

| | |
|---|---|
| [#7](../../issues/7) | Kill `sweep undo` partway and resume: the journal can keep re-reporting stale progress. A fix exists on a branch and is not merged. |
| [#12](../../issues/12) | 10,000 files takes about a minute with the journal on. A measurement, not a defect, kept failing so the number stays visible. |

The best story in the tracker is closed: an earlier fix swapped `rename` for
`link` plus `unlink` to stop silent overwrites, and that opened a crash window
where a killed process left one file under two names. It is now a single
atomic rename, and the recovery for old journals knows which link was sweep's
by its position in the journal rather than by guessing from inodes.

There is also an `unproven` count, kept separate from the passes on purpose. A
hazard that could not be exercised on this machine is not a hazard that passed.

## Layout

```
crates/
  etude-core/    scan, plan, apply, journal-first undo, zero dependencies
  etude-keep/    journal encryption (XChaCha20-Poly1305, key in the keychain)
  etude-read/    content inspection, mlock'd, zeroed, never persisted
  sweep-cli/     bin: sweep
  stash-cli/     bin: stash
  unpack-cli/    bin: unpack, dispatches to system tools, parses nothing
  fixtures/      synthetic adversarial trees; no real file is read in testing
```

Journals are namespaced per tool and share `~/.local/state/etudes`, so
`sweep undo` and `stash pop` cannot reverse each other's work.

## What these do not do

No daemon, no menu-bar app, no watching a folder in the background. `stash` does
not bring your files back on a timer; it tells you when they are due and waits
to be asked. Nothing here starts at login.

Changes are recorded in [CHANGELOG.md](CHANGELOG.md).

Apache-2.0.
