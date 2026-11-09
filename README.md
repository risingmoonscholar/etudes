<h1 align="center">Études</h1>

<p align="center">Three command-line tools that move and extract files on macOS.</p>

<p align="center">
<a href="https://risingmoonscholar.github.io/etudes/"><b>Documentation</b></a> |
<a href="https://github.com/risingmoonscholar/etudes/releases"><b>Releases</b></a> |
<a href="CHANGELOG.md"><b>Changelog</b></a> |
<a href="CONTRIBUTING.md"><b>Contributing</b></a>
</p>

---

## The tools

Études is the name of this project. It contains three tools for macOS. You
install each tool on its own.

- **`sweep`** sorts the files in a folder into groups. It shows a plan first
  and moves nothing until you run `sweep apply`.
- **`stash`** moves files out of a folder without deleting them. `stash pop`
  moves them back.
- **`unpack`** extracts an archive into one new folder. If the extraction
  fails, `unpack` puts no files in place.

## What the tools do

**A plan before a change.** A plan is the list of moves that a tool will do.
`sweep` shows a plan and moves nothing until you run `sweep apply`. `sweep`
and `stash` can save a plan with `--export-plan` and apply exactly that plan
later. `unpack` lists and checks each archive member before it writes a file.
`unpack --list` extracts nothing.

**Undo.** `sweep undo` and `stash pop` move the files back. To do this, they
use a journal. A journal is an encrypted record of the moves that a tool did.

**Extraction.** `unpack` has no undo. It extracts into a private folder, checks each path in
that folder, and then moves the folder into place with one rename. If the run
fails, `unpack` removes the private folder. If it cannot remove the folder, it
tells you where the folder is.

**The same exit codes.** All three tools use the same exit codes: `0` done,
`1` nothing to do, `2` refused, `3` error. A refusal is not an error. Each
`--json` result gives a reason code. It also tells you if a new attempt can
help.

**File contents.** Sweep decides from filesystem and Finder organization
metadata only; it never reads file contents to decide. When it moves a file it
reads the first and last 4 KiB, so undo can tell if that file changed.
`sweep` does not move or tag files that look like personal records.

**Folders that you choose.** With `--map`, `sweep` sends a file type to a
folder that you choose. A route is this link from a file type to a folder.
`--save-routes` keeps the route for later runs.

**macOS only.** The tools run only on macOS.

**No network.** Each tool declares a network policy of `none`. The test suite
runs with network sockets denied, except for one process-control test. This
check does not cover the macOS programs that the tools run: `security`,
`unzip`, `tar` and `gunzip`.

**No background process.** The tools install no daemon (a program that runs in
the background), no menu-bar app and no login item. `stash` tells you when
files are due back. It does not move them back until you tell it to.

**Code that you can check.** `etude-core` is the library that the three tools
share. It has zero dependencies. The test suite has 830 tests. The stress
harness (`stress/run.sh`) runs 86 scenarios against tools built from this tree.

## Install

**Caution: 0.5.5 is a release candidate, not a published release.** The
commands below use the 0.5.5 tags. They will work only after those tags are
published. For the published versions, see
[Releases](https://github.com/risingmoonscholar/etudes/releases).

To install this candidate from a clone of this repository, run
`cargo install --path crates/sweep-cli --locked`. For the other tools, use
`crates/stash-cli` or `crates/unpack-cli`.

```sh
cargo install --locked --git https://github.com/risingmoonscholar/etudes --tag sweep-v0.5.5 sweep-cli
cargo install --locked --git https://github.com/risingmoonscholar/etudes --tag stash-v0.5.5 stash-cli
cargo install --locked --git https://github.com/risingmoonscholar/etudes --tag unpack-v0.5.5 unpack-cli
```

These commands install `sweep`, `stash` and `unpack` in `~/.cargo/bin`. The
`--tag` flag pins each tool to a tagged version. Without `--tag`, you get the
current state of `main`.

## First commands

```sh
sweep ~/Desktop                 # show the plan; nothing moves
sweep apply ~/Desktop --yes     # move the proposed groups
sweep undo ~/Desktop            # put them back

stash ~/Desktop --for 3d        # move the folder's contents aside; due in 3 days
stash status                    # what is stashed, and when it is due back
stash pop --id ID               # bring back one batch, by the id stash printed

unpack a.zip --list             # show what is inside; extract nothing
unpack a.zip                    # extract into its own folder
```

For the full usage of a tool, run `sweep help`, `stash help` or `unpack help`.
To try the tools on a generated folder instead of a real folder, see
[testing](docs/testing.md#try-it-without-risking-a-real-folder).

## Safety and security

[SECURITY.md](SECURITY.md) lists the security properties of the tools. It also
lists the properties that the tools do not have.

## Documentation

- [Documentation site](https://risingmoonscholar.github.io/etudes/): recorded
  runs of each tool, the `--help` output of each tool, and the exit codes.
- [sweep](docs/sweep.md): the files that `sweep` does not touch, Finder tags,
  saved routes, and proposals for each file.
- [stash](docs/stash.md) and [stash batches](docs/stash-batches.md): how to
  select exact files, and how to restore one batch or one item.
- [unpack](docs/unpack.md): how extraction works, the size limit, and the
  download quarantine.
- [JSON, exit codes and refusal reasons](docs/json-and-exit-codes.md),
  [capability contracts](docs/contracts/README.md),
  [plan binding](docs/plan-binding.md) and
  [structured progress](docs/progress.md).
- [Undo, status and journals](docs/undo-and-journals.md), with
  [undo without a login keychain](docs/undo-and-journals.md#encrypted-undo-without-a-login-keychain)
  (`ETUDE_JOURNAL_KEY`).
- [Testing and checking the claims](docs/testing.md): the no-network check,
  the stress harness, and the layout of the crates.

## Notes that the tools refer to

The messages of the tools refer to these two notes.

- `sweep --scheme existing` reads routes from a profile that you write. For
  the format, see
  [use folders you already have](docs/sweep.md#use-folders-you-already-have).
- The size limit of `unpack` is soft, so an extraction can go above it. The
  largest measured amount above the limit was 16,777,215 bytes. This
  measurement was on 2026-10-06, on macOS 27.0.1 arm64. There is no guaranteed
  maximum. For more, see [the size budget](docs/unpack.md#an-audited-transaction).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

Apache-2.0. See [LICENSE](LICENSE).
