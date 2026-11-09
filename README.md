<h1 align="center">Études</h1>

Études is a set of three command-line tools for files on macOS. Each tool
installs and runs on its own.

- **sweep** is a tool that sorts the files in a folder into groups. It shows
  you a plan first and moves nothing until you apply the plan.
  Sweep decides from filesystem and Finder organization metadata only; it never
  reads file contents to decide. When it moves a file it reads the first and
  last 4 KiB, so undo can tell if that file changed. See [sweep](docs/sweep.md).
- **stash** is a tool that moves files out of a folder for a time that you
  set. It does not delete the files, and it moves them back when you tell it
  to. See [stash](docs/stash.md).
- **unpack** is a tool that extracts an archive into one new folder. If the
  extraction fails, it puts no files in place. See [unpack](docs/unpack.md).

## Install

You need Rust and Cargo. Get them from <https://rustup.rs>.

**Caution: 0.5.5 is a release candidate, not a published release.** The
commands below use the 0.5.5 tags. They will work only after those tags are
published. For the published versions, see
[Releases](https://github.com/risingmoonscholar/etudes/releases).

```sh
cargo install --locked --git https://github.com/risingmoonscholar/etudes --tag sweep-v0.5.5 sweep-cli
cargo install --locked --git https://github.com/risingmoonscholar/etudes --tag stash-v0.5.5 stash-cli
cargo install --locked --git https://github.com/risingmoonscholar/etudes --tag unpack-v0.5.5 unpack-cli
```

To install this candidate from a clone of this repository, run
`cargo install --path crates/sweep-cli --locked`. For the other tools, use
`crates/stash-cli` or `crates/unpack-cli`.

The tools go in `~/.cargo/bin`. To make sure that the install worked, run
`sweep --version`. It prints `sweep 0.5.5`.

## First use

```sh
sweep ~/Desktop                 # show the plan; nothing moves
stash ~/Desktop --for 3d        # move the folder's contents aside; due in 3 days
unpack a.zip --list             # show what is inside; extract nothing
```

For the full usage of a tool, run `sweep help`, `stash help` or `unpack help`.

## Documentation

- [Documentation site](https://risingmoonscholar.github.io/etudes/): recorded
  runs of each tool, the `--help` output of each tool, and the exit codes.
- [SECURITY.md](SECURITY.md): the security properties of the tools, and the
  properties that the tools do not have.
- [JSON and exit codes](docs/json-and-exit-codes.md),
  [undo and journals](docs/undo-and-journals.md) and
  [testing](docs/testing.md).

The help and the messages of the tools refer to these three pages:

- [The profile format for `sweep --scheme existing`](docs/sweep.md#use-folders-you-already-have).
- [The `ETUDE_JOURNAL_KEY` setup for undo without a login keychain](docs/undo-and-journals.md#encrypted-undo-without-a-login-keychain).
- [The size limit of `unpack`](docs/unpack.md#an-audited-transaction). The
  limit is soft. The largest measured amount above it was 16,777,215 bytes.

## License

Apache-2.0. See [LICENSE](LICENSE).
