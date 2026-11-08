# Changelog

All notable changes to this project are documented here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

The Fixed lists are long because the adversarial harness and two independent
reviewers were pointed at the tools before anyone else could be.

## [Unreleased]

- **Safety fix (sweep, stash):** on macOS a move now takes the item from the folder that was checked, not from whatever its path leads to. Before, only the destination folder was held this way, and a source folder swapped for a link to somewhere else just before the move made the move take a file from that other place. The source folder is now opened without following a link and checked against the device and inode recorded before the first move, the item is checked inside it, and the move, its link-then-unlink fallback and the copy to another disk all go through the opened folders. Undo holds the folder it takes each file back out of the same way. A refused move stops with `plan_stale`, as a changed destination folder does, and says `FOLDER was moved or replaced while sweep was working, so nothing more was moved out of it` (stash says the same), or `so it was not moved` when the item itself was replaced. The library error reads `refused: source folder identity changed or was unavailable: PATH; nothing moved for this file`. Files moved before the swap stay moved, and the message gives the undo step as before. The Finder tag check and the locked-flag check still read by path: macOS cannot read a tag through a folder descriptor. On other systems the source folder and item are checked by path just before the move.
- **Safety fix (sweep):** when `sweep undo` could not open an undo record with the current key, it said to keep that record, because the right key opens it again. But 30 days after the record was written, the next sweep run deleted it anyway, and `sweep undo` then said there was nothing to undo while the files were still moved. Sweep now drops a record older than 30 days only if it opens with the current key. A record that does not open is kept, so the right key can still undo it; `sweep forget` still removes it. Old records are now dropped only by a run that already uses the key for its own work: `sweep apply`, `review`, an exported-plan apply, or `sweep undo`. A scan, `status` or `verify` no longer drops records and never reads the key to do it. So a record past 30 days is dropped by the next sweep run that opens records. An apply drops them before it moves anything. An undo drops them after the undo, so the run that brings the right key still finds the record. This also means an old record that opens can be undone if that undo is the first apply or undo after its 30 days.
- **Safety fix (stash, sweep):** a stash kept longer than 30 days (`stash --for 6w`, or one left overdue) lost its undo record the next time sweep ran, because sweep dropped every record older than 30 days, from any tool. `stash pop` then said there was no stash, while the files sat in the hidden `.stash-` folder. Sweep now leaves stash records alone, and a stash record is kept for as long as its stash holds anything. Once a stash is popped, its record expires 30 days after the pop: `stash pop` removes old records that open with the current key and hold nothing, and says how many on stderr. It does so after the pop, so `stash pop --id ID` of a stash popped more than 30 days ago still says it was already popped; the next pop then answers that the id is unknown. A stash record that does not open with the current key is never removed by the tools, because it may still restore files: not by `stash pop`, sweep, or `sweep forget`. `stash status --all` names it as unreadable. `sweep forget` now says how many stash records there are, why they are kept, and prints the `rm` step to remove one by hand once its files are back. Without `--yes` it then stops before destroying the shared key, as before. With `--yes` it says the same, and before it destroys the keychain key it says that those stash records will no longer open and their files stay in the hidden `.stash-` folders, and to pop each first to keep them. With `ETUDE_JOURNAL_KEY` set, forget destroys no key and says so. Sweep's own records still expire after 30 days. Capability declarations are now version 16 (sweep) and 14 (stash).
- **sweep:** a folder held back only because its files are too recent still reports `nothing_to_do`, and the reason now says retry `later`, on both a scan and `sweep apply`, when those files would form a group once the window passes. Hidden files and links in the folder do not change that. An empty folder, one held for any other reason, a file that would not group, or a file whose modified time cannot be read keeps `not_needed`. The words match: "Run again later" prints only when the reason says `later`, and a lone recent file is told it would not form a group on its own.
- **sweep:** Finder aliases are skipped. Sweep reads only the alias flag in the FinderInfo attribute, never resolves the alias and never reads its contents. The plan gains `details.skipped.aliases_skipped` and the summary gains one line. The sweep contract is version 13 and the stash contract is version 12. The plan file gains an optional section for a nonzero alias count; other plans encode as before.
- **Tests:** a process killed between the hard link and the unlink of the link-then-unlink move recovers on undo, with one copy at the original path and the original bytes.
- **sweep, stash:** files and folders with the locked flag (Finder's Locked box, `chflags uchg` or `schg`) are left in place and counted under `skipped.locked_skipped`. They are never moved or tagged. Apply checks again before each move and counts an item locked after the scan as `held_locked`. A locked root or destination folder refuses the plan as `plan_stale` before any move. Every message says how to clear the flag: the `schg` flag gets its own step, because only an administrator can clear it. `plan_stale` now says the plan cannot run as made: clear any lock, then plan again.
- **stash:** a folder with a locked item in it is not called clear. Stash moves the rest, says the folder is NOT clear, and exits 3 with status `incomplete`. A folder that holds only locked items moves nothing, so its status is `refused`; it also exits 3 with `permission_denied`.
- **unpack:** a later extraction into the same destination removes a folder left by a killed extraction, and says so by path on stderr. It does so only after checking the exact name, that the folder is a real directory this user owns, that its lock file names that very folder by device and inode, and that the lock is free. A running extraction holds the lock, and so does an extractor that outlives a killed unpack. A symlink, a copy of a leftover, a folder from an older version and anything else unproved stay in place, and each is named with a step that works. A name that only resembles a leftover is not touched or mentioned. A folder under a read-only parent keeps its contents, and the printed step makes the parent writable first. A failed removal does not block the extraction. `--json` adds counters `leftover_folders_removed` and `leftover_folders_kept` under `effects` when not zero. A volume without advisory locks extracts as before; its leftovers are never removed. SIGKILL itself still leaves the folder until a later run. Unpack capability declaration version 8 adds these reads and this deletion. See [staging recovery evidence](docs/release-evidence/unpack-staging-recovery.md).
- **stash:** `stash select` of only a locked item moved nothing but read `incomplete`. It now reads `refused` with `permission_denied`, the same as a folder holding only locked items; it still exits 3. A stash whose every item was held at move time also reads `refused`.
- One shared progress layer for every tool. `--progress-json` now also covers `sweep undo`, `stash`, `stash select`, `stash pop` and `restore`, and `unpack` extraction and `--list`, with the same flag, schema and writer, and human milestones come from the same layer. The schema stays at version 1 and sweep's records keep their fields. `planning` with `planned` zero now means the count is not known yet, a combination the schema never wrote. The terminal record now agrees with the result: an apply or pop whose result reads `incomplete` ends in `error`, where an apply ended in `done`. `stash`, `stash select`, `stash pop`, `restore`, `sweep undo` and `unpack` used to refuse `--progress-json` with exit 2 and now accept it. Capability declarations are now version 14 (sweep), 13 (stash) and 8 (unpack); progress adds no read or write domain. See [structured progress](docs/progress.md).
- `unpack` reports progress without parsing extractor output: planned members from its judged listing, found in private staging by the walk its soft limit already makes, with the newest not counted until the extractor exits. Redirected runs of 100 or more members now print the shared milestones on stderr. On 100,000 files of 4,096 bytes, the median of five complete runs of a stored ZIP was 14.40 s before, 14.55 s after without the flag and 14.20 s with it, inside the spread of the runs.
- On macOS each journal record is ordered behind its move with a barrier sync instead of a full drive flush, and a finished apply or undo is flushed once (issue #12). On 9,999 small files apply measured 50.3–56.7 s before and 19.4–21.3 s after, undo 43.2 s before and 4.6–9.4 s after. Done records still reach the disk in the order of their moves. After a power cut, every surviving move except the newest has its record; the newest move may lack one, or a record may survive without its move, and undo reconciles both.
- **sweep, stash:** a folder, extension, scheme or duration you typed is shown as you typed it in messages, not as `"Blend\"er"` with debug quotes and backslashes. Control characters and line breaks in a name still show as `?`. This covers `--map` refusals, tag names, `--since`, `--depth`, `stash --for` and proposal files.
- **sweep:** a routes profile that starts with a UTF-8 byte order mark, which some macOS editors write, is read. It used to be refused as "the file needs one schema_version = 1 entry". A file holding only the mark is refused the same way as an empty file. Saving routes keeps the mark and every other byte you wrote.
- **sweep:** `sweep undo` of an apply that saved routes says the routes stay in the profile, names the profile and says to delete a route's `[[routes]]` block to drop it. The journal does not record that routes were saved, so the line prints when the profile holds a route for an extension and folder that this undo restored.
- Clearing a leftover routes-profile pending file never removes a file that has another hard link. Docs now say that journal records are ordered behind their moves, that a power cut can leave the newest move without its record and that undo reconciles it, and `SECURITY.md` states that code running as you can forge unpack's staging marker.

- **sweep, stash:** a move that failed after its copy to another disk landed, or after its second name was made, could leave that copy in the destination while the message said "Nothing was moved", and undo could not see it. The failed move now removes its copy before it reports. The original is never removed first. A copy is removed only while it is still the file the move made. If the copy cannot be removed, or the original vanished during the move so the copy is the only one, the copy stays. The message names it, says undo cannot see it, gives the `rm` step for when the original is still there, and says to keep it when the original is gone. It says this whether or not other files moved first, and `--json` adds `details.copy_left`.
- An apply or tag apply that stops with an error now flushes the journal records it wrote, as a finished run does.
- **sweep:** `--scheme existing` now creates a saved route's folder when it is missing, the same way as a `--map` folder. Scripts no longer get exit 2 `profile_invalid` for a missing route folder. Sweep creates the folder only when the run moves at least one file into it. The plan lists it before anything moves: the text says "sweep creates the folder NAME for your saved route", with a name that starts or ends with a space in single quotes, and the group's signal, in text and `--json`, reads `.EXT (saved route, new folder sweep creates)`. The journal records the folder, and undo removes it if it is empty after the files go back. If an apply stops after some files moved, the message names each folder it created and says undo removes it once it is empty. A route whose extension has no files in the folder is ignored and creates nothing. A file or a link that holds a route's folder name still refuses with `profile_invalid`; the message names it without quotes, says nothing moved, and gives one step.
- **sweep:** a saved route now moves any number of matching files, one or two included. Before, fewer than three stayed where they were, and the plan listed them as having no group (`no_clear_group_paths` and `unknown_extensions` in `--json`) with advice to read `sweep propose --help`. A routed file still never goes to a built-in group.
- **sweep:** the three-file floor now applies only when sweep would make a new folder. When a built-in group's folder, such as `Documents`, is already a direct folder of the selected directory, one or two files of that kind move into it. Before, they stayed. A `--map` folder that is already there works the same way. Two files with no folder of their own still stay. When a link or a file holds the folder's name, the line says so and that sweep moves nothing into it. Otherwise the plan now adds a line such as "2 files would go to Documents: sweep makes a new folder only for 3 or more files, and moves any number into a Documents folder that is already here". At `--depth 2`, files already inside such a folder are reported as in place; before, apply refused because a file of that name was already there. `--save-routes` now says each route needs at least one eligible file, not three.
- The sweep capability declaration is version 17 on top of version 16. Exported plans mark a saved route's group with a new tag (8). A plan written by an earlier version still reads. A plan that holds a saved-route group needs this version: an earlier sweep cannot read it and refuses it as an invalid or unsupported plan binding.

## [0.5.4] - 2026-11-04

sweep, stash and unpack.

### Added

- `sweep undo --id ID` reverses one apply. It opens only that record, so a record that will not open cannot block it, and it reaches an older apply of the same folder. It uses the same checks as `sweep undo`, so it never overwrites a file.

### Safety fixes

- A failed read of the login keychain no longer counts as "no key". It used to create a new key that replaced the real one, which made every undo record unreadable. Only the keychain's "item not found" answer means first run now.
- A relative path such as `unpack a.zip` now gets the same free-space floor and size limit as an absolute path.

### Error messages

Every failure now says what failed, whether anything moved, and a next step that works when you run it. Printed undo and pop steps name the folder or the id, so they cannot act on other work. Exit 0 means the whole request was done. `schema_version` stays 4.

- **A wrong step is gone.** A failed apply that moved nothing used to say `sweep undo` reverses what did happen, and following it could reverse an earlier apply of another folder. It now says nothing was moved and there is nothing to undo.
- **A crash is gone.** `unpack` on a read-only destination panicked. It now names the folder and exits 3 with `permission_denied`.
- **`unpack`:** a bare relative path such as `unpack a.zip`, or a relative `--into`, skipped the 256 MB free-space floor and took the 64 GB ceiling as its size limit, because the free-space check failed on an empty folder name. It now gets the same floor and limit as an absolute path.
- **`unpack`:** options may come before the archive. `--max-size 0` is refused. Damaged, empty and password-protected archives each get their own message, and the leftover folder is named in full if it cannot be removed.
- **`sweep`:** a scan says when a file holds a group folder's name and does not offer an apply that would refuse. It still exits 0, and `--json` adds `details.blocked_folders`.
- **`sweep undo`, `stash pop`:** when files are not where they were left, the message covers both a moved folder and a deleted one. `sweep undo` with no folder and `stash pop --latest` keep stopping at that record, so they also point to another folder's apply or another stash. A record that cannot be opened no longer claims to know where the files are.
- **`sweep undo`, `sweep undo FOLDER`, `sweep status FOLDER`:** the refusal for a record that will not open now lists the records that do, newest first and up to five, each with its folder and its `sweep undo --id ID` step, and says how many more exist. If none opens, it says so and prints no step. `--json` adds `details.records_that_open`.
- **`sweep status`:** each row's step is now `sweep undo --id ID`, so it reverses that row and not another apply of the folder. `sweep apply` prints the undo record id.
- **`stash`:** refuses your home folder unless you pass `--allow-home`.
- Folder names you typed are shown in full. Names of files the tool found itself stay hidden.
- Singular and plural counts are right in all three tools.

New reason codes: `journal_write_failed`, `profile_invalid`, `archive_unreadable`, `size_limit_exceeded`, `extraction_stopped`. `journal_unreadable` and `journal_key_unavailable` now have retry class `after_change`, not `later`: waiting does not fix a wrong key or a damaged record.

**Breaking for scripts and agents.** Each line is old -> new.

| Command and condition | Exit | Status or reason |
|---|---|---|
| any apply or stash failure that moved nothing | unchanged | status `incomplete` -> `refused` |
| undo or pop that leaves any item behind, including files that are gone | 0 -> 3 | `done` -> `incomplete`, `operation_incomplete` |
| `sweep status PATH`, missing folder | 1 -> 3 | `nothing_to_do` -> `error`, `missing_input` |
| `stash status PATH`, missing folder | 2 -> 3 | `refused` -> `error` |
| `stash pop PATH`, missing folder or a file | 1 -> 3 | `nothing_to_do` -> `error` |
| `sweep status`, `details.command` | unchanged | `sweep undo 'FOLDER'` -> `sweep undo --id ID` |
| `sweep undo FILE`, `sweep status FILE`, `stash status FILE` | 1 -> 3 | `nothing_to_do` -> `missing_input` |
| `stash select --into`, missing folder | 2 -> 3 | `unsafe_input` -> `missing_input` |
| `sweep review` with no terminal | 0 -> 2 | refused |
| `sweep apply --only NAME`, unknown group | 1 -> 2 | `nothing_to_do` -> `invalid_arguments` |
| stray word on `sweep apply`, `undo`, `forget`, `verify`, `version`, or a scan | 0 -> 2 | `invalid_arguments` |
| `sweep PATH --scheme existing`, no profile | 0 -> 2 | `profile_missing` |
| bad `ETUDE_JOURNAL_KEY`, every command | always 2; `sweep status` 3 -> 2, `stash status PATH` 0 -> 2 | `journal_key_unavailable` everywhere |
| undo record cannot be saved | unchanged | `journal_unreadable` or `operation_incomplete` -> `journal_write_failed` |
| `--plan` file missing, `stash` | 2 -> 3 | `plan_stale` -> `missing_input` |
| `--plan` file unreadable | 2 -> 3 | `stash`: `plan_stale` -> `permission_denied` |
| `--plan` empty, a folder, another tool's plan, a mistyped digest; `stash --plan --for` | unchanged | `plan_stale` -> `invalid_arguments` |
| `--export-plan`, parent folder missing | `sweep` 2 -> 3 | `permission_denied` -> `missing_input` |
| `--export-plan`, file exists | `stash` 3 -> 2 | `permission_denied` -> `destination_exists` |
| `--export-plan`, inside the scanned folder | `stash` 3 -> 2 | `permission_denied` -> `invalid_arguments` |
| `--export-plan`, disk full | `sweep` 2 -> 3 | `permission_denied` -> `insufficient_space` |
| apply on a full disk or read-only disk | unchanged | `operation_incomplete` -> `insufficient_space` or `permission_denied` |
| `sweep forget` with a supplied key | 2 -> 0 | undo records were removed, so it is not a refusal |
| repeated `--for`, `--into` or `--max-size` | 0 -> 2 | `invalid_arguments` |
| broken or malformed profile, or a route in it | unchanged | `profile_missing` or `invalid_arguments` -> `profile_invalid` |
| `--map` with `--scheme existing` | unchanged | `profile_missing` -> `invalid_arguments` |
| `sweep propose`, folder absent | unchanged | `profile_missing` -> `missing_input` |
| `sweep apply --plan`, profile changed | unchanged | `profile_missing` -> `plan_stale` |
| `stash` on a download folder | unchanged | `unsafe_input` -> `download_in_flight` |
| `stash select --from0 FOLDER` | 3 -> 2 | `internal_error` -> `invalid_arguments` |
| `stash status a b` | unchanged | `missing_input` -> `invalid_arguments` |
| `stash status --paths` without `--all` | 1 -> 2 | `invalid_arguments` |
| `stash pop --id ID --item NOPE` | 1 -> 2 | `nothing_to_do` -> `unknown_item` |
| `unpack`, damaged or not an archive | unchanged | `unsafe_input` or `internal_error` -> `archive_unreadable` |
| `unpack`, empty ZIP | 3 -> 1 | `unsafe_input` -> `nothing_to_do` |
| `unpack`, password-protected | 3 -> 2 | `internal_error` -> `unsupported_format` |
| `unpack`, `.rar`, `.7z`, `.pkg` or no suffix | 3 -> 2 | `error` -> `refused` |
| `unpack a.zip.part`, `.crdownload` | 3 -> 2 | `unsupported_format` -> `download_in_flight` |
| `unpack`, over the size limit | unchanged | `insufficient_space` -> `size_limit_exceeded` |
| `unpack`, stop signal | unchanged | `operation_incomplete` -> `extraction_stopped` |
| `unpack --into`, parent folder missing | unchanged | `internal_error` -> `missing_input` |
| `unpack`, destination is a broken link | unchanged | `internal_error` -> `destination_exists` |
| `unpack`, archive is a folder | unchanged | `unsafe_input` -> `missing_input` |
| `unpack`, read-only destination | panic -> 3 | `internal_error` -> `permission_denied` |

### Other changes

Journal append reuses one handle per apply and writes each frame together; release runs on 10,000 small files (9,998 moves) measured apply 74.83/101.72 s before and 60.07/55.56 s after, undo 65.91/79.88 s before and 99.22/55.82 s after.
- **sweep, stash:** a folder the system will not let the tool read now exits 3 with `permission_denied`, names the folder, and says what to do next: macOS privacy settings or file permissions, depending on the error. Before this, `stash` said the folder was already clear, `sweep review` said it needed no organising, and `sweep --export-plan` said "plan has incomplete observations; replan required; replan required". For scripts and agents: a scan of a folder that cannot be read at all used to exit 1 with status `incomplete`, and now exits 3 with status `error`. The reason stays `permission_denied`.
- **stash:** a folder is no longer called clear when an item in it could not be read. Stash says the item stayed and exits 3.
- **unpack:** errors now show the archive and destination names you gave, with control characters replaced. Names from inside an archive stay hidden. A denied archive no longer says "not a file", and a missing one says "no such file".
- **sweep:** `sweep status PATH` with no history now says so. It used to print nothing.
- **sweep, stash, unpack:** a permission failure while moving files or preparing a target says what to do next. A plan error no longer repeats "replan required".
- **stash:** an unknown flag or a second folder after the folder is now refused with exit 2. `stash FOLDER --dry-run` used to ignore the flag and move everything. Only a flag before the folder was checked.
- **stash:** `stash pop FOLDER` with more than one batch now exits 2 with `invalid_arguments` and lists the batch ids. It used to exit 3 with `journal_unreadable` and call a healthy journal malformed.
- **sweep:** the scan prints the exact next command, with the path and flags it was given. After `--export-plan` it prints the bound plan command. The old footer dropped flags such as `--depth`, so apply planned a different set of files.
- `--help` and `-h` now answer from any position on all three tools, before a `--`. `sweep apply --help` used to be refused, and `sweep status --help` printed nothing and exited 1.
- **unpack:** a killed extraction no longer leaves a copy of the archive in the temporary folder. The private copy has no name on disk before any bytes are written to it, so the system frees it when unpack and the extractor exit. A kill in the roughly 80 microseconds around creation can leave an empty file. Under App Sandbox, which refuses `/dev/fd` for files the process created, the copy keeps its private name and a kill can still leave it.
- **unpack:** a large tar no longer fails while the size check races the renames macOS tar makes. This was new in the 0.5.4 candidate: a 1,000-file tar passed 6 of 6 runs on 0.5.3, failed 5 of 6 before this fix, and passed 6 of 6 after it. Once the extractor has stopped, any failed read still fails the extraction.
- **unpack:** several extractions at the same time no longer refuse each other's safe zip files. This was new in the 0.5.4 candidate: 4 runs at a time refused a safe zip in 11 of 3,000 runs before this fix and 0 of 6,000 after it. Two runs can still get the same descriptor number, so the failure is rare, not impossible; it ends in a refusal with an error.
- **sweep:** when two applies overlap on one folder, the loser now records a refused attempt and reports a conflict, even if it fails after the plan check and before its journal exists. Before this fix, that loser left no record and could print a bare error. The check that every file is still present passed in the failed runs.
- Apply re-checks member eligibility before any change and again before each move or tag write, and refuses invalid tag entries in exported plans. A member found ineligible before anything moved is a refusal; one found ineligible after a move stops the apply as incomplete, with the journal holding what moved for `sweep undo`.

### Changes since 0.5.3

- Undo keeps empty created folders needed by another live journal, so stash can restore into them.
- Undo leaves tags without a completed write record because matching bytes cannot prove who added them.
- Tag undo checks file identity and leaves replacement files alone; older tag journals retain their original behavior.
- Journals with tag methods or newer base entry states use a header that older readers refuse.
- Apply removes empty created folders after a late tag causes the only item to stay at its origin.

- **sweep:** sweep holds Finder-tagged items by default and reports a count, not
  names or paths. The promise now reads: sweep decides from filesystem and
  Finder organization metadata only, never reads file contents to decide, and
  when it moves a file reads the first and last 4 KiB so undo can tell if that
  file changed. Finder comments are never read, and tag names are read only by
  `--scheme tags`, below. `--include-tagged`
  organises tagged items, prints a warning on stderr every time, and keeps the
  tag through a same-volume move. A tagged item whose move would cross volumes
  stays where it is, because the cross-volume copy does not carry the tag. Apply
  checks each item's tag again just before and just after the rename: a tag added
  late holds the item or moves it back, and if the original place has been taken
  the item stays, is counted as one that could not be returned, and the run ends
  incomplete (exit 3). A run that held every item reports `nothing_to_do`.
  Results gain `held_tagged`, `held_cross_volume`, `tagged_not_returned` and
  `hold_record_failed` in `details`, and the plan gains `tagged_skipped` and
  `include_tagged`. Journals gain optional `held`, `returned` and `stranded`
  progress frames; plan files gain an optional trailing section for the tag
  policy. Files written before this load unchanged.
- **sweep:** `sweep --scheme tags --map EXT=TAG` adds Finder tags and moves
  nothing (macOS only). Every tag name comes from the caller; sweep invents
  none. The plan lists each tag with its file count and marks a tag new when no
  file in the scanned folder carries it, because Finder's own tag list cannot be
  read. Tags are written only by `sweep apply --plan FILE --plan-digest SHA
  --yes`; planning, exporting, the live `sweep apply PATH` and `sweep review`
  write none. A tag is only added: the file keeps its tags in order and gains the
  new one, and a file that has it is left alone. Tagged files are tagged without
  `--include-tagged`, which is refused with this scheme. A file whose tag
  attribute cannot be read or parsed is listed and given nothing. The journal
  records each file's tag attribute before the first write, `sweep undo` writes
  those exact bytes back, a file whose tags changed since apply is left and
  named, and `sweep status` reports the operation. Sweep contract
  version 11 adds the domains `finder_tag_names` and
  `selected_tree_finder_tags` and a `tag_scheme` section. Plan files gain an
  optional trailing section and a group kind, and journals gain a `tag` entry
  method with two trailing fields; plans and journals that move files encode
  exactly as before.
- **stash:** stash sets `include_tagged` explicitly and clears tagged items. It
  reports the folder clear only when nothing was left behind; a tagged item
  that would have to cross volumes is left in place, the result says the folder
  is NOT clear, and the run ends incomplete (exit 3).
- **sweep:** `sweep undo` finishes after being killed at the last step. If it
  was killed after the last file went back, the empty folder that `sweep apply`
  created stayed, and a second `sweep undo` said there was nothing to undo.
  The second run now removes that folder and finishes. Only a folder the
  journal records apply created is removed, and only when it is still the same
  folder and empty; a folder with anything in it, or one that was replaced, is
  left alone. No file is deleted or overwritten.
- **sweep:** `sweep propose --plan FILE --plan-digest SHA --proposal FILE
  --export-plan OUT --json` places individual files. A plan export now lists the
  files it is willing to move in `details.items` (`id`, `name`, `size`, `group`).
  A proposal names an item and an existing folder under the plan root, or skips
  it; sweep checks each decision against the plan's own scan and writes a new
  bound plan, which `sweep apply --plan` applies and `sweep undo` reverses.
  Propose moves nothing and reads no file contents. Personal-looking, recent,
  in-flight, project, package, link and hidden files are not listed and cannot be
  named. One bad decision refuses the whole proposal. Limits: 1 MiB, 5,000
  decisions. The current sweep contract, version 11, includes the `propose`
  operation and the refusal codes `unknown_item` and `item_assigned_twice` (retry `never`,
  subject `item`). The plan file now also records the real paths of files no
  group claimed, and covers them in the plan digest.
- **Verification:** a stress scenario applies a `sweep propose` plan and kills the
  apply part-way, after the tool is stopped and the stop is confirmed. `sweep
  status` reports the interrupted apply, and `sweep undo` restores every original
  path and byte. Skipped files, unnamed files, a personal-looking file, a hidden
  file and a symlink stay in place throughout. Two controls show it can fail.
- **Verification:** `scripts/check-proposal-mutation.sh` plants four faults in a
  throwaway copy of the tree and requires a check to catch each: a changed
  refusal code, a removed duplicate-item check, a removed digest check, and a
  widened eligible list.
- **sweep:** `--scheme existing` routes eligible files using a versioned,
  reviewable profile to folders already present in the selected directory.
  A missing destination is named and refused instead of falling through to a
  built-in folder; ambiguous routes identify the profile, and safety refusals
  still win. Routed files are claimed once even when they also match a built-in
  detector. At depth 2, files already directly inside their destination are
  reported in place; deeper files remain eligible for promotion. Exported plans
  require replanning if the profile changes.
- **stash:** independent journalled batches return an opaque `stash_id`.
  Restore the chosen batch with `stash pop --id ID`; `operation_id` identifies
  only the invocation. Receipts expose no holding path; selection still carries
  `holding_root: "<redacted>"`, which is not a restore selector.
  `stash inspect --id ID` lists opaque item ids for partial restoration with
  `stash pop --id ID --item ITEM`.
- **stash folder restore:** `stash pop <folder>` (or `stash pop` in that folder)
  requires one live matching batch and authenticates all stash journals first.
  An unreadable older journal, completed or live and even from another folder,
  blocks selection of a newer batch: exit 3 with an unreadable-journal/cannot-restore
  diagnostic, leaving files and journal bytes unchanged. Use the selected batch's
  `--id` and its original key for exact restoration; isolate different keys with
  separate `ETUDE_STATE_DIR` directories.
- **sweep and stash:** exported plans bind the reviewed filesystem observations;
  stale plans refuse and require replanning.
- **unpack:** refuses to replace a destination folder created while extraction
  is running, even when that folder is empty. On volumes without a no-replace
  rename, it claims the destination before publication; the destination is
  visible and empty for an instant. If another process adds files there,
  publication refuses, preserves those files and reports the retained folder.
- **sweep and stash:** every apply checks that destinations stay inside the plan's
  root, including hand-built plans. Invalid folder names and destination folders
  that resolve outside the root are refused. Explicit stash selections can still
  gather files from several parent folders into holding folders inside the root.
- **unpack:** preserves the source archive's quarantine attribute on the final
  tree and verifies exact readback before publication; preservation failure refuses.
- **unpack:** refuses invalid arguments before it reads the archive or creates
  anything: unknown options, an option missing its value, a `--into` or
  `--max-size` value that looks like another option, a `--max-size` that is not
  a number or overflows, and extra positional arguments. It names the bad input
  and exits 2.
- **unpack:** on SIGTERM during extraction it stops and reaps the system
  extractor, removes its private staging directory, reports that it was stopped
  and exits 2. SIGKILL cannot be caught and is unchanged.
- **sweep and stash:** before each apply move, the destination folder must still
  be the real directory recorded at planning (or created by this apply). Before
  each undo move-back, the original source folder is checked against the
  identity recorded in the journal. A replaced folder stops the operation before
  any further file moves, with a named refusal. The check does not stop a swap
  in the instant between the check and the move, and does not defend against
  hostile code running as the same user.
- **sweep:** apply refuses a destination folder that appeared after the plan was
  made, instead of adopting it. Undo removes only empty destination folders that
  the apply created and that are still the same folder; a folder that existed
  before is kept.
- **sweep and stash:** with `--no-journal`, apply no longer reads the first and
  last 4 KiB of each file, since there is no undo to use it. A move across
  volumes still reads the whole file to copy and verify it.
- **sweep:** the scan output now says that applying reads the first and last
  4 KiB of each file it moves, so undo can tell whether a file changed. The
  README states when sweep reads file bytes.
- **Breaking for JSON consumers:** result envelopes use `schema_version: 4`.
  Agents that parse JSON must check the schema version and read operation
  fields under `details`; fields that were previously at the top level moved
  there. The envelope also reports scope, observations, effects, verification,
  recovery, disclosure and reason. Capability-contract schema versions are
  separate.
- **sweep, stash and unpack:** every `--json` result has a top-level `reason`
  with a stable `code` and a `retry` class: `not_needed`, `never`,
  `after_change` or `later`. For `after_change` and `later`, `reason.subject.kind`
  names what has to change or what to wait for. Each tool lists its codes under
  `refusal_codes` in `TOOL contract --json`. Exit codes are unchanged.
- **sweep and stash:** `sweep status --id ID --json`, `sweep status PATH --json`
  and `stash status --id ID --json` look up a recorded operation without moving
  anything. They report its state and its planned, moved and reversed counts,
  and no file names. An unknown id is exit 1 with the code `unknown_operation`.
  Results of commands that move files carry the id as `recovery.operation`.
- **sweep, stash and unpack:** a path that does not exist is reported as
  `missing_input` with retry class `after_change`. `sweep` reported a missing
  folder as `internal_error`, and all three tools reported a path that runs
  through a regular file as `internal_error`; both said `never`. Exit codes are
  unchanged.
- **sweep and stash:** a result's `status` and `reason` now agree. `sweep undo`
  and `stash pop` that leave a file behind because its original path is
  occupied still exit 0 with status `incomplete`, and now carry the code
  `operation_incomplete` instead of `done`. A `sweep` scan that could not read
  part of the folder carries `permission_denied` instead of `done` or
  `nothing_to_do`.
- **sweep:** `sweep status` reports an apply that was cut short and then undone
  as `reversed`. `partly_reversed` means some files are still moved. An apply
  that stops part-way names its journal in `recovery.operation`.
- **sweep and stash:** status has a fifth state, `refused`: a record with no
  entries, left by an apply that was turned away before it moved anything. It
  was reported as `completed`. Its counts are all zero and it offers no undo
  command.
- **stash:** `stash pop --id` and `stash inspect --id` with an unknown id return
  `unknown_operation`, as `stash status --id` does. Exit codes are unchanged.
- **Contracts:** each tool's `refusal_codes` lists only codes that tool can
  return. `folder_busy`, `recent_files`, `empty_selection` and `user_cancelled`
  are removed, and `download_in_flight` is removed from unpack. The capability
  declarations are now version 11 (sweep), 10 (stash) and 6 (unpack).
- **Known gaps in `reason`:** the contract check proves 16 of sweep's 18 listed
  codes, 11 of stash's 15 and 9 of unpack's 10, and names the rest as unproven:
  `internal_error` for all three, `journal_key_unavailable` for sweep and stash,
  and `destination_exists` and `destination_outside_root` for stash. A folder
  that holds only recent files reports `nothing_to_do`; a "try again later"
  code for it is a possible later
  addition. Two `sweep apply` runs on one folder at once are not held apart,
  and the one that loses ends in one of two ways. Either it is refused before
  it moves anything (exit 2, `plan_stale`; plan again), or it stops part-way
  (exit 3, `operation_incomplete`, with a `recovery.operation` id that status
  reports as `interrupted`; run `sweep undo`, then plan again). After the
  part-way ending, repeat `sweep undo` on the folder until nothing is left to
  undo. It restores the apply that recorded the remaining moves first, then
  the stopped apply, even if the stopped apply's journal is newest. Every
  original path and byte comes back. A journal that really lost several move
  records is still refused without touching files.
  `unpack` reports a `.part` or `.download` name as `unsupported_format`.

## [0.5.3] - 2026-09-26

sweep, stash and unpack.

The sweep, stash and unpack 0.5.3 tags and releases are published. The earlier
unpack 0.5.2 entry below was source-only and was never published.

- **sweep:** project documents found in child directories now hold related
  sibling assets, while unrelated documents and screenshots remain eligible.
  Content-inspection buffers erase the full allocation on drop, with a test
  that observes the destructor path.
- **sweep and stash:** `ETUDE_JOURNAL_KEY` allows encrypted undo when the login
  keychain is unavailable. Invalid supplied keys refuse without fallback.
  Retain the same key for subsequent undo; an external key cannot be destroyed
  by `sweep forget`. Journal loading also checks the opened file's identity.
- **unpack:** refuses dangerous ZIP/TAR member types, checks ZIP central and
  local records, and runs listing and extraction against a private archive
  copy. System extractor paths are fixed. Cleanup reports what remains when
  removal fails instead of claiming nothing was left behind.
- **Verification:** additional undo assertions and standalone stress scenario
  exit codes expose failures to callers. Release installation checks distinguish
  local candidates from public tags and exercise the installed binaries.
- **sweep, stash:** long apply/undo/stash/pop operations report bounded item
  progress on stderr. JSON stays on stdout and small operations stay quiet; the
  existing per-entry journal writes remain unchanged when journaling is enabled.
- **sweep:** the macOS cross-device copy refuses an occupied destination,
  including NFC/NFD-equivalent names. A forced-EXDEV test on the real APFS
  normalization behavior proves both original files survive the refusal.

The extraction budget remains a polled soft limit. Extraction now uses private
staging and atomic publication, so an interrupted extraction never publishes a
partial requested destination. Complete structured mutation receipts are not
part of the 0.5.3 candidates. Existing journal encryption and the 0/1/2/3
exit-code convention remain unchanged.

## [unpack 0.5.2 source changes] - 2026-08-23 (not published)

The earlier entry was dated 2026-08-23, but no `unpack-v0.5.2` tag or release
was published. Its changes were included in the 0.5.3 release above.

### Fixed

- **A symlink in an archive is refused, not extracted.** Measured before the
  fix: `unpack lone.zip` printed "Checked 2 paths before writing anything"
  and then landed `shortcut -> /etc/passwd` in the target. The listing it
  judged (`unzip -Z1`, `tar -tf`) reports names only, so a member's TYPE was
  invisible to every check. Both formats reproduced it.

### Added

- Preflight now judges member TYPE from the verbose listing's mode string,
  refusing symlinks, hard links, device nodes, FIFOs, sockets, and
  setuid/setgid bits — alongside the traversal and depth checks it already
  made. Each refusal names the member, states the rule, and offers a
  recourse, the same shape the `.dmg` refusal has always had.
- An archive whose verbose listing fails while its plain listing works is
  refused: a check that cannot run is not a check that passed.

SECURITY.md gains an unpack section stating what this buys and what it does
not — a lying listing, nested archives, CPU-time bombs and parser bugs in
the system tools are all outside it, and the claim is "refuses every
dangerous member its listing declares", never "safe unarchive".

## [stash 0.5.2] - 2026-08-23

stash's first independent move: the deadline becomes a contract an agent can
build on, while stash itself stays a tool that only acts when asked -- no
daemon, no timer, nothing at login. The agent is the automation layer; stash
is the custody layer. Same division sweep 0.5.2 shipped.

### Added

- `stash pop --if-due`: pops only past the deadline. Early is exit 2
  (refused by the clock), nothing stashed is exit 1 -- distinct, so a
  launchd job can tell "fire again later" from "delete yourself". A typo of
  the flag is refused rather than falling through to a plain early pop,
  which required giving stash the per-command flag table sweep already had.
- `stash status --all`: every stash this machine's journals know, with a
  stable id, ISO-8601 deadline and overdue flag. Roots are REDACTED unless
  `--paths` is passed -- the flag advertises the read's reach in the
  invocation string itself, because `status --all` reads as narrow in an
  agent's audit log while enumerating paths machine-wide.
- `id` and `due_iso` in per-root `status --json`.
- A plain early `pop` says, in one line, that it is early. It never asks
  and never refuses: deciding later includes deciding now.
- `--paths` answers only to a person at a terminal: a non-interactive caller
  gets exit 2 and the reasoning, never the paths. `journal-dump` -- the
  decrypted pathname history of every move -- now refuses non-interactive
  callers the same way. The principle and the full list of gated surfaces
  are in SECURITY.md: where a command's reach exceeds the folder it was
  pointed at, refuse and say why.

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
