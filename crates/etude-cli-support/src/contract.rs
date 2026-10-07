//! Versioned capability declarations, not receipts or evidence of access.
//!
//! The binary probes in scripts/check-tool-contracts.py pin these declarations
//! and compare them with independent filesystem observations. A declaration
//! describes what is allowed; it never certifies what happened in a run.

use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use etude_core::json as j;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy)]
pub enum Tool {
    Sweep,
    Stash,
    Unpack,
}

/// This table is also the unpack dispatch table. Aliases are real formats on
/// the command surface and must be enumerated, not hidden behind a family name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFormat {
    Zip,
    Tar,
    TarGz,
    TarBz2,
    TarXz,
    Gz,
    Dmg,
}

pub const ARCHIVE_SUFFIXES: &[(&str, ArchiveFormat)] = &[
    (".tar.gz", ArchiveFormat::TarGz),
    (".tgz", ArchiveFormat::TarGz),
    (".tar.bz2", ArchiveFormat::TarBz2),
    (".tbz", ArchiveFormat::TarBz2),
    (".tar.xz", ArchiveFormat::TarXz),
    (".txz", ArchiveFormat::TarXz),
    (".tar", ArchiveFormat::Tar),
    (".zip", ArchiveFormat::Zip),
    (".jar", ArchiveFormat::Zip),
    (".dmg", ArchiveFormat::Dmg),
    (".gz", ArchiveFormat::Gz),
];

fn strings(values: &[&str]) -> String {
    j::arr(values.iter().map(|s| j::str(s)))
}

fn scope(operation: &str, reads: &[&str], writes: &[&str]) -> String {
    j::obj(&[
        ("operation", j::str(operation)),
        ("reads", strings(reads)),
        ("writes", strings(writes)),
    ])
}

fn formats(supported: &[&str], unsupported: &[&str]) -> String {
    j::obj(&[
        ("supported", strings(supported)),
        ("unsupported", strings(unsupported)),
        ("unlisted", j::str("unsupported")),
        ("matching", j::str("case_insensitive_suffix")),
    ])
}

/// Handle only the contract command, before migration, pruning, key acquisition
/// or any filesystem work. No environment values, filenames or keys are output.
pub fn command(
    tool: Tool,
    version: &str,
    args: &[String],
    text_exts: &[&str],
    text_max_bytes: Option<usize>,
) -> ExitCode {
    if args.len() > 2 || args.get(1).is_some_and(|arg| arg != "--json") {
        eprintln!("contract accepts only --json");
        return ExitCode::from(2);
    }
    println!("{}", declaration(tool, version, text_exts, text_max_bytes));
    ExitCode::SUCCESS
}

