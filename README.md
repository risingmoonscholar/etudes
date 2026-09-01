# etudes

Three small command-line tools that tidy a folder without reading your private files.

**macOS only.**
```console
$ sweep ~/Desktop

Scanned 108 items  ·  names, sizes and dates only  ·  no contents read

  Screenshots      34 files   named "Screenshot ..."
  Photos, Jan 15   27 files   camera names, taken within 3 days
  Installers        3 files   .dmg and .pkg
  Archives          3 files   .tar, .zip
  Documents        17 files   .docx, .md, .pdf, .txt
  Images            3 files   .jpg
  Scripts           3 files   .sh

  14 files look like personal records and were not touched
  1 file changed too recently to judge and was left alone
  3 files matched no group and were left where they are

  skipped 1 hidden item and 3 symlinks

Nothing has been moved.  Nothing left this machine.
Note: this listing is in your terminal scrollback.
Review: sweep review <path>     Apply: sweep apply <path> --yes
```

Nothing moved, because nothing has been confirmed yet. The tax documents, the
medical records and the driver's licence are the files reported as personal
records, and no flag moves them.

Each tool removes one recurring friction, is small enough to read in an
afternoon, and can prove its own claims rather than asking you to trust them.

| Tool | Does | Status |
|---|---|---|
| **`sweep`** | Organises the obvious and leaves the private alone | v0.5.2, maturing |
| **`stash`** | Clears a folder now, decides nothing, brings it all back | v0.5.2 |
| **`unpack`** | One command for every archive format, safely | v0.5.2 |

## Install

```sh
cargo install --git https://github.com/risingmoonscholar/etudes --tag sweep-v0.5.2 sweep-cli
cargo install --git https://github.com/risingmoonscholar/etudes --tag stash-v0.5.2 stash-cli
cargo install --git https://github.com/risingmoonscholar/etudes --tag unpack-v0.5.2 unpack-cli
```

The crates are named `*-cli`; the binaries they install are `sweep`, `stash` and
`unpack`, in `~/.cargo/bin`.

`--tag` pins the install to a released version. Without it `cargo install` takes
whatever `main` is at that moment, so two people running the same command on the
same day can get binaries that behave differently -- which is how three exit
codes changed under a version number that never moved. Drop the flag to track
`main` deliberately.

Security properties, and the ones this does not have, are in
[SECURITY.md](SECURITY.md).

## Check the claims in a minute

Every étude ships the same two witnesses. Neither is a promise; both are
commands you can run.

```sh
cargo test --all                # 253 tests
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

## What sweep refuses to touch

Four things, and the output always says which one applied and to how many
files. A count with no reason beside it is the defect this project keeps
finding in itself.

**Your projects.** A folder holding `project.godot`, `Cargo.toml`, a `.flp`,
a `.blend`, a `.ptx` -- 25 markers in all -- is stepped over rather than
sorted. A Godot `.tscn` references its siblings as `res://scripts/main.gd`,
absolute from the project root, so moving any file inside one breaks every
reference to it. The list was measured against real projects on a real disk,
not assembled from vendor documentation.

**Anything macOS treats as a single item.** A `.fcpbundle`, a `.band`, a
`.logicx`, a `.app`, a Pages document. The folder holding one is ordinary and
still gets swept -- the bundle is stepped over as a unit, not made contagious.

**Downloads still arriving.** `.part`, `.crdownload`, `.download` and friends,
whether they are files or directories. Safari writes `movie.mp4.download/`
with the partial data inside it.

**Anything touched in the last day.** A file you are working on right now is
not a file to be filed. `--since 6h` narrows the window, `--since 0` turns it
off, and an unreadable value is an error rather than a silent fallback to the
default.

```console
$ sweep ~/Downloads

  Documents       12 files   .pdf
  1 folder was left alone because it holds a project file
  2 files changed too recently to judge and were left alone
  1 download is still in progress and was left alone
```

### What it does not protect

`sweep` never reads your files, and that has a cost worth stating plainly.

A project *document* -- a `.blend`, an `.flp`, an `.als` -- references its
assets relative to itself and freely upward, out of its own folder. Sweep
steps over the folder holding one, which is right for a `project.godot` that
marks a project root by definition, and **not enough** for a `.blend` in
`scenes/` that points at `../textures/`. Those textures can still be sorted.
[Issue #49](../../issues/49) carries four reproductions.

The complete rule -- refuse any scan with a project document anywhere below it
-- was built, reviewed and rejected: one `.flp` made an entire Downloads folder
unsweepable, which removes the tool from the folder it exists for.

Similarly, a Final Cut library told to keep its media *outside* the bundle has
no filesystem-level mark saying which library owns that media. That
relationship lives in the library's own database. Sweep protects managed
layouts; it cannot protect an arrangement only the application knows about.

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
sweep ~/Desktop --json          # the plan, including projects_skipped,
                                # downloads_skipped, packages_skipped, and
                                # unknown_extensions -- counts per extension
                                # sweep has no rule for
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

**The agent supplies judgment; sweep supplies custody.** `unknown_extensions`
tells an agent what sweep has no rule for. The agent decides whether four
`.bpy` files deserve a folder -- that is the nondeterministic part, spent
where judgment is the job -- and comes back with `--map bpy=Blender`. Sweep
executes the mapping as one more table row, this run only: every refusal
still wins over a map, the moves are journaled, `undo` reverses them, and
`--map` with `--no-journal` is refused outright. If the folder does not
already exist, the output says so plainly: *the folder name "Blender" was
chosen by your agent, not derived from your files* -- the one sanctioned
exception to the naming rule, and it announces itself.

**`--json` discloses less, not more.** For files that look like personal
records, the JSON carries counts by category and **never the paths**. An agent
gets "3 tax documents were left alone", not a list of which files those are.
Handing over that index is exactly what the naming rule exists to prevent.

## What is broken

I wrote an adversarial harness and pointed it at my own tools: 38 scenarios
covering macOS filesystem hazards, crashes mid-apply, races between plan and
apply, 50,000-file trees, and real disk images for full, read-only and
case-sensitive volumes.

```sh
bash stress/run.sh        # 38 scenarios, 1 of them failing
```

The one failing scenario is real and it is [filed](../../issues), with a
reproduction. It fails on purpose so the reproduction does not rot, and CI
fails only when the number gets worse:

| | |
|---|---|
| [#12](../../issues/12) | 10,000 files takes about a minute with the journal on. A measurement rather than a defect, kept failing so the number stays visible. |

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

Journals are namespaced per tool and share `~/Library/Application
Support/etudes`, so `sweep undo` and `stash pop` cannot reverse each other's
work.

## What these do not do

No daemon, no menu-bar app, no watching a folder in the background. `stash` does
not bring your files back on a timer; it tells you when they are due and waits
to be asked. Nothing here starts at login.

Changes are recorded in [CHANGELOG.md](CHANGELOG.md).

Apache-2.0.
