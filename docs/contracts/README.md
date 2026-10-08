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
| `selected_tree_metadata` | Directory entries, names, types, sizes, modes, dates, devices and inodes under the requested root, within the tool's scan depth and skip rules |
| `ancestor_metadata` | Ancestor names and metadata used for system-location, cloud-sync and filesystem checks |
| `project_marker_names` | Names of project markers; no parsing of marker contents |
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
digests are checked by the witness. Canonical pins in `v2/` retain the version-2 declarations; `v3/unpack.json`
pins the current version-3 unpack declaration in a version-2 result envelope.
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

These are finite falsification probes. They do not observe every read attempt,
prove absence of ordinary-file reads, check subprocess networking or cover every
conditional path (including keychain, EXDEV, review and crash recovery). Those
claims remain unproven by this witness. Broader existing unit/stress witnesses
exercise some of those paths separately.

## Result envelopes and read receipts

Schema version 2 wraps every `--json` response, including parser refusals,
operational errors and recovered panics. Plain `contract` also returns this
envelope. Existing contract and operation fields move under `details`; consumers
must check `schema_version` before accessing that field. The original schema-version-1
declarations are retained verbatim under `history/v1/`. The published
`v1/{sweep,stash,unpack}.json` fixtures mirror the current canonical
contract envelopes (`v2/` for sweep and stash, `v3/` for unpack); `v2/envelope.json` pins the result structure. Editing fields or types without
adding a new version is rejected by the independent binary witness.

The envelope contains `schema_version`, `tool_version`, `operation_id`, `status`,
`scope`, `observations`, `effects`, `verification`, `recovery`, `disclosure` and
`details`. Status is one of `done`, `nothing_to_do`, `refused`, `incomplete` or
`error`. Exit codes retain their existing meaning. Incomplete move or restore
attempts require checking the filesystem and the journal before retrying.

`scope` identifies the tool and operation and includes its conditional declared
read/write domains and startup domains. Those are capabilities, not observations.
`observations.categories` records instrumented read operations by category:
metadata, directory enumeration, access opens, encrypted journal bytes, key
acquisition, fingerprint bytes, cross-device copying and archive copying.
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

## Unpack quarantine declaration version 3

Unpack's capability declaration under `details.schema_version` is version 3;
its surrounding result envelope remains version 2. `v3/unpack.json` pins the
new declaration and `v1/unpack.json` mirrors that current fixture. The preceding
`v2/unpack.json` stays unchanged; sweep and stash declarations remain version 2.
The declaration names captured archive quarantine metadata, staging attribute
reads and writes, exact readback before publication, and refusal if preservation
fails. On exFAT, macOS-generated AppleDouble companions are retained when they
provide the attribute representation. Raw values are never disclosed.
