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
| **`sweep`** | Organises the obvious and leaves the private alone | v0.5.3 candidate |
| **`stash`** | Clears a folder now, decides nothing, brings it all back | v0.5.3 candidate |
| **`unpack`** | Checks and extracts ZIP, tar variants and gzip | v0.5.3 candidate |

## Install

**0.5.3 is a release candidate, not a published release.** The commands below
are the proposed release pins and will work only after those tags are published.
For currently published versions, see [Releases](https://github.com/risingmoonscholar/etudes/releases).
To try this candidate from a checkout, use
`cargo install --path crates/sweep-cli --locked`
(or `crates/stash-cli`, `crates/unpack-cli`).

```sh
cargo install --locked --git https://github.com/risingmoonscholar/etudes --tag sweep-v0.5.3 sweep-cli
cargo install --locked --git https://github.com/risingmoonscholar/etudes --tag stash-v0.5.3 stash-cli
cargo install --locked --git https://github.com/risingmoonscholar/etudes --tag unpack-v0.5.3 unpack-cli
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
cargo test --all                # 351 tests
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
assets relative to itself and freely upward, out of its own folder. In this
candidate, a document found during the scan also holds related asset families
beside it: a `.blend` in `scenes/` keeps sibling textures in place. Unrelated
documents and screenshots can still be organized. This protection follows
filesystem names and layout; it does not parse the project's references or
promise to discover documents outside the scan's scope.

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

**Versioned capability contracts.** `sweep contract --json`, `stash contract
--json` and `unpack contract --json` declare observation and mutation scope,
network policy, overwrite, deletion, reversibility, persistent state and exact
supported formats. Contract queries have no application-state side effects.
Each carries `schema_version`, `tool_version`, `operation_id` and `status`.
Binary probes in the test suite compare declarations with exercised filesystem
behaviour and pin each contract's schema; deliberate schema and behaviour mutants
must fail those probes. The declaration is not a receipt or proof of no access.
Every JSON result now uses schema version 2, including errors and refusals.
Operation fields move under `details` (for example, `details.groups`,
`details.moved`, and `details.paths_audited`). The shared envelope adds `scope`,
`observations`, `effects`, named `verification` claims, `recovery` and `disclosure`.
Status is `done`, `nothing_to_do`, `refused`, `incomplete` or `error`.
Read receipts separate attempted, observed, verified and failed operations,
and name unproven coverage. They contain categories and counts, never payloads,
keys or environment values. Zero counts do not prove absence of access. See [the contract specification and witness limits](docs/contracts/README.md).

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

**Unpack publishes an audited transaction.** Extraction happens in a private
0700 staging directory beside the destination. Before tidying and again before
the final rename, a tree audit checks every actual member, including implicit
directories, for containment and type. Only directories and ordinary files
are accepted; links, special files, setuid/setgid modes and unreadable members
refuse publication. The requested destination appears with one rename after
the final audit. Failed runs remove staging; if removal fails, the diagnostic
names the remaining staging path or explicitly says its state is unverified.

**Unpack's size budget is a monitored soft limit.** `--max-size N[G|M]` replaces
the default of half the target volume's free space. System extractors write
directly, so unpack measures logical file bytes every 100 ms and after exit,
then kills and reaps an over-budget extractor and measures again. Refusals
report the final exact byte total, limit and overshoot. Read errors stop the
transaction instead of silently undercounting. This is not a hard write cap,
and filesystem allocation, metadata and the private archive copy are outside
that logical-byte measurement.

On 2026-10-06, macOS 27.0.1 arm64, the maximum measured overshoot in
`80-finder-unpack` was **16,777,215 bytes**: a 16,777,216-byte payload against a
one-byte limit. The scenario runs ZIP, tar and gzip with one-byte and 1 MiB
limits, three repetitions each (18 trials), and prints
`MEASURED_MAX_OVERSHOOT_BYTES`. Reproduce with `bash stress/run.sh 80-finder-unpack`.
This maximum describes that fixture on that host; larger or faster extractions
can overshoot further. There is no guaranteed maximum overshoot.

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

Large apply, undo, stash and pop operations report item-count milestones to
stderr, leaving JSON on stdout untouched. Small operations stay quiet; large
ones produce at most ten updates.

**`--json` discloses less, not more.** For files that look like personal
records, the JSON carries counts by category and **never the paths**. An agent
gets "3 tax documents were left alone", not a list of which files those are.
Handing over that index is exactly what the naming rule exists to prevent.

## What is broken

I wrote an adversarial harness and pointed it at my own tools: 48 scenarios
covering macOS filesystem hazards, crashes mid-apply, races between plan and
apply, exact item-cap boundaries, and real disk images for full, read-only and
case-sensitive volumes.

```sh
bash stress/run.sh        # 48 scenarios
```

Every failed assertion is actionable. Timing is recorded as a measurement, but
it cannot exempt an integrity assertion in the same scenario.

The best story in the tracker is closed: an earlier fix swapped `rename` for
`link` plus `unlink` to stop silent overwrites, and that opened a crash window
where a killed process left one file under two names. It is now a single
atomic rename, and the recovery for old journals knows which link was sweep's
by its position in the journal rather than by guessing from inodes.

There is also an `unproven` count, kept separate from the passes on purpose. A
hazard that could not be exercised on this machine is not a hazard that passed.

Each run writes a compact JSON bundle under `stress/results/` (or
`STRESS_RESULTS_DIR`): one row per case with its contract, capability, tier,
duration, verdict, child exit, and any retained failure evidence. CI uploads
that bundle with failure-only transcripts and assertion records.

A scenario can also run directly against built release binaries:

```sh
BIN="$PWD/target/release" SCENARIO=85-scenario-exit-status bash stress/scenarios/85-scenario-exit-status.sh
bash scripts/check-scenario-outcomes.sh  # compare direct runs, the batch, and the origin/main runner
```

The harness records assertions separately from stdout. Failed assertions produce
exit 1 even after `exit 0`, cleanup, command substitution, or `exec`. Subshells
still have their own shell counters; their assertion records reach the wrapper.
Runs with only unproven assertions exit 2; a run with no assertions fails.

## Layout

```
crates/
  etude-core/    scan, plan, apply, journal-first undo, zero dependencies
  etude-keep/    journal encryption (XChaCha20-Poly1305, keychain or supplied key)
  etude-read/    content inspection, mlock'd, zeroed, never persisted
  etude-cli-support/ bounded progress presentation shared by the CLIs
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

## Encrypted undo without a login keychain

Both `sweep` and `stash` accept `ETUDE_JOURNAL_KEY`: exactly 64 ASCII hex
characters encoding a randomly generated 256-bit key. When set, it is used
instead of the login keychain. An empty, malformed, or non-Unicode value is
refused; it never falls back to another key or to a plaintext journal.

For a temporary session, generate a key with the operating system's secure
random source, then keep the same environment for apply and restore:

```sh
export ETUDE_JOURNAL_KEY="$(python3 -c 'import secrets; print(secrets.token_hex(32))')"
sweep apply /path/to/folder --yes
sweep undo /path/to/folder
stash /path/to/another-folder
stash pop /path/to/another-folder
```

Keep a durable copy in your secret manager if you need undo after that shell
closes. Supply that same key through the environment on later runs. Do not
use a password, repeat a short value, or generate a fresh key before undo.
The tools validate the encoding and size; they cannot measure the randomness
of a supplied secret. Do not paste a literal key into command arguments or
shell history, print it in logs, or enable shell tracing while supplying it.
Child processes inherit environment variables, and code running as you may
be able to read them. Unset the variable when the session is finished.

The encrypted journal format and cipher are unchanged. `sweep forget` cannot
destroy a supplied key and refuses key destruction while the variable is set;
you must remove all retained copies yourself. Losing the key loses undo.
Use a separate `ETUDE_STATE_DIR` for each key if you maintain multiple keys.
Undo/pop refuse to search past an unreadable newer journal, even for a named
folder, because it could describe a newer operation on those same files.

If key acquisition fails, the only way to move files without a journal is
explicit `--no-journal` on `sweep apply` or `stash`. This writes no journal
and removes undo; `stash pop` cannot restore an unjournaled stash. Files then
need to be restored manually.

Undo and pop also refuse when journal discovery or reads fail, when a journal
entry is not a regular file, or when journals have identical modification times
and their order cannot be established. They do not guess which operation is newer.

### Download quarantine on extracted archives

Unpack captures `com.apple.quarantine` from the same no-follow source descriptor
used to copy the archive. A change to that attribute during copying refuses the
operation. After junk removal and wrapper flattening, unpack applies the captured
opaque value to every final file and directory, including the destination root,
and reads it back before publishing. Attribute values are never printed. A source
without that attribute does not cause unpack to add one.

On macOS 27.0.1 (26A434), synthetic raw-extractor probes observed `/usr/bin/unzip
-o -q` and `/usr/bin/tar -xf/-xzf/-xjf/-xJf` propagating the archive mark; repeated
observations can include a changed mark rather than exact equality. `/usr/bin/gunzip
-c` with stdout redirected to a file omitted it. The unmodified unpack binary
anchored by copying the archive: ZIP/JAR and all TAR suffixes produced changed
payload marks and no marked destination root; bare GZIP omitted the mark entirely.
The complete extraction path is measured separately from the raw extractor.
`crates/unpack-cli/tests/quarantine_matrix.py` probes all ten supported suffixes,
with marked and unmarked sources, and checks final payload bytes and attributes.

macOS can represent attributes as `._` AppleDouble companions on exFAT. These
companions are retained when needed to store the final quarantine state. Unpack
refuses publication and removes staging if the captured attribute cannot be set
or read back exactly; it never reports a successful downgrade to unmarked output.
A native extended-attribute filesystem is not required when the OS's companion
representation supports exact readback. The quarantine receipt reports read
attempts and verified comparisons, without recording the attribute value.

Sources: Apple's [getxattr(2)](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/getxattr.2.html)
and [fsetxattr(2)](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fsetxattr.2.html)
document descriptor access, absent attributes, and unsupported storage. Extractor
propagation and exFAT storage are measured behavior, rather than promises inferred
from these manuals. The unpack capability declaration is version 3; the result
envelope remains version 2. `docs/contracts/v3/unpack.json` is the canonical pin,
`v1/unpack.json` mirrors it, and `v2/unpack.json` remains the prior immutable pin.

Opt in to versioned stderr progress with `sweep apply PATH --yes --progress-json` (also available during review). Records distinguish planned entries, successful moves, and durable journal acknowledgements; `--json` stdout remains one result envelope. See [structured progress](docs/sweep-progress.md) for the schema and failure counts. Journal persistence is unchanged; no speed improvement is claimed.

### Stash exactly the selected objects

`stash select PATH...` takes the chosen files and directories from any number of
parents. A selected directory moves as one opaque object; its children are not
scanned. The existing `stash DIR` command still stashes that directory's visible
contents. Explicit selections can include hidden files, but retain the existing
system and credential-directory location refusals. Known sync locations matched
by the existing path-marker rules are refused for selected sources and holding
parents before any move. This mode has no cloud-support override. Explicit symlinks and special
files are refused before holding storage is created. Directory selections require
holding storage on their own volume; regular files retain the existing copy
fallback when crossing volumes.

`stash select --from0 FILE` reads UTF-8 paths terminated by NUL; use `--from0 -`
for stdin. Newlines inside a path are preserved. The list must terminate every
path, contains no empty records, and is limited to 32 MiB and 100,000 objects.
Positional sources can accompany a list. Use `--` before option-shaped positional
filenames. Duplicate canonical paths, repeated object identities such as hard-link
aliases, and any ancestor/descendant overlap are refused before any move. Overlap
checks sort canonical path components; they do not scan a common ancestor.

Selections require an encrypted journal. `--no-journal` remains available for
legacy whole-folder stashing and is refused for explicit selections. Each selection
uses one private operation root and numbered holding slots, so equal basenames
from different parents cannot collide. `--into PARENT` chooses the holding parent;
otherwise the first canonical selected object's parent is used. A holding parent
inside a selected directory is refused.

The result prints counts and the minimum operation-root locator needed for
`stash pop LOCATOR`; it does not list source paths or basenames. Pop uses the
original absolute parents stored in the encrypted journal, and removes only
empty holding directories authenticated by that journal. Selected directories
retain the existing opaque directory fingerprint behavior; this is not a recursive
content-integrity claim. Receipts continue to name uninstrumented access as
unproven, and no zero counter proves absence of reads.

Journalled stash operations return a persistent `stash_id`. Use `stash pop --id ID` for exact restoration, or `stash pop --latest` to explicitly select the latest live batch. Folder operations can coexist; ambiguous path-based restores refuse. Deadlines remain information until an explicit pop. See [independent stash batches](docs/stash-batches.md) for identity, legacy compatibility and disclosure semantics.
