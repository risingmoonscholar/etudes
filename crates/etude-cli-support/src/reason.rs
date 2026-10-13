//! Stable machine-readable outcome codes shared by the command line tools.

use etude_core::json as j;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryClass {
    NotNeeded,
    Never,
    AfterChange,
    Later,
}

impl RetryClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotNeeded => "not_needed",
            Self::Never => "never",
            Self::AfterChange => "after_change",
            Self::Later => "later",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    Done,
    NothingToDo,
    InvalidArguments,
    UnknownOption,
    MissingInput,
    UnsupportedFormat,
    InsufficientSpace,
    InternalError,
    ProjectFolder,
    DestinationExists,
    DestinationOutsideRoot,
    PlanStale,
    ProfileMissing,
    FolderBusy,
    RecentFiles,
    DownloadInFlight,
    NotDueYet,
    JournalUnreadable,
    JournalKeyUnavailable,
    UnknownOperation,
    UnsafeInput,
    PermissionDenied,
    EmptySelection,
    OperationIncomplete,
    UserCancelled,
}

impl Code {
    pub const REFUSAL_CODES: &[Self] = &[
        Self::InvalidArguments,
        Self::UnknownOption,
        Self::MissingInput,
        Self::UnsupportedFormat,
        Self::InsufficientSpace,
        Self::InternalError,
        Self::ProjectFolder,
        Self::DestinationExists,
        Self::DestinationOutsideRoot,
        Self::PlanStale,
        Self::ProfileMissing,
        Self::FolderBusy,
        Self::RecentFiles,
        Self::DownloadInFlight,
        Self::NotDueYet,
        Self::JournalUnreadable,
        Self::JournalKeyUnavailable,
        Self::UnknownOperation,
        Self::UnsafeInput,
        Self::PermissionDenied,
        Self::EmptySelection,
        Self::OperationIncomplete,
        Self::UserCancelled,
    ];
}

#[derive(Debug, Clone, Copy)]
pub struct CodeSpec {
    pub code: Code,
    pub name: &'static str,
    pub retry: RetryClass,
    pub subject_kind: Option<&'static str>,
    pub operations: &'static [&'static str],
    pub explanation: &'static str,
}

