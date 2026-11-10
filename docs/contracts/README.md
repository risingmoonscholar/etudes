# Capability contracts

`sweep contract --json`, `stash contract --json` and `unpack contract --json`
return the same versioned capability declaration shape. A plain `contract`
also prints JSON. Extra flags and positional arguments are refused with exit 2.
The query returns before journal migration, expiry, key acquisition and scan;
it reads no application files and changes no application state. Runtime loading
of the executable and system libraries is outside these application scopes.

Each declaration carries integer `schema_version`, the binary's `tool_version`,
a fresh `operation_id`, and `status: "done"`. It describes potential capabilities,
including conditional paths. It is not an operation receipt. `evidence` explicitly
marks access as unverified; a self-reported count cannot prove absence of access.

`observation_scope` declares default access to user payloads, content exceptions,
symlink handling and environment variable names. It never outputs their values.
Subprocesses inherit the environment. `mutation_scope.operations` gives each
operation's read and write domains; `mutation_scope.startup` must be added to
every operation except `contract`. Sweep's startup deletes no journal: a sweep
`apply`, `review`, exported-plan apply or `undo` may delete expired journals of
any tool except stash that open with the key it already holds, `stash pop` may delete expired records of
stashes with nothing left to pop, and stash status can acquire a key and migrate
state. `unpack --list` copies archive bytes into a private temporary directory
and removes that copy; absence of a published destination does not mean no writes.

Scope domains resolve as follows:

| Domain | Meaning |
| --- | --- |
| `selected_tree_metadata` | Directory entries, names, types, sizes, modes, dates, devices, inodes and `st_flags` (the immutable flag) under the requested root, within the tool's scan depth and skip rules. Sweep also reads the 32-byte `com.apple.FinderInfo` attribute of visible regular files to skip Finder aliases by the `kIsAlias` flag; this is counted in the `metadata` observation category. For items sweep may move this includes whether a Finder tag attribute is present, asked through a zero-length buffer so tag names are never read; Finder comments are never queried. The tag presence read is counted in the `finder_tag_presence` observation category |
| `finder_tag_names` | `sweep --scheme tags` only: the tag attribute of files sweep may tag, read in full to learn which tags exist in the folder, which files already carry the tag asked for, and what to put back on undo. Nothing else is read from the attribute list, Finder comments are never queried, and file contents are not read. The read is counted in the `finder_tag_names` observation category |
| `selected_tree_finder_tags` | `sweep --scheme tags` only: the tag attribute of planned files, replaced by the old tag list plus one tag on apply of an exported plan, and by the recorded old bytes on undo. No file is moved, renamed, created or deleted |
| `ancestor_metadata` | Ancestor names and metadata used for system-location, cloud-sync and filesystem checks |
| `project_marker_names` | Names of project markers; no parsing of marker contents |
| `explicit_proposal_file_bytes` | The proposal file named by `sweep propose --proposal`, at most 1 MiB; its decisions are checked against the plan and never read file contents |
| `sweep_existing_profile_bytes` | The versioned plain-data routing profile in the configured etudes state directory; read to plan and verify before applying an existing-folder plan |
| `consented_text_prefixes` | Prefix bytes of allowlisted files, after separate terminal consent; only increases refusal |
| `journal_fingerprint_bytes` | First and last 4 KiB of regular files used by custody and restoration fingerprints; directories and links stay opaque |
| `cross_device_source_bytes` | Data read by the copy fallback after EXDEV; applies to moves and restores, independently of inspection consent |
| `selected_tree_entries` | Relocations and new group/holding directories within the selected tree, including filesystem case-probe files |
| `journal_store`, `journal_store_metadata` | Encrypted journal bytes, or just journal names and metadata, in the configured state directory |
| `journal_recorded_paths_metadata`, `journal_recorded_entries` | Paths recorded in decrypted journals, which can span roots outside the current working directory |
| `legacy_state_metadata`, `legacy_state_migration` | The old HOME/.local/state/etudes directory and its one-time move to the current default state directory, when no override is supplied |
| `expired_journals_that_open_except_stash` | Journals older than the core TTL in every namespace except stash, pruned by a sweep apply, review, exported-plan apply or undo, only if they open with the key that run already holds; one that does not open is kept |
| `expired_journal_store` | The encrypted bytes of journals outside stash older than the core TTL, opened by an apply-type run with its own key to check the key fits before pruning |
| `popped_stash_journals_past_ttl` | Stash journals older than the core TTL that open with the current key and hold nothing: no item still moved, no damaged progress tail, no unrecorded move on disk. Removed by `stash pop`. A stash journal that does not open with the current key is never removed by the tools, not by this prune, sweep's prune or `sweep forget`, because it may still restore files. `stash status --all` names it as unreadable; once its files are back, remove the file `stash-ID.journal` from the state directory by hand (`sweep forget` prints the `rm` step for each stash record, with or without `--yes`; without `--yes` it then stops before destroying the shared key, and with `--yes` it first says that those records will no longer open and their files stay in the hidden `.stash-` folders) |
| `journal_key`, `keychain_if_no_supplied_key` | Supplied key or shared login keychain key; acquisition may create a key when none exists |
| `sweep_journals`, `shared_keychain_key_if_authorized` | Forget removes sweep's journals; destroying the shared key has separate guards |
| `empty_group_directories`, `empty_holding_directories` | Empty directories removed after restoration |
| `terminal_input` | Review, inspection or key destruction consent and choices |
| `archive_bytes`, `archive_metadata` | The requested archive's bytes and filesystem metadata |
| `private_archive_copy`, `os_random_bytes` | A temporary private copy of the archive and /dev/urandom bytes for its name |
| `destination_parent_metadata`, `volume_free_space` | Destination checks and the volume measurement used for the default soft extraction budget |
| `staging_tree_metadata`, `private_staging_tree` | Audits of the actual extracted tree and the periodic walk that measures bytes and planned members during extraction, and private extraction, junk removal and failure cleanup beside the destination |
| `new_destination_tree` | The audited tree published by rename to a previously absent destination |

