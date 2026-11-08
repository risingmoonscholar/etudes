<h1 align="center">Études</h1>

<h3 align="center">Careful file tools for macOS.</h3>

<p align="center">Every move is checked first, and every failure says why.</p>

<p align="center">
<a href="https://risingmoonscholar.github.io/etudes/"><b>Documentation</b></a> |
<a href="https://github.com/risingmoonscholar/etudes/releases"><b>Releases</b></a> |
<a href="CHANGELOG.md"><b>Changelog</b></a> |
<a href="CONTRIBUTING.md"><b>Contributing</b></a>
</p>

---

## About

Your Desktop has 400 screenshots. Your Downloads folder has 30 archives you
never opened. Études cleans that up, and when something isn't safe, it stops
and tells you why instead of guessing.

- **`sweep ~/Desktop`** shows the groups it would make. Nothing moves until
  you've seen the plan. Say yes, and it moves them. Changed your mind?
  `sweep undo`. Send a file type to your own folder with `--map`, and keep
  that route with `--save-routes`.
- **`stash`** gets files out of your way without deleting them. `stash pop`
  puts them back.
- **`unpack`** opens an archive into one clean folder, or places nothing at
  all. No half-extracted mess.

`sweep` sorts by names, dates and Finder data, and never reads what's inside your
files to decide.

What the tools promise:

- **Plan first.** `sweep` shows a plan and moves nothing until you run
  `sweep apply`. `sweep` and `stash` can save a plan with `--export-plan` and
  later apply exactly that plan. `unpack` lists and checks every archive member
  before it writes anything, and `unpack --list` extracts nothing.
- **Undo.** `sweep undo` and `stash pop` reverse what was moved, using an
  encrypted journal. `unpack` has no undo. It extracts into a private folder,
  checks every path in it, and moves it into place with one rename. If a run
  fails, it removes that private folder, or says where it is if it cannot.
- **Honest failure.** Exit codes are the same for every tool: `0` done, `1`
  nothing to do, `2` refused, `3` error. A refusal is not an error. Every
  `--json` result gives a reason code and says whether trying again can help.
- **Private files stay private.** Sweep decides from filesystem and Finder
  organization metadata only; it never reads file contents to decide. When it
  moves a file it reads the first and last 4 KiB, so undo can tell if that file
  changed. Files that look like personal records are never moved or tagged by
  sweep.
- **macOS only.**
- **No network.** Each tool declares a network policy of `none`. The test suite
  runs with sockets denied. That check does not cover the system tools the
  CLIs call (`security`, `unzip`, `tar`, `gunzip`).
- **Nothing in the background.** No daemon, no menu-bar app, nothing starts at
  login. `stash` tells you when files are due back and waits to be asked.
- **Small and checkable.** `etude-core` has zero dependencies. The suite has
  830 tests, and the stress harness has 86 scenarios.

## Getting Started

**0.5.5 is a release candidate, not a published release.** The commands below
are the proposed release pins and will work only after those tags are published.
For currently published versions, see [Releases](https://github.com/risingmoonscholar/etudes/releases).
To try this candidate from a checkout, use
`cargo install --path crates/sweep-cli --locked`
(or `crates/stash-cli`, `crates/unpack-cli`).

```sh
cargo install --locked --git https://github.com/risingmoonscholar/etudes --tag sweep-v0.5.5 sweep-cli
cargo install --locked --git https://github.com/risingmoonscholar/etudes --tag stash-v0.5.5 stash-cli
cargo install --locked --git https://github.com/risingmoonscholar/etudes --tag unpack-v0.5.5 unpack-cli
```

This installs `sweep`, `stash` and `unpack` in `~/.cargo/bin`. `--tag` pins
each tool to a tagged version; without it you get whatever `main` is that
day. Security properties, and the ones the tools do not have, are in
[SECURITY.md](SECURITY.md).

Try them:

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

Each tool prints its full usage with `sweep help`, `stash help` or
`unpack help`. To try them on a generated folder instead of a real one, see
[testing](docs/testing.md#try-it-without-risking-a-real-folder).

Where to read next:

- [Documentation site](https://risingmoonscholar.github.io/etudes/): recorded
  runs of each tool, its captured `--help`, and the exit codes.
- [sweep](docs/sweep.md): what it refuses to touch, Finder tags, saved folder
  routes, per-file proposals.
- [stash](docs/stash.md) and [stash batches](docs/stash-batches.md): selecting
  exact files, restoring one batch or one item.
- [unpack](docs/unpack.md): the extraction transaction, the size limit,
  download quarantine.
- [JSON, exit codes and refusal reasons](docs/json-and-exit-codes.md),
  [capability contracts](docs/contracts/README.md),
  [plan binding](docs/plan-binding.md) and
  [structured progress](docs/progress.md).
- [Undo, status and journals](docs/undo-and-journals.md), including
  [undo without a login keychain](docs/undo-and-journals.md#encrypted-undo-without-a-login-keychain)
  (`ETUDE_JOURNAL_KEY`).
- [Testing and checking the claims](docs/testing.md): the no-network check,
  the stress harness, the crate layout.

Two notes the tools point to from their own messages:

- `sweep --scheme existing` reads routes from a profile you write. The format
  is in [use folders you already have](docs/sweep.md#use-folders-you-already-have).
- `unpack`'s size limit is soft. The largest overshoot measured, on 2026-10-06
  on macOS 27.0.1 arm64, was 16,777,215 bytes. There is no guaranteed maximum.
  See [the size budget](docs/unpack.md#an-audited-transaction).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

Apache-2.0. See [LICENSE](LICENSE).