pub fn declaration(
    tool: Tool,
    version: &str,
    text_exts: &[&str],
    text_max_bytes: Option<usize>,
) -> String {
    let name = match tool {
        Tool::Sweep => "sweep",
        Tool::Stash => "stash",
        Tool::Unpack => "unpack",
    };
    let custody = !matches!(tool, Tool::Unpack);
    let operations = match tool {
        Tool::Sweep => vec![
            scope(
                "scan",
                &[
                    "selected_tree_metadata",
                    "ancestor_metadata",
                    "project_marker_names",
                ],
                &[],
            ),
            scope(
                "scan_with_inspect_content",
                &[
                    "selected_tree_metadata",
                    "ancestor_metadata",
                    "project_marker_names",
                    "consented_text_prefixes",
                ],
                &[],
            ),
            scope(
                "apply",
                &[
                    "selected_tree_metadata",
                    "ancestor_metadata",
                    "project_marker_names",
                    "journal_key",
                    "cross_device_source_bytes",
                ],
                &[
                    "selected_tree_entries",
                    "journal_store",
                    "keychain_if_no_supplied_key",
                ],
            ),
            scope(
                "review",
                &[
                    "selected_tree_metadata",
                    "ancestor_metadata",
                    "project_marker_names",
                    "terminal_input",
                    "journal_key",
                    "cross_device_source_bytes",
                ],
                &[
                    "selected_tree_entries",
                    "journal_store",
                    "keychain_if_no_supplied_key",
                ],
            ),
            scope(
                "undo",
                &[
                    "journal_store",
                    "journal_key",
                    "journal_recorded_paths_metadata",
                    "cross_device_source_bytes",
                ],
                &[
                    "journal_recorded_entries",
                    "journal_store",
                    "empty_group_directories",
                    "keychain_if_no_supplied_key",
                ],
            ),
            scope(
                "forget",
                &["journal_store_metadata", "terminal_input"],
                &["sweep_journals", "shared_keychain_key_if_authorized"],
            ),
            scope(
                "verify",
                &["journal_store_metadata", "ancestor_metadata"],
                &[],
            ),
            scope("lesson", &[], &[]),
        ],
        Tool::Stash => vec![
            scope(
                "stash",
                &[
                    "selected_tree_metadata",
                    "ancestor_metadata",
                    "journal_key",
                    "cross_device_source_bytes",
                ],
                &[
                    "selected_tree_entries",
                    "journal_store",
                    "keychain_if_no_supplied_key",
                ],
            ),
            scope(
                "pop",
                &[
                    "journal_store",
                    "journal_key",
                    "journal_recorded_paths_metadata",
                    "cross_device_source_bytes",
                ],
                &[
                    "journal_recorded_entries",
                    "journal_store",
                    "empty_holding_directories",
                    "keychain_if_no_supplied_key",
                ],
            ),
            scope(
                "status",
                &[
                    "selected_tree_metadata",
                    "journal_store",
                    "journal_key",
                    "journal_recorded_paths_metadata",
                ],
                &["keychain_if_no_supplied_key"],
            ),
            scope(
                "status_all",
                &[
                    "journal_store",
                    "journal_key",
                    "journal_recorded_paths_metadata",
                ],
                &["keychain_if_no_supplied_key"],
            ),
        ],
        Tool::Unpack => vec![
            scope(
                "list",
                &[
                    "archive_bytes",
                    "archive_metadata",
                    "private_archive_copy",
                    "os_random_bytes",
                ],
                &["private_archive_copy"],
            ),
            scope(
                "extract",
                &[
                    "archive_bytes",
                    "archive_metadata",
                    "private_archive_copy",
                    "destination_parent_metadata",
                    "volume_free_space",
                    "staging_tree_metadata",
                    "os_random_bytes",
                ],
                &[
                    "private_archive_copy",
                    "private_staging_tree",
                    "new_destination_tree",
                ],
            ),
        ],
    };
    let startup = match tool {
        Tool::Sweep => scope(
            "startup_except_contract",
            &["journal_store_metadata", "legacy_state_metadata"],
            &["legacy_state_migration", "expired_journals_all_tools"],
        ),
        Tool::Stash => scope(
            "startup_except_contract",
            &["legacy_state_metadata"],
            &["legacy_state_migration"],
        ),
        Tool::Unpack => scope("startup_except_contract", &[], &[]),
    };
    let content_formats = if matches!(tool, Tool::Sweep) {
        let suffixes: Vec<String> = text_exts.iter().map(|ext| format!(".{ext}")).collect();
        let suffixes: Vec<&str> = suffixes.iter().map(String::as_str).collect();
        formats(
            &suffixes,
            &[
                ".pdf", ".doc", ".docx", ".xls", ".xlsx", ".ppt", ".pptx", ".zip", ".tar", ".gz",
                ".jpg", ".png",
            ],
        )
    } else {
        formats(&[], &["all"])
    };
    let supported_archives: Vec<&str> = ARCHIVE_SUFFIXES
        .iter()
        .filter(|(_, format)| *format != ArchiveFormat::Dmg)
        .map(|(suffix, _)| *suffix)
        .collect();
    let archive_formats = if matches!(tool, Tool::Unpack) {
        formats(
            &supported_archives,
            &[
                ".dmg", ".rar", ".7z", ".bz2", ".xz", ".tbz2", ".zst", ".tar.zst",
            ],
        )
    } else {
        formats(&[], &["all"])
    };
    let operation_id = format!(
        "contract-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    j::obj(&[
        ("schema_version", j::num(SCHEMA_VERSION)),
        ("tool_version", j::str(version)),
        ("operation_id", j::str(&operation_id)),
        ("status", j::str("done")),
        ("tool", j::str(name)),
        ("kind", j::str("capability_contract")),
        ("contract_query", scope("contract", &[], &[])),
        (
            "observation_scope",
            j::obj(&[
                (
                    "default_user_content",
                    j::str(if custody {
                        "metadata_only"
                    } else {
                        "archive_bytes"
                    }),
                ),
                (
                    "content_exceptions",
                    strings(match tool {
                        Tool::Sweep => &["explicit_tty_consent_text_prefix", "cross_device_copy"],
                        Tool::Stash => &["cross_device_copy"],
                        Tool::Unpack => &["system_extractor_reads_private_archive_copy"],
                    }),
                ),
                (
                    "symlinks",
                    j::str(if matches!(tool, Tool::Stash) {
                        "move_link_itself"
                    } else {
                        "refuse_or_skip"
                    }),
                ),
                (
                    "environment",
                    strings(match tool {
                        Tool::Sweep => &[
                            "HOME",
                            "TMPDIR",
                            "ETUDE_STATE_DIR",
                            "ETUDE_JOURNAL_KEY",
                            "SWEEP_GRACE_SECS",
                        ],
                        Tool::Stash => &["HOME", "TMPDIR", "ETUDE_STATE_DIR", "ETUDE_JOURNAL_KEY"],
                        Tool::Unpack => &["HOME", "TMPDIR"],
                    }),
                ),
                ("subprocess_environment", j::str("inherited")),
            ]),
        ),
        (
            "mutation_scope",
            j::obj(&[("operations", j::arr(operations)), ("startup", startup)]),
        ),
        (
            "network",
            j::obj(&[
                ("policy", j::str("none")),
                ("runtime_enforcement", j::str("none")),
                (
                    "subprocesses",
                    strings(if custody {
                        &["/usr/bin/security"]
                    } else {
                        &["/usr/bin/unzip", "/usr/bin/tar", "/usr/bin/gunzip"]
                    }),
                ),
                ("subprocess_network_verified", j::bool(false)),
            ]),
        ),
        (
            "overwrite",
            j::obj(&[
                ("user_destinations", j::str("refused")),
                (
                    "owned_state",
                    j::str(if custody {
                        "journal_updates_replace"
                    } else {
                        "private_staging_only"
                    }),
                ),
            ]),
        ),
        (
            "deletion",
            j::obj(&[
                (
                    "source_entries",
                    j::str(if custody {
                        "relocation_and_size_checked_copy_unlink"
                    } else {
                        "never"
                    }),
                ),
                ("user_payloads", j::str("never")),
                (
                    "owned_artifacts",
                    strings(match tool {
                        Tool::Sweep => &[
                            "expired_journals_all_tools",
                            "sweep_journals_on_forget",
                            "authorized_shared_key",
                            "empty_group_directories",
                            "case_probe_files",
                        ],
                        Tool::Stash => &["empty_holding_directories", "case_probe_files"],
                        Tool::Unpack => &[
                            "private_archive_copy",
                            "failed_staging_tree",
                            "junk_members_in_staging",
                        ],
                    }),
                ),
            ]),
        ),
        (
            "reversibility",
            j::obj(&[
                (
                    "mode",
                    j::str(if custody {
                        "conditional_journal_restore"
                    } else {
                        "no_undo"
                    }),
                ),
                (
                    "conditions",
                    strings(if custody {
                        &[
                            "readable_journal",
                            "same_key",
                            "unchanged_items",
                            "vacant_original_paths",
                            "journal_not_expired",
                            "journal_progress_writable",
                        ]
                    } else {
                        &["source_archive_retained"]
                    }),
                ),
                (
                    "disabled_by",
                    strings(if custody {
                        &["--no-journal", "journal_or_key_loss"]
                    } else {
                        &[]
                    }),
                ),
            ]),
        ),
        (
            "persistent_state",
            j::obj(&[
                (
                    "journal_store",
                    j::str(if custody {
                        "ETUDE_STATE_DIR_or_HOME/Library/Application Support/etudes"
                    } else {
                        "none"
                    }),
                ),
                (
                    "journal_namespace",
                    j::str(if custody { name } else { "none" }),
                ),
                (
                    "journal_ttl_days",
                    if custody {
                        j::num(etude_core::journal::TTL_DAYS)
                    } else {
                        "null".into()
                    },
                ),
                (
                    "key",
                    j::str(if custody {
                        "supplied_environment_or_shared_login_keychain"
                    } else {
                        "none"
                    }),
                ),
                (
                    "result_artifacts",
                    strings(match tool {
                        Tool::Sweep => &["group_directories"],
                        Tool::Stash => &["holding_directory_deadline_in_name"],
                        Tool::Unpack => &["destination_tree"],
                    }),
                ),
            ]),
        ),
        (
            "formats",
            j::obj(&[
                (
                    "filesystem_handling",
                    j::str(match tool {
                        Tool::Sweep => "visible_regular_files_by_metadata_any_extension",
                        Tool::Stash => {
                            "visible_files_directories_packages_and_links_opaque_any_extension"
                        }
                        Tool::Unpack => "archive_suffix_allowlist",
                    }),
                ),
                ("content_inspection", content_formats),
                ("archive_extraction", archive_formats),
            ]),
        ),
        (
            "limits",
            j::obj(&[
                ("platform", j::str("macos")),
                (
                    "inspection_max_bytes",
                    text_max_bytes.map(j::num).unwrap_or_else(|| "null".into()),
                ),
                (
                    "extraction_budget",
                    j::str(if custody {
                        "not_applicable"
                    } else {
                        "monitored_soft_limit_not_hard_cap"
                    }),
                ),
            ]),
        ),
        (
            "evidence",
            j::obj(&[
                ("kind", j::str("declaration_not_receipt")),
                ("access_verified", j::bool(false)),
                ("witness", j::str("scripts/check-tool-contracts.py")),
            ]),
        ),
    ])
}