`overwrite.user_destinations` covers caller-owned destinations. Journal updates
and private staging have separate rules. `deletion.user_payloads: "never"` means
no intentional destruction of the caller's only payload: relocation can remove
source entries, and cross-device relocation unlinks only after checking copied
size. That check is not a cryptographic integrity verification. Unpack removes
private copies and junk in staging while retaining the original archive.

`formats` separates opaque filesystem handling, optional byte inspection and
archive extraction. Metadata handling and moving an item do not imply parsing
its format. Sweep's content inspection list comes from `TEXT_EXTS`; support
means bounded raw-byte scanning, not a PDF, Office, RTF or structured-data parser.
Unpack's extraction list comes from the same table used for suffix dispatch,
including `.jar`, `.tbz` and `.txz`. `.dmg` is explicitly unsupported by design.
Each format domain includes a named unsupported list and an `unlisted:
"unsupported"` policy; `unsupported: ["all"]` means that entire domain is
unavailable. Archive support is still subject to member safety gates, extractor
availability and the monitored soft size budget. The supported platform is macOS.

Stash's `home_folder` says it refuses the caller's home folder unless the named
flag is passed.

`profile_missing` means no saved profile exists; `profile_invalid` means one
exists and cannot be used. `insufficient_space` and `destination_exists` now also
cover a full disk and an occupied `--export-plan` path in `sweep` and `stash`.
`sweep status` adds `counts.not_as_left` and `damaged`, and its `command` is now
`sweep undo --id ID` for that row. `undo_lookup` declares `sweep undo --id OPERATION`.

A missing folder is exit 3 with `missing_input` on `sweep status PATH`, `stash
status PATH` and `stash pop PATH`. A scan whose plan is blocked by a file holding a
group folder's name adds `details.blocked_folders` and stays exit 0.

A `sweep` or `stash` apply whose failed move left a copy or second name it
could not remove at the destination adds `details.copy_left`: that path, or
`"<redacted>"` for `stash select`, whose result shows no holding path. Undo does
not know about that copy. The field is absent otherwise, and its presence does
not change the status, reason or exit code.

