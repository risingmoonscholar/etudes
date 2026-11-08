# Structured progress

Every operation that takes time or writes takes one flag, `--progress-json`, and writes newline-delimited JSON records on stderr alongside normal diagnostics. The flag, the schema, the writer and the human milestones are one layer, `crates/etude-cli-support/src/progress.rs`. A tool supplies counts it observed and nothing else.

| Operation | Command | `planned` | `completed` | `journalled` |
| --- | --- | --- | --- | --- |
| `apply`, `review` | `sweep apply PATH --yes`, `sweep apply --plan`, `sweep review PATH` | entries after full preflight validation | successful moves or tag writes, including one whose done record failed | done records written to the journal; zero with `--no-journal` |
| `undo` | `sweep undo [PATH]`, `sweep undo --id ID` | journal entries | entries fully examined, including ones left alone or already restored | reversals this run recorded |
| `stash` | `stash PATH`, `stash --plan`, `stash select` | entries after full preflight validation | successful moves | done records written to the journal; zero with `--no-journal` |
| `pop` | `stash pop`, `stash restore`, with `--id`; one with `--item` | journal entries, or one | entries fully examined | reversals this run recorded |
| `extract` | `unpack ARCHIVE` | distinct members of the judged listing, without metadata unpack drops | planned members seen in private staging, see below | zero |
| `list` | `unpack ARCHIVE --list` | members in the plain listing | the same, once the listing returned | zero |

`--json` still writes one result envelope on stdout, unchanged by the flag. Records and the envelope share an operation ID. Without the flag, the same layer writes the human milestones to stderr: one updating line on a terminal, at most ten lines when redirected, and nothing below 100 items. Stdout carries neither. Other commands refuse the flag.

## Schema 1

A record has exactly these fields: `schema_version`, `event` (`progress`), `tool_version`, `operation_id`, `operation`, `phase`, `planned`, `completed` and `journalled`. Phases are `planning`, `advancing`, `done` and `error`. Records contain no paths, contents, keys or environment values. Any stderr line that is not a JSON object with `event` `progress` is a diagnostic, including what a system extractor prints.

The version is still 1: no field, type or phase changed, and sweep's stream is the same for the same run. Two things are newly defined, and neither reaches a run that schema 1 could already describe:

- **Not known yet.** `planning` with `planned` zero means work has started and the count is not known. Schema 1 never wrote that combination, and `planned` has always counted what had been validated so far. A count of nothing is `done` with `planned` zero, and a failure before any count is `error` with `planned` zero. No sentinel appears in `completed`.
- **A terminal record agrees with the result.** `done` means the result status is `done` or `nothing_to_do`, and anything else is `error`. An apply that reached the end of its plan but held items back, whose result reads `incomplete`, used to end in `done`. A pop that left a changed item exits 3, reads `incomplete` and ends in `error`.

There are at most twelve records per run: one `planning` with a count, at most ten `advancing` milestones and one terminal. A run that starts indeterminate spends one milestone on that record. Each record is written whole in one call and is shorter than the 512 bytes POSIX guarantees a pipe writes atomically, so a kill cannot tear one and an extractor sharing the stream cannot land inside one. Twelve short records also cannot fill a pipe, so a reader that never reads cannot stall a run through this layer.

Records begin once work begins. A refusal before that, such as a missing archive, a `.dmg` or a bad argument, uses the usual diagnostics and result envelope and writes no record. Unpack begins work when it starts reading the archive, so a refusal it reaches after that, such as a destination that exists or an unsafe member, writes `planning` with `planned` zero and then `error`. A run that has nothing to do writes one terminal record with zero counts.

## What a killed run leaves

Every record describes something that had already happened when it was written, so nothing written before a kill can be untrue afterwards. A killed run has no terminal record: the absence is how a reader knows the outcome was never reported, and the records say only how far it got. SIGTERM to unpack ends in `error` with the last observed count. A recovered panic ends a started stream in `error` with its last counts.

For apply, the journal base is saved before any move. Each move's done record is written before the callback and before the next move begins, and ordered behind its move with a barrier sync, not a full flush. One full flush ends the run, before it reports success. After a power cut, every surviving move except the newest has its record. The newest move may lack one, or a record may survive without its move, and undo reconciles both. Tests reload the sealed journal during callbacks and check that later sources remain untouched. Output errors do not change this order. Undo records each reversal the same way before counting it.

## Unpack's count

Unpack runs system tools and does not parse what they print while they run. Its count comes from what it can already see:

- `planned` is the listing it already judged: the typed and plain listings it reads before extracting, reduced to distinct normalized paths, without the metadata it removes anyway. Until that judgement passes, the count stays unknown: a listing of a large compressed tar has to decompress the whole archive, twice.
- `completed` comes from the walk of private staging that the soft size limit already makes every 100 ms. A name counts only if it is on the plan, so parents the extractor creates implicitly, temporary names and anything unexpected can never raise it.
- A member exists before it is finished: unzip and gunzip write in place, and unpack itself creates a bare gzip's output before gunzip starts. While the extractor runs, the newest present member is therefore not counted. This assumes the extractor writes one member at a time, which is how `unzip` and `tar` were observed to behave, not something their manuals promise. After a successful exit everything present counts.
- The count can end below `planned`, with `done`: names a volume folds together (`Readme.txt`, `README.TXT`) and names a listing escapes or that are not UTF-8 are not found. It reports what was found, not what was hoped.

This adds no system call, no read of file contents and no contract domain: `staging_tree_metadata` already covers the walk, and receipts list the same categories with and without the flag. A hash lookup per entry is the whole addition.

Original branch measurements, before the tar-rename fix now on `main`: 100,000 files of 4,096 bytes in 100 folders, wall time of a complete run, median of five, on one APFS volume. These measurements were not repeated on the current code:

| Archive | System tool alone | Unpack before this change | Unpack, no flag | Unpack, `--progress-json` |
| --- | --- | --- | --- | --- |
| stored ZIP | 9.53 s | 14.40 s (13.97 to 14.59) | 14.55 s (14.05 to 14.77) | 14.20 s (13.98 to 14.90) |
| tar, 634 MiB | 28.11 s | failed on all five runs | 36.00 s (35.49 to 37.17) | 35.99 s (35.50 to 36.39) |

The ZIP difference is inside the spread of the runs in both directions, so no slowdown from progress could be measured. Unpack's time above the system tool also pays for the private copy, both listings, the audits and the walk, none of which this change adds. The tar row has no before figure because a large tar could not extract: see the changelog.

## Reading it to drive a bar

`phase` `planning` with `planned` zero: indeterminate. Otherwise the fraction is `completed / planned`, and it can sit at 1 while audit and publication finish, because `done` is written only after them. A single large member reads 0 of 1 until it finishes, so a small `planned` should be drawn as indeterminate. Take the outcome from the last record and the exit code, and treat a stream that ends without a terminal record as a killed run.

## Adding a tool

A new tool calls `Reporter::for_operation(label, operation)`, feeds it `StructuredProgress` values it observed, and calls `finish(exit_code)`. `indeterminate()` covers the time before it can count. It declares its operations through `structured_progress` in the contract, and its counts belong in the table above. A tool that cannot observe members while a system tool runs writes `planning` and the terminal record and no `advancing`. Bytes are not in schema 1: nothing on main has a denominator for them that does not come from an archive header. A byte count would need schema 2.

`scripts/check-progress-records.py` is the independent reader used by every tool's tests and by `stress/scenarios/113-shared-progress.sh`. `scripts/check-progress-mutation.sh` plants faults that those tests must reject.
