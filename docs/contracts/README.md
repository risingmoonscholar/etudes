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

The immutable canonical pins in `v1/sweep.json`, `v1/stash.json` and
`v1/unpack.json` fix all fields, types and declared semantics. Only the per-run
operation ID and manifest version are normalized. Changing the contract without
bumping `SCHEMA_VERSION` fails `cargo test`. To evolve it, retain the old pins,
increase the version, and add all three pins under the new version directory.
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

This is the first stage of the tool handoff. Ordinary operation JSON still uses
the existing per-tool shapes. Read receipts with attempted/observed/verified/
failed/unproven evidence, the shared result envelope, named verification claims,
recovery/disclosure fields and result-schema version pins remain to be built.