A `sweep undo` result adds `details.remaining_reversible`: how many other
applies of the same folder can still be undone after this one, not counting a
record that does not open. It does not change the status, reason or exit code.

Stash also returns `download_in_flight` for a folder still downloading and
`unknown_item` for an item id its stash does not hold. `stash pop` exits 3 with
`operation_incomplete` when any item is left behind, and its details add
`skipped_occupied`. `stash status --all` lists the stashes that open and adds
`unreadable`, the ids of those that do not.

The declarations do not enforce an OS sandbox. Network policy is `none`, runtime
enforcement is `none`, and subprocess network behaviour is unverified. Consumers
must treat declarations separately from observed or independently verified access.

The original immutable declarations remain under `history/v1/`; their byte
digests are checked by the witness. Canonical pins in `v2/` retain the preceding version-2 declarations; `v3/`
pins the preceding version-3 declarations. `v4/` pins version-4 declarations;
`v5/` pins version-5 declarations of all three tools. Later directories pin each
later declaration: `v17/sweep.json`, `v14/stash.json` and `v9/unpack.json` are the
current pins.
They fix all fields, types and declared semantics. The published `v1/*.json` fixture
locations mirror each tool's current canonical envelope for existing documentation
links; trust `schema_version`, not a fixture directory name. Only the per-run
operation ID and manifest version are normalized. Changing the contract without
bumping `SCHEMA_VERSION` fails `cargo test`. To evolve it, retain the old pins,
increase the affected declaration version and add its pin under the new version directory.
Consumers should reject unknown versions rather than guess at their meaning.

`scripts/check-tool-contracts.py` runs against `CARGO_BIN_EXE` in each CLI's
integration tests and release binaries in `70-content-blindness-traps`. It checks
query immutability even with expired journals and a malformed key, exact supported
archive suffixes through real listing and extraction, source preservation,
selected-tree effects, hidden items, symlinks, collision handling, encrypted
state, recovery and `--no-journal`. It scans each binary for direct networking
symbols. Negative controls remove a field without a version bump and inject
an out-of-scope mutation while advertising the unchanged valid contract; both
must fail the witness for the expected reason.

The same script makes each tool return the refusal codes its contract lists.
For every code under `refusal_codes` it runs a real invocation on a throwaway
fixture and compares the result's `reason` with the contract: the code, the
retry class and, for `after_change` and `later`, the subject kind. It prints a
summary per tool. `proven` means the tool returned what the contract promises.
`unproven` names a listed code no probe reached, with the reason; it is a gap,
not a pass. `mismatched` means the tool answered with something else, and fails
the check. A refused or error result whose `reason` is `null` also fails: a
tool that leaves a refusal unclassified still exits with its usual status and
prints `"reason": null`, so this check is what catches it.

Every `--json` result the script sees must also tell one story in `status` and
`reason`. A `done` status carries the code `done`. A `nothing_to_do` status
carries `nothing_to_do`, or `unknown_operation` or `missing_input` when a
lookup found nothing. Any other status carries a code other than those two
success codes, so a result that is `incomplete`, `refused` or `error` can never
read as finished to a caller that looks only at `reason`.
`scripts/check-refusal-code-mutation.sh` plants three faults in sweep's source,
a wrong code, a missing reason and an incomplete undo that says `done`, and
requires the check to reject each.
`scripts/check-proposal-mutation.sh` does the same for `sweep propose` on a
throwaway copy of the tree: a changed refusal code, a removed duplicate-item
check, a removed digest check and a widened eligible list must each be caught.

**Declaration versions 8 (sweep), 9 (stash) and 6 (unpack)** list only codes
the tool has a code path for. `folder_busy`, `recent_files`, `empty_selection`
and `user_cancelled` are removed from every list: no tool returned them, or
returned them only from a command that takes no `--json`. `download_in_flight`
is removed from unpack, which reports a `.part` or `.download` name as
`unsupported_format`. `unknown_operation` now also covers `stash pop --id` and
`stash inspect --id`. `v8/sweep.json`, `v9/stash.json` and `v6/unpack.json` pin
these declarations; earlier pins are unchanged.

