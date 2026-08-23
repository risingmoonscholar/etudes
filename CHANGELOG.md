# Changelog

All notable changes to this project are documented here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

The Fixed lists are long because the adversarial harness and two independent
reviewers were pointed at the tools before anyone else could be.

## [Unreleased]

## [sweep 0.5.2] - 2026-08-23

From here the tools version separately. sweep is the one maturing; stash and
unpack are static and keep 0.5.1 until they themselves change. A sweep-only
release is tagged `sweep-vN` so the tag says what moved.

### Added

- **`--map EXT=Folder`** (repeatable): an agent -- or a person -- routes an
  extension sweep has no rule for into a named folder, this run only. Maps
  run last and claim only files every built-in pass declined: no map outranks
  a refusal, the personal/project/grace/in-flight holds all win, and the
  three-file floor applies so `--map` cannot be a one-file move command.
  All-or-nothing validation; `--map` with `--no-journal` is refused outright.

  If the folder does not already exist, the output says so in as many words:
  *the folder name "X" was chosen by your agent, not derived from your
  files*. That is the one sanctioned exception to the rule that destination
  names come from the filesystem, and it announces itself every time.

- **`unknown_extensions`** in `--json` under `left_alone`: counts per
  extension among the files sweep had no rule for. The agent's eyes --
  it sees the shape of what sweep declined without another copy of the names.

- The scan summary line now ends with `· sweep 0.5.2`, so a screenshot
  identifies its own version. A tester's stale build produced folders no
  current binary can produce, and nothing in his screenshot could say why.

## [0.5.1] - 2026-08-21

### Fixed

- **A loose project document no longer takes the whole folder hostage.** The
  first professional to test sweep ran it on a Downloads folder holding one
  stray `.flp` and the entire sweep exited 2 -- the file had merely been
  downloaded there, but it went through the same top-level check as
  `Cargo.toml`, which genuinely does mark a project root.

  The two marker kinds now part ways at the scan root. A ROOT marker
  (`project.godot`, `Cargo.toml`, ...) still refuses the folder whole: it
  marks a project by definition. A DOCUMENT marker (`.flp`, `.blend`,
  `.als`, ...) holds instead: the document stays put, and so does every file
  in the families it could reference -- Media, Images, Scripts, Data --
  disclosed with the document's name. Documents, Archives and Installers
  beside it still sort.

  The reference surface IS the protection, so a flat project folder (an
  `.als` beside its bounces, no subfolders) keeps its layout too: every
  bounce is Media and every bounce stays. The refusal it replaces protected
  that case by locking the tester's Downloads folder.

  Screenshot-named files are exempt from the hold. macOS puts screenshots on
  the Desktop, the folder most likely to also hold a stray project file, and
  a project referencing "Screenshot 2026-08-12 at 9.15.11 AM.png" is not a
  real layout.

  Two new `--json` keys under `left_alone`: `project_documents`,
  `held_near_document`. Child directories are unchanged -- a folder holding
  either kind of marker is still stepped over.

## [0.5.0] - 2026-08-20

Sweep refuses more than it used to, and group names changed, so scripts
written against 0.4.0 need reading before upgrading. See **Migrating** at the
end of this entry.

### Added

- **Sweep steps over your projects.** A folder holding a project file --
  `project.godot`, `Cargo.toml`, `package.json`, `.flp`, `.blend`, `.ptx`,
  `.als`, 25 markers in all -- is left alone rather than sorted. A Godot
  `.tscn` references its siblings as `res://scripts/main.gd`, absolute from
  the project root, so moving any file inside one breaks every reference to
  it. The list was read off real projects on a real disk; an earlier version
  assembled from vendor documentation was missing `project.godot`, and a real
  18,724-file Godot project produced a plan.
- **Bundles are stepped over as a unit.** `.fcpbundle`, `.band`, `.logicx`,
  and anything macOS reports as a package including a Pages document. The
  folder holding one is ordinary and still gets swept -- a year of invoices
  beside a video library is not made unsweepable by the library.
- **Downloads still arriving are held back**, as files (`.part`,
  `.crdownload`, `.download`) and as directories -- Safari writes
  `movie.mp4.download/` with the partial data inside it.
- **A grace window.** Anything changed in the last day is left alone: a file
  you are working on right now is not a file to be filed. Keyed on mtime and
  never atime, because Spotlight and Time Machine touch atime just by looking,
  which would freeze a folder permanently.
- `sweep --since N[h|d]` sets that window. `--since 0` turns it off. An
  unreadable value is an error rather than a silent fallback to the default:
  a wrong window changes which files are held back, so `--since 6hh` must not
  quietly produce the ordinary 24h result.
- Three counts in `--json`: `projects_skipped`, `downloads_skipped`,
  `packages_skipped`. Additive; nothing was removed.

### Fixed

- `sweep apply` and `sweep review` say what they left behind. `apply` reported
  "Moved 4 files." over a six-file folder with nothing explaining the other
  two, so the only available reading was that sweep tried them and failed.
  The plain scan and `--json` were already correct, which meant the
  agent-facing output was honest and the human-facing one was not.
- A flag before the path no longer sends the scan somewhere else.
  `sweep --depth 2 ~/Downloads` scanned the *current* directory, never looked
  at the path, and exited 0 with no warning -- a confident report about a
  folder the user did not name. `sweep review --depth 2 DIR` took `2`, the
  value of `--depth`, as the path. `apply` was correct, and the fix that made
  it correct had never reached the other two.

