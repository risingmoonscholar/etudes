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
every operation except `contract`. In particular, a sweep scan may delete expired
journals belonging to either tool, and stash status can acquire a key and migrate
state. `unpack --list` copies archive bytes into a private temporary directory
and removes that copy; absence of a published destination does not mean no writes.

Scope domains resolve as follows:

| Domain | Meaning |
| --- | --- |
| `selected_tree_metadata` | Directory entries, names, types, sizes, modes, dates, devices and inodes under the requested root, within the tool's scan depth and skip rules. For items sweep may move this includes whether a Finder tag attribute is present, asked through a zero-length buffer so tag names are never read; Finder comments are never queried. The read is counted in the `finder_tag_presence` observation category |
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
| `expired_journals_all_tools` | Journals older than the core TTL, irrespective of namespace, pruned at sweep startup |
| `journal_key`, `keychain_if_no_supplied_key` | Supplied key or shared login keychain key; acquisition may create a key when none exists |
| `sweep_journals`, `shared_keychain_key_if_authorized` | Forget removes sweep's journals; destroying the shared key has separate guards |
| `empty_group_directories`, `empty_holding_directories` | Empty directories removed after restoration |
| `terminal_input` | Review, inspection or key destruction consent and choices |
| `archive_bytes`, `archive_metadata` | The requested archive's bytes and filesystem metadata |
| `private_archive_copy`, `os_random_bytes` | A temporary private copy of the archive and /dev/urandom bytes for its name |
| `destination_parent_metadata`, `volume_free_space` | Destination checks and the volume measurement used for the default soft extraction budget |
| `staging_tree_metadata`, `private_staging_tree` | Audits of the actual extracted tree, and private extraction, junk removal and failure cleanup beside the destination |
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

The declarations do not enforce an OS sandbox. Network policy is `none`, runtime
enforcement is `none`, and subprocess network behaviour is unverified. Consumers
must treat declarations separately from observed or independently verified access.

The original immutable declarations remain under `history/v1/`; their byte
digests are checked by the witness. Canonical pins in `v2/` retain the preceding version-2 declarations; `v3/`
pins the preceding version-3 declarations. `v4/` pins version-4 declarations;
`v5/` pins version-5 declarations of all three tools. Later directories pin each
later declaration: `v11/sweep.json`, `v10/stash.json` and `v6/unpack.json` are the
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
contract envelopes (`v11/sweep.json`, `v10/stash.json` and `v6/unpack.json`);
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
result uses `done` or `nothing_to_do` with `retry: "not_needed"`. Adding this
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

Unpack's capability declaration under `details.schema_version` is version 6;
its surrounding result envelope is version 4. `v6/unpack.json` pins the
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
declaration is version 10 and its surrounding result envelope is version 4.
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
declaration is version 10 in `v10/stash.json`; `v1/stash.json` mirrors it, while
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
