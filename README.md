<h1 align="center">Études</h1>

Études is a set of three command-line tools for files on macOS. You install
and use each tool separately.

- **sweep** sorts the files in a folder into groups. It shows you a plan
  first and moves nothing until you apply the plan.
  Sweep decides from filesystem and Finder organization metadata only; it never
  reads file contents to decide. When it moves a file it reads the first and
  last 4 KiB, so undo can tell if that file changed. See [sweep](docs/sweep.md).
- **stash** moves files into a hidden holding folder and does not delete them.
  `stash pop` moves them back. See [stash](docs/stash.md).
- **unpack** extracts an archive into one new folder. If the extraction
  fails, it puts no files in place. See [unpack](docs/unpack.md).

## Install

You need Rust and Cargo. Get them from <https://rustup.rs>.

Install the published 0.5.5 releases. See
[Releases](https://github.com/risingmoonscholar/etudes/releases) for the notes.

```sh
cargo install --locked --git https://github.com/risingmoonscholar/etudes --tag sweep-v0.5.5 sweep-cli
cargo install --locked --git https://github.com/risingmoonscholar/etudes --tag stash-v0.5.5 stash-cli
cargo install --locked --git https://github.com/risingmoonscholar/etudes --tag unpack-v0.5.5 unpack-cli
```

The `--tag` flag pins each tool to a tagged version. Without `--tag`, you get
the current state of `main`.

To install from a clone of this repository, run
`cargo install --path crates/sweep-cli --locked`. For the other tools, use
`crates/stash-cli` or `crates/unpack-cli`.

Cargo puts the tools in `~/.cargo/bin`. To make sure that the tools are
installed, run `sweep --version`. It prints `sweep 0.5.5`.

## First use

```sh
sweep ~/Desktop                 # show the plan; nothing moves
stash ~/Desktop                 # move the folder's contents into a hidden holding folder
stash pop ~/Desktop             # move them back
unpack a.zip --list             # show what is inside; extract nothing
```

`sweep apply ~/Desktop --yes` scans the folder again and moves what it finds
then. It does not apply the plan you saw. To move only a plan you have seen,
see [Applying an observed plan](docs/plan-binding.md).

To see all the commands and options of a tool, run `sweep help`, `stash help`
or `unpack help`.

## Documentation

- [Documentation site](https://risingmoonscholar.github.io/etudes/): how to
  install, the docs for each tool, and recorded runs.
- [SECURITY.md](SECURITY.md): the security properties of the tools, and the
  properties that the tools do not have.
- [JSON and exit codes](docs/json-and-exit-codes.md),
  [undo and journals](docs/undo-and-journals.md) and
  [testing](docs/testing.md).

The help and some messages of the tools point to this README. These are the
pages that they mean:

- [The profile format for `sweep --scheme existing`](docs/sweep.md#use-folders-you-already-have).
- [The `ETUDE_JOURNAL_KEY` setup for undo without a login keychain](docs/undo-and-journals.md#encrypted-undo-without-a-login-keychain).
- [The size limit of `unpack`](docs/unpack.md#an-audited-transaction). The
  limit is soft. The largest measured amount above it was 16,777,215 bytes.

## License

Apache-2.0. See [LICENSE](LICENSE).
