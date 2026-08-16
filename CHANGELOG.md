# Changelog

All notable changes to this project are documented here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Nothing has been tagged or published yet, so everything below sits under the
one version the workspace has ever carried. The Fixed list is long because the
adversarial harness and two independent reviewers were pointed at the tools
before anyone else could be.

## [0.3.0] - 2026-08-10

### Added

- `sweep`: analyses a folder by name signal and proposes groups. `apply`
  moves them, `undo` reverses, `verify` prints the tool's own privacy posture.
  Files that look like personal records are never moved, in any mode.
- `sweep review`: walk each group, rename or skip, then apply.
- `--inspect-content`: reads plain text files so sweep can refuse *more*
  files. Content findings can only widen refusal; they never name a group or
  affect where anything moves. Consent is asked for separately from `--yes`.
- `stash`: moves everything in a folder into one hidden holding directory and
  brings it back with `stash pop`. The deadline lives in the directory name, so
  there is no second state store to drift.
- `unpack`: one command for `.zip .tar .tar.gz .tgz .tar.bz2 .tar.xz .gz
  .dmg`, extracting into its own directory. Nothing is parsed in-process; the
  system tools do the work. Every archive is listed and judged before anything
  is written, and member paths that escape the target are refused.
- Encrypted undo journal (XChaCha20-Poly1305), keyed from the login keychain.
  There is no plaintext fallback: if sealing is unavailable the tool refuses.
  Journals are pruned after 30 days.
- `--json` on all three tools, for callers that are not people.
- `--version` on all three tools.
- A no-network witness: a symbol scan of the shipped binary, plus
  `scripts/no-network-test.sh`. This script runs the whole suite under a
  macOS sandbox profile that denies sockets. It proves the sandbox denies
  them before trusting the result.
- Apache-2.0 licence and NOTICE.

### Changed

- One repo, one shared core: `etude-core` holds the engine and has zero
  dependencies, which is what makes the no-network claim cheap to check.
  Content inspection lives in `etude-read` and the cipher in `etude-keep`.
- Journals are namespaced by tool, so `sweep undo` cannot reverse a `stash`.

### Fixed

- `sweep verify` claimed content inspection did more than it does.
- `mkfx` exits cleanly instead of panicking, and the repository URL is real.
- Symlinks are fingerprinted by the link, not by the target, so a link into the
  folder being emptied no longer reads as modified and refuses to restore.
- Directories and package bundles (`.app`, `.photoslibrary`) move as whole
  units instead of panicking on a hash that cannot be taken.
- Human-readable deadlines round rather than truncate, and a count of one reads
  as one.
- `stash status` in a folder with no stash now names the stash elsewhere that
  `stash pop` would restore, instead of reporting nothing stashed.
- A mistyped leading flag (`stash --dry-run`) is refused instead of being taken
  as consent to stash the current directory.
- A move within one device is a single atomic syscall (`renamex_np` with
  `RENAME_EXCL`) rather than `link` then `unlink`. A crash between those two
  used to leave one file reachable by two names, and undo could not clean it up
  without guessing whether the second name was sweep's or the user's.
- `undo` records each reversal as it makes it, so a killed run resumes where it
  stopped instead of walking everything again and never converging.
- `undo` takes an optional folder, so applying to two folders in a row leaves
  both reversible. Previously only the newest journal was reachable.
- A folder that cannot be read is reported instead of being silently dropped
  from a count that implied it was complete.
- A truncated or torn journal is refused rather than half-loaded, in both
  `sweep` and `stash`.
- Two filenames macOS considers identical are caught at plan time, and the tool
  asks the filesystem whether it folds case instead of assuming.
- `sweep` can reach an iCloud Desktop, and `--allow-sync` reaches apply rather
  than only the scan.
- `sweep forget` no longer destroys the key `stash` relies on.
- A misspelled scan flag is refused. `sweep PATH --explainn` used to print an
  ordinary scan, so the reader believed they were seeing `--explain` output;
  `--jsonn` returned prose to whatever was going to parse it.
- `etude-read` proves its buffer erase without reading freed memory, which was
  a segfault on Linux.
- Every command checks its flags, from one table beside the dispatch rather
  than a list per command. `sweep forget --frobnicate` used to destroy a
  journal on a typo, and `undo` and `verify` ignored flags outright. Single
  dash flags were ignored everywhere. A flag given to the wrong command names
  the command it belongs to.
- `--only` and `--depth` refuse a value that is another flag, instead of
  taking it as a group name or a depth and reporting the confusing result.
- `unpack` refuses a leading flag rather than reading it as an archive name,
  which makes the exit-code contract uniform across the three tools.
- The journal directory moved from `~/.local/state/etudes`, a Linux
  convention on a macOS-only tool, to `~/Library/Application Support/etudes`,
  the location Apple's own documentation specifies. Existing journals are
  moved forward automatically the first time any tool runs.
- The cross-device copy on macOS no longer relies on `fs::copy`'s
  undocumented platform mapping, which also propagated a file's own
  pre-existing extended attributes unnecessarily; it now requests only data
  and POSIX stat via an explicit `copyfile()` call.
- A same-device move on a filesystem without hard-link support (exFAT,
  FAT32) no longer fails outright; the fallback now handles that error the
  same way its sibling call site already did.
- A camera's own sequence number is no longer mistaken for a tax-form or
  identity marker when the filename is shaped the way a camera actually
  names one (DCF, JEITA CP-3461B).