**Declaration versions 9 (sweep) and 10 (stash)** add a fifth status state.
`status_lookup.states` is now `refused`, `completed`, `interrupted`, `reversed`
and `partly_reversed`. `refused` is a journal record with no entries. Such a
record is written only when an apply is turned away before it moves anything,
because the folder changed under its plan; an apply with nothing to move writes
no journal. Its counts are all zero and it has no recovery command. It was
previously reported as `completed`. `v9/sweep.json` and `v10/stash.json` pin
these declarations; unpack stays at version 6. The `v1/` fixtures mirror the
current declarations listed above.

**Declaration version 10 (sweep)** adds the `propose` operation and two refusal
codes, `unknown_item` and `item_assigned_twice`, both retry `never` with subject
kind `item`. `propose` reads the exported plan and the proposal file, checks
destination folders by metadata, and writes one new plan file; it moves nothing.
`destination_outside_root`, `destination_exists`, `plan_stale` and
`profile_missing` also apply to it. `v10/sweep.json` pins this declaration and
the `v1/` fixture now mirrors version 11; stash stays at version 10 and unpack
at version 6.

**Declaration version 11 (sweep)** adds `--scheme tags`. The domains
`finder_tag_names` (read) and `selected_tree_finder_tags` (write) are declared on
the operations that use them: scan, scan with content inspection, export,
`apply_exported_plan` and undo. The live `apply` and `review` operations refuse
tags and declare neither. A `tag_scheme` section states the rules: every tag
name comes from the caller, tags are written only by applying an exported plan,
a tag is only added, "new" means no file in the scanned folder carries the tag
(Finder's own tag list is not readable without extra permissions), a file with
unreadable tag state is listed and given nothing, the journal holds the previous
and written bytes before the first write, and undo restores them only while the
file still holds what apply wrote. In `status`, `moved` counts files tagged.
No refusal code is added: a refused request is `invalid_arguments`,
`unsafe_input` or `plan_stale` as before. `v11/sweep.json` pins this declaration
and `v1/` mirrors it; stash stays at version 10 and unpack at version 6. The
witness drives a tag plan end to end on a synthetic folder.

**Declaration version 13 (sweep)** declares the FinderInfo alias flag read in
`selected_tree_metadata` and the separate `skipped.aliases_skipped` plan count.
Finder aliases are skipped without reading their contents or resolving their
targets. An unreadable attribute holds the file and makes the scan incomplete.
`v13/sweep.json` pins this declaration and `v1/sweep.json` mirrors it; earlier
pins remain unchanged. The binary probe checks flagged and unflagged files,
separate symlink accounting, the human summary and unchanged fixture bytes.
Plan artifacts append optional section 4 for a nonzero alias count; older plans
load with zero and plans without aliases retain their earlier encoding. A folder
held back only because its files are too recent, where those files would form a
group once the window passes, reports `nothing_to_do` with retry `later` and an `input` subject, on a scan and on apply; `nothing_to_do` keeps
retry `not_needed` everywhere else.
The same version also declares `locked_items`: the immutable flag is read from
the `st_flags` of the per-entry `lstat` the scan already takes (no further
read), locked files and folders are held and counted under
`skipped.locked_skipped`, apply takes one `lstat` per item before its move and
holds a newly locked item (`held_locked`), and a locked root or destination
folder refuses the plan before any move. Plan artifacts append optional
section 5 for a nonzero locked count, with how many of those carry the system
flag (`skipped.locked_system`).
A scan of a locked root does not offer apply, and `--json` adds
`details.root_locked`, only when it is true.

**Declaration versions 13 (sweep) and 12 (stash)** reword `plan_stale`: the plan
cannot run as made, because what it saw has changed or a folder it needs is
locked. Clear any lock, then plan again. A locked root or destination folder
keeps this code and exit 2. `v12/stash.json` pins the stash declaration and
`v1/stash.json` mirrors it; earlier pins remain unchanged.

**Declaration versions 14 (sweep), 13 (stash) and 8 (unpack)** declare one
`structured_progress` section for every tool: the flag, the schema version, the
fields, the record bound, what each operation's counts count, how an unknown
count reads, and what a terminal record means. It lists sweep `undo`, stash
`stash` and `pop`, and unpack `extract` and `list` for the first time, and moves
sweep's single count descriptions into per-operation rows. The progress schema
itself stays at version 1 and the result envelope at version 4. No read or write
domain is added: unpack's count comes from the staging walk already declared as
`staging_tree_metadata`, so progress adds nothing to an unpack `extract` scope. `v14/sweep.json`,
`v13/stash.json` and `v8/unpack.json` pin these declarations and `v1/` mirrors
them. See [structured progress](../progress.md).

These are finite falsification probes. They do not observe every read attempt,
prove absence of ordinary-file reads, check subprocess networking or cover every
conditional path (including keychain, EXDEV, review and crash recovery). Those
claims remain unproven by this witness. Broader existing unit/stress witnesses
exercise some of those paths separately.

## Result envelopes and read receipts

**Breaking in 0.5.4:** the result envelope (introduced as schema version 2;
version 4 is current) wraps every `--json` response, including parser refusals,
operational errors and recovered panics. Plain `contract` also returns this
envelope. Existing contract and operation fields move under `details`; consumers
must check `schema_version` before accessing that field. The original schema-version-1
declarations are retained verbatim under `history/v1/`. The published
`v1/{sweep,stash,unpack}.json` fixtures mirror the current canonical
contract envelopes (`v14/sweep.json`, `v13/stash.json` and `v8/unpack.json`);
`v2/envelope.json` and `v3/envelope.json` retain the preceding result structures;
`v4/envelope.json` pins the current one. Editing fields or types without adding a new version is
rejected by the independent binary witness.

The envelope contains `schema_version`, `tool_version`, `operation_id`, `status`,
`scope`, `observations`, `effects`, `verification`, `recovery`, `disclosure`,
`details` and `reason`. Status is one of `done`, `nothing_to_do`, `refused`, `incomplete` or
`error`. Exit codes retain their existing meaning. Incomplete move or restore
attempts require checking the filesystem and the journal before retrying.

Each `TOOL contract --json` capability declaration lists every refusal under
`refusal_codes`, with its stable code, retry class, operation names and reason
for that class and a `subject_kind` (null when the code has no subject).
`never` means the same request cannot work; `after_change`
requires a changed request or folder; `later` means the same request may work
after waiting. The last two always include a `subject`, which names the relevant
input, plan, profile, journal or contention without disclosing extra file names.
A `never` code has none, except `unknown_item` and `item_assigned_twice`, whose
subject kind is `item`. The result schema marks `subject` optional, so no
result version changes.

**Result envelope version 3:** version 3 is version 2 plus the top-level
`reason` field. A reason has a stable code and retry class; refusals that need
the caller to change something or wait include a typed subject. A successful
result uses `done` or `nothing_to_do` with `retry: "not_needed"`. A sweep scan
held only by the grace window uses `nothing_to_do` with `retry: "later"` and
subject kind `input`; empty folders and other holds keep `not_needed`. Adding this
field is breaking for parsers that require exactly schema version 2.

**Result envelope version 4:** version 4 is version 3 plus
`recovery.operation`, the durable journal identifier for operations that can
be queried later. The top-level `operation_id` remains the identifier for one
invocation. The recovery operation field is empty when the operation has no
durable journal identifier.

`scope` identifies the tool and operation and includes its conditional declared
read/write domains and startup domains. Those are capabilities, not observations.
`observations.categories` records instrumented read operations by category:
metadata, Finder tag presence, Finder tag names (tag scheme only), directory enumeration, access opens, encrypted
journal bytes, key acquisition, fingerprint bytes, cross-device copying and
archive copying.
`attempted` counts calls, `observed` successful calls, `failed` failed calls,
and `verified` successful calls with a separate check. An open is not a payload
read. Directory-enumeration calls and yielded directory entries have separate
categories. Fingerprint reads and cross-device reads have separate categories. Verification
is narrow: journal authentication and identity checks, or copied size equality,
which is not cryptographic equality. Key acquisition is recorded without its
source value. Receipts contain no paths, contents, keys or environment values.
`details` retains the operation's existing path disclosures.

`unproven` separately names subprocess reads, environment access, uninstrumented
operations and absence of access. Receipt instrumentation runs on the command
thread, not in the OS or system extractors. No zero or missing counter is evidence
of no access. `verification` names each claim and gives `pass`, `fail` or
`unproven`; a successful process does not establish complete read coverage or
subprocess network absence. `effects` identifies the outcome and reports available operation counters
for moves, restoration, publication, audit and expiry pruning. Unaccounted effects
remain explicitly unproven; missing or zero accounting proves no absence of effects.
Recovery names its operation-specific mode and conditions. Moves use
conditional journal restoration; `--no-journal` requires manual restoration.
Queries and scans need no operation undo; their declared startup effects still
apply. Restore failures require resolving refusals before retrying remaining
journal entries. Recovery verification remains unproven by the envelope. Unpack retains its source archive and has no undo command.

## Unpack quarantine declaration (introduced in version 3)

Unpack's capability declaration under `details.schema_version` is version 8;
its surrounding result envelope is version 4. `v8/unpack.json` pins the
current declaration and `v1/unpack.json` mirrors that current fixture. Earlier
unpack pins stay unchanged. Sweep and stash use their own declaration
versions, while the shared result envelope uses version 4.
The declaration names captured archive quarantine metadata, staging attribute
reads and writes, exact readback before publication, and refusal if preservation
fails. On exFAT, macOS-generated AppleDouble companions are retained when they
provide the attribute representation. Raw values are never disclosed.


Sweep capability declaration version 6 adds the read-only, versioned
existing-folder profile to its declared scope. The plan digest binds the
profile bytes; profile changes require a fresh plan. Safety refusals still
outrank profile routes, and the scheme never creates a destination. A missing
profile destination is named and refused so built-in grouping cannot silently
route its files elsewhere. The plan reports files already directly inside a
profile destination as in place; deeper files can still be promoted when the
selected scan depth includes them.

Sweep capability declaration version 7 adds `download_in_flight` to the refusal
catalog for a scan whose root is still being downloaded. The result is retryable
later and names the in-flight download as its subject. Earlier versioned
declarations remain unchanged.

Stash's capability version 3 and sweep's preceding version 3 declare explicit
plan exports and replay. The result envelope was still version 2 at that point; the separate
bounded plan artifact is version 1. Existing versioned pins remain immutable. A plan holds metadata
observations and selected paths, with private held-name commitments. Replay uses
the original digest and current tool/profile/observation contract, refusing stale
observations before case probes, journals or relocations. Root/filesystem identity
values and plan digests are meaningful binding evidence, not proof of payload
integrity or complete access coverage. See [plan binding](../plan-binding.md).
## Stash explicit selection introduced in declaration version 3

Explicit selection was introduced in stash declaration version 3; the current
declaration is version 14 and its surrounding result envelope is version 4.
`v3/stash.json` pins declaration version 3; `v1/stash.json`
tracks the current declaration. The preceding `v2/stash.json` stays unchanged. The `stash` operation scope declares
conditional legacy folder scanning and explicit selection access separately.
Selection reads only its path list, selected object metadata and necessary ancestors,
plus the existing journal fingerprints and cross-volume file copying. Its writes
are relocations of explicitly selected objects, private numbered holding slots,
and the existing encrypted journal. A selected directory is opaque; its children
are not enumerated. Explicit links, special files and known sync source or holding
locations are refused before holding; no sync override is offered for selection.

`explicit_selection_object_metadata` includes canonical parent resolution and
no-follow selected-object metadata used for location, type, identity, overlap and
volume checks. `explicit_selection_path_list_bytes` is the supplied NUL-terminated
UTF-8 list from a file or stdin, not arbitrary payload inspection.
`os_random_bytes_for_selection_holding` names its private operation root.
`explicit_selected_objects` and `private_selection_holding_tree` include the
chosen relocations, numbered slots and case-probe files. Selection JSON retains
`details.holding_root: "<redacted>"` and exposes no holding
path. Use `details.stash_id` with `stash pop --id ID` as the recovery selector;
the envelope `operation_id` identifies only this invocation.
`stash inspect --id ID` returns path-free item ids, and `stash pop --id ID --item
ITEM` records a single item's restore while reporting what remains held. Original
source paths remain in the encrypted journal and are not listed in the receipt.
`--no-journal` is refused for selection, while legacy whole-folder behavior is
retained.


## Stash batch declaration version 5

Stash's batch capability was introduced in declaration version 5. The current
declaration is version 14 in `v14/stash.json`; `v1/stash.json` mirrors it, while
earlier versions remain pinned.
Every journalled stash has a persistent `stash_id`, separate
from the per-invocation result `operation_id`. Exact-ID restore refuses missing,
malformed, unreadable or mismatched IDs without falling back to another operation.
`--latest` uses creation time for current IDs and refuses ties; legacy IDs use
modification time and also refuse ties. Due dates are informational: `--if-due`
only gates an explicitly requested pop. Explicit `--no-journal` folder stashes have
no restorable ID and require manual recovery. `inspect --id ID` enumerates stable,
path-free item ids and actual held state; `pop --id ID --item ITEM` restores one
item with per-entry journal progress, so a later whole-operation pop resumes the
remaining entries.

Folder pop (`stash pop <folder>` or `stash pop` in the current directory) requires
one live matching batch. Discovery authenticates all stash journals before
filtering by folder or discarding completed batches. An older journal unreadable
under the current key therefore blocks even a newer matching batch, whether the
older batch is completed or live, including one from another folder. Exit 3 names
the unreadable journal and says it cannot restore; files and journal bytes remain
unchanged. Exact `stash pop --id ID` loads only that selected journal with its
original key. Keep different key histories in separate `ETUDE_STATE_DIR` stores.

Stash declaration version 8 adds `not_due_yet` to the refusal catalog for an
explicit `pop --if-due` whose journal deadline is still ahead. It is retryable
later with the same request and identifies the journal as its subject. Earlier
versioned declarations remain unchanged.

## Unpack staging recovery, declaration version 8

`staging_recovery_metadata_and_lock_marker` covers listing the destination's
parent, ownership and identity checks on folders with the exact leftover name, and
the lock marker. `verified_abandoned_staging_for_same_destination` is a write
scope and an owned artifact that may be deleted. Recovery does not read archived
payloads to decide whether a folder is abandoned. `v8/unpack.json` pins this
declaration and `v1/unpack.json` mirrors it; `v7/unpack.json` remains retained.
Stderr names every folder removed and every folder with the exact name left in
place. The result's `effects.counters` add `leftover_folders_removed` and
`leftover_folders_kept`, present only when not zero. See
[staging recovery evidence](../release-evidence/unpack-staging-recovery.md).

**Declaration version 15 (sweep)** adds routing discovery: `--map`,
`--save-routes`, `--scheme existing`, the version-1 TOML profile path and shape,
and `propose` are listed in `routing`. Live apply declares the conditional
profile write. A plan may include `details.routing_hint` with `count`, at most
three `extensions`, `folder` and an executable `command`; text hides this under
`--quiet`, and `--json --quiet` keeps it. Map advice names a new folder for the
person to replace, never a folder found in the tree, and observes the three-file
grouping floor; smaller groups, built-in extensions and extensionless files get
the working `propose --help` step instead. Saving requires validated existing
direct visible destinations and an undo journal; only the routes being added are
checked against the tree. Conflicts refuse before moves. A save with nothing new
writes nothing; new routes are appended, so existing text and comments stay. A
complete apply replaces the mode-0600 profile through a per-writer locked
same-folder temporary file, fsync and rename; temporary files left by dead
writers are removed. Failed or partial applies save nothing. `details.routes_saved`
reports `state`, `reason`, `added`, `requested`, `profile`, `written` and
`folder_synced`. A save that fails after every file moved is `incomplete` with
`profile_write_failed`, whose subject is the profile; refusals before moves use
`invalid_arguments`, `missing_input`, `profile_invalid`, `permission_denied`,
`insufficient_space` or `profile_write_failed`. Undo keeps saved routes. Existing profile SHA-256 replay binding
is retained. `v15/sweep.json` pins this declaration and `v1/sweep.json` mirrors
it; the result envelope stays at version 4.

**Declaration version 16 (sweep) and 14 (stash)** stop sweep's prune from
deleting stash records. A stash record is the only way to bring back the
files in its holding folder, so it is kept for as long as anything is held,
however old it is. Sweep's prune moves out of startup: startup now reads no key
and deletes no journal, so a scan, `status` or `verify` never reaches the
keychain to prune. Records past 30 days are dropped by the next run that opens
records: `apply`, `review`, an exported-plan apply or `undo`, each with the key
it already holds. An apply with `--no-journal` holds no key and prunes nothing. Those scopes add the write domain
`expired_journals_that_open_except_stash`, and the apply-type scopes add the read
domain `expired_journal_store` (`undo` already reads `journal_store`). A record
that does not open with that key is kept, so the advice `sweep undo` gives for it
("Keep that record") stays true. An apply prunes before it moves anything, so its
own new record cannot be removed. An undo prunes after the undo, so the run that
brings the right key still finds the record. Stash adds the owned
artifact `popped_stash_journals_past_ttl`: `stash pop` opens stash records past
the TTL with the current key and removes those that hold nothing, so a popped
stash's record expires 30 days after the pop that finished it. A record that
does not open is kept. Stash's reversibility conditions drop
`journal_not_expired`, since a record that holds items does not expire. Sweep's
reversibility conditions keep it. No read or write domain of `pop` changes: it
already reads and writes `journal_store` and reads
`journal_recorded_paths_metadata`. `v16/sweep.json` and `v14/stash.json` pin
these declarations and `v1/` mirrors them; the result envelope stays at version 4.

**Declaration version 17 (sweep)** lets `--scheme existing` create a saved
route's folder, and limits the three-file floor to new folders. When a route's
direct visible folder is missing, apply creates it only for a group that moves
at least one file, through the same checked path as a `--map` folder: the plan
lists it first (signal `.EXT (saved route, new folder sweep creates)`), the
journal records it, and undo removes it when it is empty. A route with no files
in the selected folder is ignored and creates nothing. A file or link holding
the route's folder name still refuses with `profile_invalid` before anything
moves, and routed files never fall to a built-in group. A saved route has no
floor: one or two matching files move. `routing.new_folder_floor` states the
rest: a built-in group or a `--map` group needs three eligible files only when
sweep would create its folder; when the folder already exists as a direct child
of the root, any number moves into it, and files already directly inside it
are reported in place. Apply, review and exported-plan apply declare the write
`selected_tree_route_folders_if_missing`. An exported plan marks a saved
route's group with a new signal tag (8), stated in `routing.saved_route_plans`.
A plan written by an earlier version reads as it did. A plan that holds a
saved-route group needs this version: an earlier one cannot read it and refuses
it as an invalid or unsupported plan binding. `plan_binding.artifact_schema_version`
stays 1, and the result envelope stays at version 4.
`v17/sweep.json` pins this declaration and `v1/sweep.json` mirrors it;
`v16/sweep.json` remains retained.

**Declaration version 9 (unpack)** adds the refusal code `private_copy_unopened`,
retry `later`, subject kind `input`, for `extract` and `list`, with exit 3.
unpack gives the system `unzip` its private copy of the archive as `/dev/fd/N`.
On macOS, `unzip` can fail to open that path while another process opens a
`/dev/fd` path with the same number. unpack then starts `unzip` again, up to 3
times in total, and returns this code only when every attempt failed. The
archive was not read, so the code says nothing about it; the same request may
work when run again. Before this version the same failure was reported as
`archive_unreadable` or `unsafe_input`. No read or write domain changes, and
the result envelope stays at version 4. `check-tool-contracts.py` cannot stage
the race and names the code as unproven; unpack's unit tests close the
descriptor to reach it. `v9/unpack.json` pins this declaration and
`v1/unpack.json` mirrors it; `v8/unpack.json` remains retained.
