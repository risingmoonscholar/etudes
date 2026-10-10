//! Versioned capability declarations, not receipts or evidence of access.
//!
//! The binary probes in scripts/check-tool-contracts.py pin these declarations
//! and compare them with independent filesystem observations. A declaration
//! describes what is allowed; it never certifies what happened in a run.

use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use etude_core::json as j;

pub const SCHEMA_VERSION: u32 = 3;

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
    crate::envelope::print(declaration(tool, version, text_exts, text_max_bytes));
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
    let operations = operation_scopes(tool);
    let startup = startup_scope(tool);
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
    let mut fields = vec![
        (
            "schema_version",
            j::num(if matches!(tool, Tool::Stash) {
                4
            } else {
                SCHEMA_VERSION
            }),
        ),
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
                        Tool::Sweep => &[
                            "explicit_tty_consent_text_prefix",
                            "cross_device_copy",
                            "journal_fingerprint_prefixes",
                        ],
                        Tool::Stash => &[
                            "cross_device_copy",
                            "journal_fingerprint_prefixes",
                            "explicit_selection_path_list_bytes",
                        ],
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
    ];
    if matches!(tool, Tool::Sweep) {
        fields.push(("structured_progress", j::obj(&[
            ("flag", j::str("--progress-json")),
            ("stream", j::str("stderr_ndjson_alongside_diagnostics")),
            ("schema_version", j::num(crate::progress::SCHEMA_VERSION)),
            ("operations", strings(&["apply", "review"])),
            ("fields", strings(&["schema_version", "event", "tool_version", "operation_id", "operation", "phase", "planned", "completed", "journalled"])),
            ("maximum_events", j::num(crate::progress::MAX_EVENTS)),
            ("planned", j::str("entries_after_full_preflight_validation")),
            ("completed", j::str("successful_moves_including_a_move_whose_done_record_failed")),
            ("journalled", j::str("successful_durable_done_records;_zero_without_a_journal")),
            ("ordering", j::str("each_done_record_is_durable_before_callback_and_next_move")),
            ("disclosure", j::str("counts_and_invocation_identity_only;_no_paths_contents_keys_or_environment_values")),
        ])));
    }
    if matches!(tool, Tool::Stash) {
        fields.push(("explicit_selection", j::obj(&[
            ("command", j::str("select")),
            ("sources", j::str("multiple_exact_canonical_files_and_directories;_no_common_ancestor_or_sibling_scan")),
            ("directories", j::str("whole_opaque_objects;_not_recursive_content_integrity_verified")),
            ("path_input", j::str("positional_paths_or_from0_file_or_stdin;_nul_terminated_utf8")),
            ("max_input_bytes", j::num(32 * 1024 * 1024)),
            ("max_objects", j::num(100_000)),
            ("overlaps", j::str("duplicate_paths_repeated_object_identities_and_ancestor_descendant_selections_refused_before_holding_or_moves")),
            ("unsupported", strings(&["explicit_symlinks", "special_files", "cross_volume_directory_holding", "known_sync_sources_or_holding", "non_utf8_journal_paths"])),
            ("journal", j::str("required_encrypted_original_absolute_parent_paths;_no_journal_refused_for_select_only")),
            ("holding", j::str("one_private_operation_root_with_numbered_slots;_equal_basenames_do_not_collide")),
            ("holding_parent", j::str("into_parent_or_first_canonical_source_parent;_inside_selected_object_refused")),
            ("recovery", j::str("stash_pop_operation_root;_authenticated_owned_empty_directories_only")),
            ("disclosure", j::str("counts_and_minimum_recovery_locator_only;_source_paths_and_basenames_not_listed")),
        ])));
    }
    if matches!(tool, Tool::Stash) {
        fields.push(("batches", j::obj(&[
            ("persistent_id", j::str("stash_id_is_the_encrypted_journal_id;_distinct_from_invocation_operation_id")),
            ("identity", j::str("new_stash_journal_v2_authenticates_id_and_tool_before_progress_replay;_legacy_v1_filename_binding_unproven")),
            ("exact_restore", j::str("pop_--id_ID_or_current_creation_ID;_bounded_selector;_missing_or_damaged_id_never_falls_back")),
            ("latest", j::str("latest_live_stash_by_original_creation_nanos_in_current_id;_ties_refused;_legacy_unrecognised_ids_use_modification_time_with_its_tie_barrier")),
            ("latest_scope", j::str("no_path_means_machine_wide_stash_journals;_path_limits_to_exact_journal_root")),
            ("path_restore", j::str("one_live_batch_required;_multiple_matches_refused")),
            ("status", j::str("one_row_per_live_journal;_held_count_from_nofollow_destination_metadata;_redacted_roots_without_paths_disclosure")),
            ("due", j::str("deadline_from_selected_journal_holding_name;_information_only;_if_due_refuses_early_explicit_pop")),
            ("no_journal", j::str("explicit_legacy_mode_has_no_restorable_stash_id;_manual_restore_required")),
        ])));
    }
    if matches!(tool, Tool::Unpack) {
        fields.push(("quarantine", j::obj(&[
            ("source", j::str("captured_from_same_nofollow_descriptor_as_archive_bytes")),
            ("changed_during_copy", j::str("refused_before_extraction")),
            ("output", j::str("captured_attribute_applied_to_every_final_file_and_directory_and_readback_verified_before_publication")),
            ("unsupported_storage", j::str("refuse_publication_and_remove_staging_if_attribute_cannot_be_stored_or_exactly_read_back")),
            ("appledouble", j::str("filesystem_generated_companions_retained_when_they_store_quarantine;_not_a_promise_of_native_xattr_storage")),
            ("absent_source", j::str("no_quarantine_attribute_added_by_unpack")),
            ("disclosure", j::str("attribute_value_never_reported")),
        ])));
    }
    j::obj(&fields)
}