pub const CODES: &[CodeSpec] = &[
    CodeSpec {
        code: Code::InvalidArguments,
        name: "invalid_arguments",
        retry: RetryClass::Never,
        subject_kind: None,
        operations: &["all"],
        explanation: "The arguments do not form a supported request.",
    },
    CodeSpec {
        code: Code::UnknownOption,
        name: "unknown_option",
        retry: RetryClass::Never,
        subject_kind: None,
        operations: &["all"],
        explanation: "An option is not recognized by this command.",
    },
    CodeSpec {
        code: Code::MissingInput,
        name: "missing_input",
        retry: RetryClass::AfterChange,
        subject_kind: Some("input"),
        operations: &["all"],
        explanation: "The requested input is absent or no longer available; choose an existing input.",
    },
    CodeSpec {
        code: Code::UnsupportedFormat,
        name: "unsupported_format",
        retry: RetryClass::Never,
        subject_kind: None,
        operations: &["unpack:extract"],
        explanation: "This format is outside the tool's supported set.",
    },
    CodeSpec {
        code: Code::InsufficientSpace,
        name: "insufficient_space",
        retry: RetryClass::AfterChange,
        subject_kind: Some("destination"),
        operations: &["unpack:extract"],
        explanation: "The destination volume lacks the working space this request needs; choose a suitable destination or reduce the input.",
    },
    CodeSpec {
        code: Code::InternalError,
        name: "internal_error",
        retry: RetryClass::Never,
        subject_kind: None,
        operations: &["all"],
        explanation: "The tool stopped on an internal failure; inspect its result and local state before choosing another action.",
    },
    CodeSpec {
        code: Code::ProjectFolder,
        name: "project_folder",
        retry: RetryClass::Never,
        subject_kind: None,
        operations: &["sweep:scan", "sweep:apply", "stash:stash"],
        explanation: "The folder is a project and this request is refused for that folder.",
    },
    CodeSpec {
        code: Code::DestinationExists,
        name: "destination_exists",
        retry: RetryClass::AfterChange,
        subject_kind: Some("destination"),
        operations: &[
            "sweep:apply",
            "sweep:undo",
            "stash:stash",
            "stash:pop",
            "unpack:extract",
        ],
        explanation: "The destination is occupied; choose or make a vacant destination before retrying.",
    },
    CodeSpec {
        code: Code::DestinationOutsideRoot,
        name: "destination_outside_root",
        retry: RetryClass::AfterChange,
        subject_kind: Some("destination"),
        operations: &["sweep:apply", "stash:stash"],
        explanation: "The destination is outside the selected plan root; change the destination or plan.",
    },
    CodeSpec {
        code: Code::PlanStale,
        name: "plan_stale",
        retry: RetryClass::AfterChange,
        subject_kind: Some("plan"),
        operations: &["sweep:apply", "stash:stash"],
        explanation: "The observed plan is stale; create a fresh plan before applying.",
    },
    CodeSpec {
        code: Code::ProfileMissing,
        name: "profile_missing",
        retry: RetryClass::AfterChange,
        subject_kind: Some("profile"),
        operations: &["sweep:scan", "sweep:apply"],
        explanation: "The selected profile is unavailable; restore or select a valid profile.",
    },
    CodeSpec {
        code: Code::FolderBusy,
        name: "folder_busy",
        retry: RetryClass::Later,
        subject_kind: Some("other_operation"),
        operations: &["sweep:apply", "sweep:undo", "stash:stash", "stash:pop"],
        explanation: "Another operation currently holds the folder; the same request may work after it releases the folder.",
    },
    CodeSpec {
        code: Code::RecentFiles,
        name: "recent_files",
        retry: RetryClass::Later,
        subject_kind: Some("recent_files"),
        operations: &["sweep:scan", "sweep:apply"],
        explanation: "The files are within the configured grace window and may be judged later.",
    },
    CodeSpec {
        code: Code::DownloadInFlight,
        name: "download_in_flight",
        retry: RetryClass::Later,
        subject_kind: Some("download_in_flight"),
        operations: &["unpack:extract"],
        explanation: "The input appears to still be downloading and may be checked later.",
    },
    CodeSpec {
        code: Code::NotDueYet,
        name: "not_due_yet",
        retry: RetryClass::Later,
        subject_kind: Some("journal"),
        operations: &["stash:pop"],
        explanation: "The journal entry is not due yet; the same request may succeed later without changing the caller's request.",
    },
    CodeSpec {
        code: Code::JournalUnreadable,
        name: "journal_unreadable",
        retry: RetryClass::Later,
        subject_kind: Some("journal"),
        operations: &[
            "sweep:apply",
            "sweep:undo",
            "sweep:status",
            "stash:stash",
            "stash:pop",
            "stash:status",
        ],
        explanation: "The journal cannot be read now; the same read-only request may work after the journal becomes readable.",
    },
    CodeSpec {
        code: Code::JournalKeyUnavailable,
        name: "journal_key_unavailable",
        retry: RetryClass::Later,
        subject_kind: Some("journal"),
        operations: &[
            "sweep:apply",
            "sweep:undo",
            "sweep:status",
            "stash:stash",
            "stash:pop",
            "stash:status",
        ],
        explanation: "The required journal key is unavailable and may become available later.",
    },
    CodeSpec {
        code: Code::UnknownOperation,
        name: "unknown_operation",
        retry: RetryClass::Never,
        subject_kind: None,
        operations: &["sweep:status", "stash:status"],
        explanation: "No journal contains the requested durable operation id.",
    },
    CodeSpec {
        code: Code::UnsafeInput,
        name: "unsafe_input",
        retry: RetryClass::AfterChange,
        subject_kind: Some("input"),
        operations: &["sweep:scan", "sweep:apply", "stash:stash", "unpack:extract"],
        explanation: "The input violates a safety rule; change the input before retrying.",
    },
    CodeSpec {
        code: Code::PermissionDenied,
        name: "permission_denied",
        retry: RetryClass::AfterChange,
        subject_kind: Some("input"),
        operations: &["all"],
        explanation: "The request lacks required access; change the relevant permissions before retrying.",
    },
    CodeSpec {
        code: Code::EmptySelection,
        name: "empty_selection",
        retry: RetryClass::AfterChange,
        subject_kind: Some("input"),
        operations: &["sweep:apply", "stash:stash"],
        explanation: "The selection contains no applicable entries; change the selection.",
    },
    CodeSpec {
        code: Code::OperationIncomplete,
        name: "operation_incomplete",
        retry: RetryClass::AfterChange,
        subject_kind: Some("journal"),
        operations: &[
            "sweep:apply",
            "sweep:undo",
            "stash:stash",
            "stash:pop",
            "unpack:extract",
        ],
        explanation: "The operation stopped part-way; inspect its journal or filesystem and resolve the reported condition before continuing.",
    },
    CodeSpec {
        code: Code::UserCancelled,
        name: "user_cancelled",
        retry: RetryClass::Never,
        subject_kind: None,
        operations: &["sweep:review", "sweep:forget", "stash:select"],
        explanation: "The user declined or cancelled this request.",
    },
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subject {
    kind: &'static str,
    path: Option<String>,
    seconds: Option<u64>,
}

impl Subject {
    pub fn path(kind: &'static str, path: &str) -> Self {
        Self {
            kind,
            path: Some(path.into()),
            seconds: None,
        }
    }

    pub fn wait(kind: &'static str, seconds: Option<u64>) -> Self {
        Self {
            kind,
            path: None,
            seconds,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reason {
    code: Code,
    retry: RetryClass,
    subject: Option<Subject>,
}

impl Reason {
    pub const fn completed() -> Self {
        Self {
            code: Code::Done,
            retry: RetryClass::NotNeeded,
            subject: None,
        }
    }

    pub const fn nothing_to_do() -> Self {
        Self {
            code: Code::NothingToDo,
            retry: RetryClass::NotNeeded,
            subject: None,
        }
    }

    pub fn refusal(code: Code, subject: Option<Subject>) -> Self {
        let spec = CODES
            .iter()
            .find(|spec| spec.code == code)
            .expect("refusal code has a contract entry");
        assert_eq!(
            spec.subject_kind.is_some(),
            subject.is_some(),
            "refusal subject presence disagrees with the code contract"
        );
        if let (Some(expected), Some(actual)) = (spec.subject_kind, &subject) {
            assert_eq!(
                expected, actual.kind,
                "refusal subject kind disagrees with the code contract"
            );
        }
        Self {
            code,
            retry: spec.retry,
            subject,
        }
    }

    pub fn json(&self) -> String {
        let spec = if matches!(self.code, Code::Done | Code::NothingToDo) {
            None
        } else {
            CODES.iter().find(|spec| spec.code == self.code)
        };
        if let Some(spec) = spec {
            assert_eq!(
                spec.retry, self.retry,
                "refusal retry class disagrees with the code contract"
            );
        }
        let mut fields = vec![
            ("code", j::str(self.name())),
            ("retry", j::str(self.retry.as_str())),
        ];
        if let Some(subject) = &self.subject {
            let mut subject_fields = vec![("kind", j::str(subject.kind))];
            if let Some(path) = subject.path.as_deref() {
                subject_fields.push(("path", j::str(path)));
            }
            if let Some(seconds) = subject.seconds {
                subject_fields.push(("seconds", j::num(seconds)));
            }
            fields.push(("subject", j::obj(&subject_fields)));
        }
        j::obj(&fields)
    }

    pub const fn name(&self) -> &'static str {
        match self.code {
            Code::Done => "done",
            Code::NothingToDo => "nothing_to_do",
            Code::InvalidArguments => "invalid_arguments",
            Code::UnknownOption => "unknown_option",
            Code::MissingInput => "missing_input",
            Code::UnsupportedFormat => "unsupported_format",
            Code::InsufficientSpace => "insufficient_space",
            Code::InternalError => "internal_error",
            Code::ProjectFolder => "project_folder",
            Code::DestinationExists => "destination_exists",
            Code::DestinationOutsideRoot => "destination_outside_root",
            Code::PlanStale => "plan_stale",
            Code::ProfileMissing => "profile_missing",
            Code::FolderBusy => "folder_busy",
            Code::RecentFiles => "recent_files",
            Code::DownloadInFlight => "download_in_flight",
            Code::NotDueYet => "not_due_yet",
            Code::JournalUnreadable => "journal_unreadable",
            Code::JournalKeyUnavailable => "journal_key_unavailable",
            Code::UnknownOperation => "unknown_operation",
            Code::UnsafeInput => "unsafe_input",
            Code::PermissionDenied => "permission_denied",
            Code::EmptySelection => "empty_selection",
            Code::OperationIncomplete => "operation_incomplete",
            Code::UserCancelled => "user_cancelled",
        }
    }
}

pub fn codes_json(tool: &str) -> String {
    let tool_operations: &[&str] = match tool {
        "sweep" => &[
            "scan",
            "apply",
            "undo",
            "forget",
            "verify",
            "lesson",
            "review",
            "export_plan",
            "apply_exported_plan",
            "status",
        ],
        "stash" => &[
            "stash",
            "pop",
            "status",
            "inspect",
            "select",
            "export_plan",
            "apply_exported_plan",
        ],
        "unpack" => &["extract", "list"],
        _ => &[],
    };
    j::arr(
        CODES
            .iter()
            .filter(|spec| {
                spec.operations.contains(&"all")
                    || spec
                        .operations
                        .iter()
                        .any(|op| op.starts_with(&format!("{tool}:")))
            })
            .map(|spec| {
                let operations = tool_operations.iter().filter(|operation| {
                    spec.operations.contains(&"all")
                        || spec
                            .operations
                            .contains(&format!("{tool}:{operation}").as_str())
                });
                j::obj(&[
                    ("code", j::str(spec.name)),
                    ("retry", j::str(spec.retry.as_str())),
                    (
                        "subject_kind",
                        spec.subject_kind
                            .map(j::str)
                            .unwrap_or_else(|| "null".into()),
                    ),
                    (
                        "operations",
                        j::arr(operations.map(|operation| j::str(operation))),
                    ),
                    ("why", j::str(spec.explanation)),
                ])
            }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_classes_and_subjects_are_typed_and_stable() {
        assert_eq!(RetryClass::NotNeeded.as_str(), "not_needed");
        assert_eq!(RetryClass::Never.as_str(), "never");
        assert_eq!(RetryClass::AfterChange.as_str(), "after_change");
        assert_eq!(RetryClass::Later.as_str(), "later");
        let reason = Reason::refusal(
            Code::FolderBusy,
            Some(Subject::path("other_operation", "/Desktop")),
        );
        assert_eq!(
            reason.json(),
            r#"{"code":"folder_busy","retry":"later","subject":{"kind":"other_operation","path":"/Desktop"}}"#
        );
    }

    #[test]
    fn catalog_codes_are_snake_case_and_retryable_codes_name_a_subject() {
        assert_eq!(CODES.len(), Code::REFUSAL_CODES.len());
        for spec in CODES {
            assert!(
                spec.name
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
            );
            assert!(!spec.operations.is_empty());
            assert_eq!(
                spec.retry != RetryClass::Never,
                spec.subject_kind.is_some(),
                "{}",
                spec.name
            );
            assert!(Code::REFUSAL_CODES.contains(&spec.code));
        }
        for code in Code::REFUSAL_CODES {
            assert!(CODES.iter().any(|spec| spec.code == *code));
        }
        let sweep = codes_json("sweep");
        assert!(sweep.contains("\"code\":\"folder_busy\""));
        assert!(sweep.contains("\"retry\":\"later\""));
        assert!(sweep.contains("\"subject_kind\":\"other_operation\""));
        assert!(sweep.contains("\"subject_kind\":null"));
        assert!(!sweep.contains("\"all\""));
        assert!(!codes_json("unpack").contains("\"code\":\"folder_busy\""));
    }

    #[test]
    fn insufficient_space_and_internal_failure_have_explicit_contract_codes() {
        let space = Reason::refusal(
            Code::InsufficientSpace,
            Some(Subject::wait("destination", None)),
        );
        assert_eq!(
            space.json(),
            r#"{"code":"insufficient_space","retry":"after_change","subject":{"kind":"destination"}}"#
        );
        let internal = Reason::refusal(Code::InternalError, None);
        assert_eq!(
            internal.json(),
            r#"{"code":"internal_error","retry":"never"}"#
        );
        assert!(codes_json("unpack").contains("\"code\":\"insufficient_space\""));
    }

    #[test]
    #[should_panic(expected = "refusal subject presence disagrees")]
    fn retryable_reason_cannot_be_built_without_its_subject() {
        let _ = Reason::refusal(Code::FolderBusy, None);
    }
}
