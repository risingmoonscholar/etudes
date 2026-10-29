# etudes

Three small command-line tools that tidy a folder without reading your private files.

**macOS only.**
```console
$ sweep --scheme folders ~/Desktop

Scanned 108 items  ·  filesystem and Finder organization metadata  ·  no contents read to decide

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
Applying reads the first and last 4 KiB of each file it moves, so undo can tell if one changed.
```

Nothing moved, because nothing has been confirmed yet. The tax documents, the
medical records and the driver's licence are the files reported as personal
records, and no flag moves them.

Each tool removes one recurring friction, is small enough to read in an
afternoon, and can prove its own claims rather than asking you to trust them.

| Tool | Does | Status |
|---|---|---|
| **`sweep`** | Organises the obvious and leaves the private alone | v0.5.4 candidate |
| **`stash`** | Clears a folder now, decides nothing, brings it all back | v0.5.4 candidate |
| **`unpack`** | Checks and extracts ZIP, tar variants and gzip | v0.5.4 candidate |

## Install

**0.5.4 is a release candidate, not a published release.** The commands below
are the proposed release pins and will work only after those tags are published.
For currently published versions, see [Releases](https://github.com/risingmoonscholar/etudes/releases).
To try this candidate from a checkout, use
`cargo install --path crates/sweep-cli --locked`
(or `crates/stash-cli`, `crates/unpack-cli`).

```sh
cargo install --locked --git https://github.com/risingmoonscholar/etudes --tag sweep-v0.5.4 sweep-cli
cargo install --locked --git https://github.com/risingmoonscholar/etudes --tag stash-v0.5.4 stash-cli
cargo install --locked --git https://github.com/risingmoonscholar/etudes --tag unpack-v0.5.4 unpack-cli
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
<<<<<<< HEAD
cargo test --all                # 591 tests
=======
cargo test --all                # 592 tests
>>>>>>> 082a451 (sync measured counts: 592 tests, 59 scenarios)
scripts/no-network-test.sh      # all but the process-control test under socket denial
```

The second one proves the sandbox works *before* running the suite: a control
program that opens a TCP connection must succeed unsandboxed and be denied
under the profile. On macOS, all tests except the SIGTERM process-control
witness run under denial; Seatbelt hides child enumeration from that test, so
the script runs it separately outside denial. That witness makes no network
calls. The denied suite witnesses that its exercised code paths open no
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

The current organization mode is named `--scheme folders` (and is the default
when the flag is omitted). A user exclusion has the highest precedence: review
can skip a group, and `apply --only NAME` selects only the named group. Safety
holds come next and remove files from every grouping rule. Built-in grouping
then claims eligible files; an agent `--map` can claim only what those built-in
rules leave behind. This is the policy order, not a claim that sweep detects
every filesystem state. The schemes are `folders`, `existing` and `tags`.

### Tag files, move nothing

`sweep --scheme tags` adds Finder tags and moves nothing. You name the tag for
each extension with `--map EXT=TAG`. The tag can be one you already use, a new
one an agent proposes, or a new one you asked for. Sweep never makes up a tag.

```console
$ sweep ~/Downloads --scheme tags --map pdf=Receipts --map png=Photos --export-plan ~/plan.bin

  Tags to add
  Receipts    12 files  .pdf
  Photos       4 files  .png   NEW TAG

  1 tag is new: Photos
  A tag is new when no file in the scanned folder carries it.
  Finder's own tag list cannot be read, so a tag you use in Finder but not on a file here also shows as new.

Nothing has been tagged or moved.  Nothing left this machine.

$ sweep apply --plan ~/plan.bin --plan-digest <digest> --yes
```

- **The plan is the gate.** Planning and exporting write no tag and touch no
  file. Tags are written only by `sweep apply --plan FILE --plan-digest SHA
  --yes`, for a plan you have seen. The live `sweep apply PATH` and `sweep review`
  refuse tags, and a plan made before a file's tags changed is refused as stale.
- **Tags are only added.** A file keeps every tag it has, in the same order, and
  the new tag goes after them. A file that already has the tag is left as it is.
  Nothing is moved, renamed, created or deleted.
- **Tagged files are tagged too.** Because a tag change is add-only and moves
  nothing, `--scheme tags` works on files that already carry tags without
  `--include-tagged`. That flag is refused with this scheme.
- **New tags are marked.** The plan, in words and in `--json`, lists each tag
  with its file count and marks each tag no file in the folder carries as new.
  An existing tag is added exactly as the folder spells it, colour included.
- **Unknown tag state is not guessed.** A file whose tag attribute cannot be read
  or parsed is listed and given nothing.
- **Undo restores recorded tag writes.** The journal records each file's tag
  attribute before the first write. Undo restores those exact bytes when a done
  record proves the write completed, the file has the same identity, and its
  tags still match. Replacement files and changed tags are left alone and named.
  A write without a done record is left alone and settled, since matching tag
  bytes cannot prove who wrote them. Older tag journals without file identity
  retain their original tag-byte checks. Apply needs a journal; `--no-journal`
  is refused.
- Sweep reads the tag list of the files it may tag, never their contents and
  never Finder comments. Personal-looking, recent, in-progress, project and
  hidden files are skipped, as in every scheme. `--scheme tags` needs macOS.

### Use folders you already have

`sweep --scheme existing PATH` reads the plain profile
`~/Library/Application Support/etudes/sweep-existing-profile.toml` (or the same
filename under `ETUDE_STATE_DIR` when that variable is set). Sweep never
creates this file. Add routes as versioned data you can review before use:

```toml
schema_version = 1

[[routes]]
extension = "bpy"
folder = "Blender Files"

[[routes]]
extension = "pdf"
folder = "Reading"
```

Each route applies only when its destination is already a direct, visible
folder in the selected directory. If a destination is missing, sweep refuses
and names it rather than letting a built-in rule send those files elsewhere.
A route needs at least three eligible files to form a group, and each extension
needs its own destination. Duplicate or malformed routes refuse before anything
moves. Safety refusals still take precedence over the profile; eligible profile
routes take precedence over built-in grouping. A routed file is claimed once,
even when its name also matches a built-in detector such as Screenshots. At
`--depth 2`, files already directly inside their route destination are reported
as in place and are not proposed again; deeper files remain eligible for
promotion into that destination. Sweep records the profile's SHA-256 in an
exported plan, and refuses replay if the profile changes.

| State | Current behavior |
|---|---|
| locked | An unreadable subtree is counted and the scan reports incomplete; visible siblings remain usable. |
| immutable | Sweep does not preflight the immutable flag. If the operating system refuses a move, apply reports the failure and leaves recovery to the journal; sweep does not claim it detected the flag. |
| invisible | Hidden entries are skipped and counted. Unreadable entries are separately reported as incomplete. |
| alias | Symlinks are skipped during scans; selected parent aliases are resolved for overlap checks. |
| iCloud placeholder | Dot-prefixed `.icloud` stubs are skipped as hidden entries, not grouped as their missing original file. |
| coordinated write | Recognized in-flight filename suffixes and recent modification times are held. Sweep does not query File Provider coordination, so those checks do not prove that no coordinated write is active. |

The implementation declares the same precedence and state accounting in
`etude_core::plan`; regression tests exercise safety holds against `--map` and
keep the named state inventory reviewable.

The four main plan-level refusal groups are named in the output with counts.
A count with no reason beside it is the defect this project keeps finding in
itself.

**Finder-tagged items.** A file or folder with a Finder tag is organisation
you may be using, so sweep leaves it where it is and says how many
(`1 Finder-tagged item was left alone`). It asks only whether a tag is
present: it never reads the tag names (only `--scheme tags` does, to add to
them), never reads Finder comments, and gives a caller a count, not names or
paths. Held tagged items are not counted in the "Scanned N items" total; the
JSON plan reports them separately as `tagged_skipped`. `--include-tagged`
organises tagged items anyway; it prints a warning on stderr every time, and
the tag stays on the item through a move on the same volume. A tagged item that
would have to cross volumes stays where it is, because a cross-volume copy does
not carry the tag.
`sweep apply` checks each item's tag again just before it moves the item and
again just after, so a tag added at the last moment holds the item or moves it
back. `stash` exists to clear a folder, so it takes tagged items too, and says
the folder is clear only when it is.

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

Sweep decides from filesystem and Finder organization metadata only; it never
reads file contents to decide. When it moves a file it reads the first and last
4 KiB, so undo can tell if that file changed. If a Finder tag lands on a file
while sweep is moving it and the file cannot be moved back because its original
place has been taken, sweep leaves it where it is, counts it as an item it
could not return, and records that in the journal. With `--scheme tags` it reads
each taggable file's tag list and writes it back with one more tag; it moves
nothing.

That has a cost worth stating plainly. Sweep reads whole contents only if you
pass `--inspect-content` and consent, or when a move crosses volumes and the
file has to be copied.

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

**Apply the plan you exported.** `sweep PATH --export-plan FILE --json` and
`stash PATH --export-plan FILE --json` save explicit private metadata artifacts
without moving anything. Replay with `--plan FILE --plan-digest SHA` binds the
original root, file identities, configuration, tool version and observation
contract. Changed observations refuse with a replan requirement before probes,
journals or moves. Direct apply still creates a fresh proposal in that invocation.
See [plan binding and its metadata limits](docs/plan-binding.md).

**Place individual files.** `sweep propose` reads your folder choice for each
file you name in an exported plan and writes a new plan. It moves nothing. The
export lists the files sweep is willing to move in `details.items`, each with an
`id`, `name`, `size` and the `group` it would join (`null` when no group claims
it). A file that looks personal, changed too recently, is still downloading,
sits in a project folder or package, is a link or is hidden is never listed, so
a proposal cannot name it.

```sh
sweep ~/Desktop --since 0 --export-plan ~/desktop.plan --json
# proposal.json
# {"schema_version":1,"plan_digest":"SHA","rest":"leave","decisions":[
#   {"item":"i-000004","folder":"Reading","mode":"require_existing"},
#   {"item":"i-000005","skip":true}]}
sweep propose --plan ~/desktop.plan --plan-digest SHA --proposal proposal.json \
  --export-plan ~/desktop-proposed.plan --json
sweep apply --plan ~/desktop-proposed.plan --plan-digest SHA2 --yes --json
```

The folder must already exist directly under the plan's root and is one plain
name. A file keeps its name. Files the proposal does not name stay where they
are, and `rest` must say `"leave"`. One bad decision refuses the whole proposal
and writes no plan. A proposal is at most 1 MiB and 5,000 decisions. The plan it
writes applies, and undoes, like any other. `sweep propose` reads no file contents.

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
**Breaking in 0.5.4 for JSON consumers:** every JSON result uses schema version 4,
including errors and refusals. Check the outer `schema_version` before parsing.
Operation fields move under `details` (for example, `details.groups`,
`details.moved`, and `details.paths_audited`). The shared envelope adds `scope`,
`observations`, `effects`, named `verification` claims, `recovery`, `disclosure`
and `reason`.
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

### Whether to try again: `reason`

Every `--json` result has a top-level `reason`. It holds a stable `code` and a
`retry` class, so a caller can decide what to do next without reading the
message text:

```json
"reason": {"code": "missing_input", "retry": "after_change", "subject": {"kind": "input"}}
```

- `not_needed`: the command finished; the code is `done` or `nothing_to_do`.
- `never`: the same request cannot work, so do not send it again. The codes
  `unknown_item` and `item_assigned_twice` also carry `subject.kind: "item"`.
- `after_change`: it can work once you change something, and `subject.kind`
  says what: the input, the destination, the plan, the profile or the journal.
- `later`: the same request may work if you wait, and `subject.kind` says what
  you are waiting for.

Exit codes keep their meaning. `reason` adds the part an exit code cannot
carry.

The full list is in each tool's contract: `sweep contract --json`, `stash
contract --json` and `unpack contract --json` list every code under
`details.refusal_codes`, with its retry class, its subject kind, the operations
it applies to and one line on why. `scripts/check-tool-contracts.py` makes each
tool return the codes it lists and compares the answer with the contract. It
prints which codes it proved and names the ones it could not reach. A contract
lists only codes the tool has a code path for; the few the check cannot reach
need a missing login keychain, an internal fault or a race it cannot stage.

To ask about an operation after the fact, without moving anything:

```sh
sweep status --id ID --json     # one sweep operation, by its id
sweep status PATH --json        # the operations recorded for a folder
stash status --id ID --json     # one stash operation, by its id
```

The id is `recovery.operation` in the result of the command that moved the
files. The answer gives a state (`refused`, `completed`, `interrupted`,
`reversed` or `partly_reversed`) and counts of planned, moved and reversed
items. `refused` is an apply that was turned away before it moved anything. It never
lists file names. An id that no journal holds gives exit 1 with the code
`unknown_operation`. `reversed` means nothing is still moved; `partly_reversed`
means some files are back and some are not.

A result's `status` and `reason` always agree: when the status is `incomplete`,
`refused` or `error`, the reason is never `done` or `nothing_to_do`. An undo or
pop that had to leave a file behind exits 0 as before, with status `incomplete`
and the code `operation_incomplete`.

Two applies on one folder at once are not held apart. One wins. The other ends
in one of two ways, and timing decides which:

- **Refused before it moved anything** (exit 2, `plan_stale`): the folder
  changed under its plan. Nothing of its own needs undoing, and `sweep status`
  reports its record as `refused`. Scan and apply again.
- **Stopped part-way** (exit 3, `operation_incomplete`): it had started and
  then collided. `recovery.operation` is an id that `sweep status --id` reports
  as `interrupted`. Run `sweep undo` on the folder before planning again.

After the part-way ending, run `sweep undo` on the folder until it says there
is nothing left to undo. It reverses the apply that recorded the remaining
moves first, then the stopped apply, restoring every original path and byte.
This works even when the stopped apply's journal is the newest. Journals that
actually lost several move records are still refused without touching files.

If `sweep undo` is killed after it has put the last file back, the folder that
`sweep apply` created can be left behind, empty. Run `sweep undo` again: it
removes that folder and finishes. It removes only a folder the journal says
apply created, and only if the folder is still the same one and has nothing
in it. A folder needed as an origin or guard by another live journal is kept;
unreadable history also keeps the folder. Undo reports how many empty folders
it retained for journals. A folder with anything inside it, or one that was
replaced, is left alone. Undo never deletes or overwrites a file.

Known gap: a folder that holds only recent files reports `nothing_to_do`, with
no code that says "try again later".

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

I wrote an adversarial harness and pointed it at my own tools: 59 scenarios
covering macOS filesystem hazards, crashes mid-apply, races between plan and
apply, exact item-cap boundaries, and real disk images for full, read-only and
case-sensitive volumes.

```sh
bash stress/run.sh        # 59 scenarios
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
stash /path/to/another-folder --json
# Save details.stash_id from the receipt, then restore that exact batch:
stash pop --id ID
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
Sweep undo refuses to search past an unreadable newer journal, even for a named
folder. Stash folder pop authenticates every stash journal before filtering by
folder. An unreadable older journal also blocks it, even if already completed
or from another folder: exit 3 says the journal is unreadable and cannot restore,
with files and journal bytes unchanged. This also applies to `stash pop` in the
current directory. Restore a known batch with `stash pop --id ID` and its original
key; exact-ID selection does not search unrelated journals.

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
from these manuals. The unpack capability declaration is version 6 and the result
envelope is version 4. `docs/contracts/v6/unpack.json` is the canonical pin and
`v1/unpack.json` mirrors it; earlier pins stay immutable.

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

The result prints counts and a persistent operation id; it does not list source
paths, basenames, or the holding path. JSON carries `details.stash_id`; selection
still includes `details.holding_root: "<redacted>"`, not a usable path.
Restore the whole operation with `stash pop --id ID`. Use `stash inspect --id ID` to see path-free opaque item
ids and which items remain held, then `stash pop --id ID --item ITEM` to restore
one item. Each selected restore is durably recorded, and inspection reports the
actual held count after partial restores. Pop uses the original absolute parents
stored in the encrypted journal, and removes only empty holding directories
authenticated by that journal. Selected directories retain the existing opaque
directory fingerprint behavior; this is not a recursive content-integrity
claim. Receipts continue to name uninstrumented access as unproven, and no zero
counter proves absence of reads.

Journalled stash operations return a persistent `stash_id`. Use `stash pop --id ID` for exact restoration, or `stash pop --latest` to explicitly select the latest live batch. Folder operations can coexist; ambiguous path-based restores refuse. Deadlines remain information until an explicit pop. See [independent stash batches](docs/stash-batches.md) for identity, legacy compatibility and disclosure semantics.
