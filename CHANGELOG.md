# Changelog

All notable changes to this project are documented here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Nothing has been tagged or published yet, so everything below sits under the
one version the workspace has ever carried.

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