### Migrating from 0.4.0

- **Group names changed** in the same cycle. `--only acme` and anything else
  scripted against a coined name stops working; groups are now the seven type
  families listed under Unreleased above.
- **Some applies that used to move files now move fewer, or none.** A folder
  of fresh downloads can produce exit 1 where 0.4.0 would have sorted it. Pass
  `--since 0` for the old behaviour.
- **A scan pointed at a project folder now exits 2** rather than proposing a
  plan. That is the point of the release, but a script that treated exit 2 as
  a crash needs updating: 2 has always meant refused, distinct from 3 for
  error.

### Known limits

- A project *document* -- `.blend`, `.flp`, `.als` -- references its assets
  relative to itself and freely upward. Stepping over the folder holding one
  is right for a `project.godot` that marks a project root by definition, and
  not enough for a `.blend` in `scenes/` pointing at `../textures/`. Filed as
  #49 with four reproductions. The complete rule was built and rejected: one
  `.flp` made an entire Downloads folder unsweepable.
- A Final Cut library configured to keep media outside the bundle cannot be
  protected without reading the library's own database, which sweep does not
  do.

### Changed

- Groups are named for what files **are**, not for words they mention. Seven
  type families -- `Images` `Documents` `Scripts` `Installers` `Archives`
  `Media` `Data` -- decided by extension.

  `Data` is tabular only: csv, tsv, xlsx, xls, parquet, numbers. json and
  plist are configuration more often than data, and `sqlite` travels with
  companion files, so all three are left alone. `Scripts` is interpreted
  files only, and is named that rather than `Code` because the name marks
  the boundary: compiled-language source is not a script and does not
  belong in an automatic move.

  The rule this replaces grouped any word appearing in five or more filenames
  and named the folder after it, guarded by a hand-written stoplist. On the
  first real Downloads folder it met it produced a folder called `apple`, out
  of a receipt, an agreement, a script and an export that shared a word.
  Frequency is not category, and the stoplist could not have saved it: the set
  of words that are not categories is the whole vocabulary minus a few dozen.

  Its lifetime record on data this project did not author was zero true
  positives and one false positive; both groups it produced in the fixture had
  been planted to demonstrate it.

  Group names are the tool's public vocabulary, so `--only acme` and anything
  else scripted against a coined name stops working.

- Extensions the table does not know are left where they are rather than swept
  into a catch-all. The table is the whole mechanism -- the OS is not
  consulted -- and unknown means untouched: filing formats sweep cannot name
  would mean maintaining a list of every app's private extension forever.
- `.app` bundles are left untouched. An application in Downloads is not an
  installer, and moving one under a folder named `Installers` would be an
  inference about intent rather than a fact about type.

## [0.4.0] - 2026-08-18

Exit codes moved, so this is a minor bump rather than a patch. Anyone who
installed from git during 0.3.0's long tail has a binary that behaves
differently from one installed now, and both reported the same version -- which
is the reason for tagging from here on.

### Added

- `unpack --max-size N[G|M]`: raise the bound on what one extraction may write.

### Changed

- `unpack` stops an extraction that writes more than half the free space on the
  target volume, and removes what it wrote. The bound is on bytes landing on
  disk, never on the size an archive declares: forging four bytes makes
  `unzip -Z`, `tar -tvf` and `gzip -l` each understate a member by three orders
  of magnitude, and the forged zip then extracts in full with `unzip` exiting 0.
  A large legitimate archive and a decompression bomb are the same event to this
  check; it bounds damage rather than detecting intent.
- `unpack` refuses `.dmg` by design rather than as an unimplemented case, and
  exits 2 (refused) rather than 3 (error). Opening one means asking the kernel
  to mount a stranger's filesystem image, which no size check covers. The
  message names `hdiutil attach` so the decision stays with the user.
- `stash pop` exits 0 rather than 3 after successfully restoring from a journal
  whose tail was cut. A cut tail is the ordinary outcome of any interrupted run,
  so exiting non-zero made routine crash recovery read as failure to any script
  checking the code. The damage is still disclosed on stderr.

### Fixed

- A journal whose final record was cut short keeps the records before it, and
  says its tail was lost. Voiding the whole file over a partial frame stranded
  every file the intact records described; one CI failure lost 130 that way over
  a 4-byte tail. A frame that is complete but fails to authenticate is alteration
  rather than an interrupted write, and is still refused.
- A journal missing *several* records restores nothing and says so, on every
  route that looks for one. One unrecorded move is a crash between the move and
  its record and is recoverable; several means records were lost, and reversing
  only the reachable ones would strand the rest under an exit code that reads as
  success.

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
- A journal whose final record was cut short keeps the records before it, and
  says its tail was lost. Voiding the whole file over a partial frame stranded
  every file the intact records described; one CI failure lost 130 that way
  over a 4-byte tail. A frame that is complete but fails to authenticate is
  alteration rather than an interrupted write, and is still refused.
- A journal missing *several* records restores nothing and says so. One
  unrecorded move is a crash between the move and its record and is
  recoverable; several means records were lost, and reversing only the
  reachable ones would strand the rest under a success exit. Both `sweep` and
  `stash`, on every route that looks for a journal.
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
  the location Apple's own documentation specifies. An existing journal is
  moved forward automatically the first time `sweep` or `stash` runs.