fn operation_scopes(tool: Tool) -> Vec<String> {
    match tool {
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
                    "journal_fingerprint_bytes",
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
                    "journal_fingerprint_bytes",
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
                    "journal_fingerprint_bytes",
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
                    "explicit_selection_object_metadata",
                    "explicit_selection_path_list_bytes",
                    "os_random_bytes_for_selection_holding",
                    "journal_key",
                    "cross_device_source_bytes",
                    "journal_fingerprint_bytes",
                ],
                &[
                    "selected_tree_entries",
                    "explicit_selected_objects",
                    "private_selection_holding_tree",
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
                    "journal_fingerprint_bytes",
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
                    "archive_quarantine_metadata",
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
                    "archive_quarantine_metadata",
                    "private_archive_copy",
                    "destination_parent_metadata",
                    "volume_free_space",
                    "staging_tree_metadata",
                    "staging_quarantine_metadata",
                    "os_random_bytes",
                ],
                &[
                    "private_archive_copy",
                    "private_staging_tree",
                    "staging_quarantine_attributes",
                    "new_destination_tree",
                ],
            ),
        ],
    }
}

fn startup_scope(tool: Tool) -> String {
    match tool {
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
    }
}

pub fn invocation_scope(tool: Tool, operation: &str) -> String {
    let prefix = format!("{{\"operation\":{}", j::str(operation));
    let domains = operation_scopes(tool)
        .into_iter()
        .find(|row| row.starts_with(&prefix))
        .unwrap_or_else(|| scope(operation, &[], &[]));
    j::obj(&[
        (
            "tool",
            j::str(match tool {
                Tool::Sweep => "sweep",
                Tool::Stash => "stash",
                Tool::Unpack => "unpack",
            }),
        ),
        ("operation", j::str(operation)),
        ("domains", domains),
        (
            "startup",
            if operation == "contract" {
                scope("contract", &[], &[])
            } else {
                startup_scope(tool)
            },
        ),
        (
            "evidence",
            j::str("conditional declared domains; observations report actual instrumented reads"),
        ),
    ])
}
