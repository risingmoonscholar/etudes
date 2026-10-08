//! Versioned per-invocation results; receipts describe instrumented reads only.
use etude_core::json as j;
use std::cell::RefCell;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

pub const SCHEMA_VERSION: u32 = 2;
pub const FIELDS: &[&str] = &[
    "schema_version",
    "tool_version",
    "operation_id",
    "status",
    "scope",
    "observations",
    "effects",
    "verification",
    "recovery",
    "disclosure",
    "details",
];

#[derive(Default)]
struct ResultState {
    enabled: bool,
    tool: String,
    version: String,
    operation: String,
    payloads: Vec<String>,
    status: Option<&'static str>,
    scope: String,
    journal_enabled: bool,
    effects: std::collections::BTreeMap<&'static str, usize>,
}
thread_local! { static RESULT: RefCell<ResultState> = RefCell::new(ResultState::default()); }

pub fn begin(tool: &str, version: &str, args: &[String]) {
    etude_core::scan::reset_receipt();
    let operation = match args.first().map(String::as_str) {
        Some(
            "contract" | "apply" | "undo" | "forget" | "verify" | "lesson" | "pop" | "status"
            | "review",
        ) => args[0].clone(),
        _ if tool == "unpack" && args.iter().any(|arg| arg == "--list") => "list".into(),
        _ => match tool {
            "sweep" => "scan",
            "stash" => "stash",
            _ => "extract",
        }
        .into(),
    };
    let operation = if operation == "scan" && args.iter().any(|arg| arg == "--inspect-content") {
        "scan_with_inspect_content".into()
    } else if operation == "status" && args.iter().any(|arg| arg == "--all") {
        "status_all".into()
    } else {
        operation
    };
    let tool_kind = match tool {
        "sweep" => crate::contract::Tool::Sweep,
        "stash" => crate::contract::Tool::Stash,
        _ => crate::contract::Tool::Unpack,
    };
    let scope = crate::contract::invocation_scope(tool_kind, &operation);
    RESULT.with(|state| {
        *state.borrow_mut() = ResultState {
            enabled: args.iter().any(|arg| arg == "--json") || operation == "contract",
            tool: tool.into(),
            version: version.into(),
            operation,
            payloads: Vec::new(),
            status: None,
            scope,
            journal_enabled: !args.iter().any(|arg| arg == "--no-journal"),
            effects: std::collections::BTreeMap::new(),
        }
    });
}

pub fn print(value: String) {
    RESULT.with(|state| {
        let mut state = state.borrow_mut();
        if state.enabled {
            if value.starts_with('{') && value.ends_with('}') {
                state.payloads.push(value);
            } else {
                std::eprintln!("{value}");
            }
        } else {
            std::println!("{value}");
        }
    });
}

pub fn prompt(value: String) {
    RESULT.with(|state| {
        if !state.borrow().enabled {
            std::print!("{value}");
        } else {
            std::eprint!("{value}");
        }
    });
}

pub fn detail(value: String) {
    RESULT.with(|state| {
        let mut state = state.borrow_mut();
        if state.enabled {
            state.payloads.push(value);
        }
    });
}

pub fn status(value: &'static str) {
    assert!(["done", "nothing_to_do", "refused", "incomplete", "error"].contains(&value));
    RESULT.with(|state| state.borrow_mut().status = Some(value));
}

pub fn effect(name: &'static str, count: usize) {
    RESULT.with(|state| {
        state.borrow_mut().effects.insert(name, count);
    });
}

pub fn finish(code: ExitCode) {
    RESULT.with(|state| {
        let state = state.borrow();
        if !state.enabled { return; }
        let details = match state.payloads.as_slice() { [] => "{}".into(), [value] => value.clone(), values => j::arr(values.iter().cloned()) };
        let status = state.status.unwrap_or(if code == ExitCode::from(1) { "nothing_to_do" } else if code == ExitCode::from(2) { "refused" } else if code != ExitCode::SUCCESS { "error" } else { "done" });
        let id = format!("{}-{}-{}", state.tool, std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos());
        let recovery = match state.operation.as_str() {
            "apply" | "stash" | "review" if !state.journal_enabled => "manual_restore_required",
            "apply" | "stash" | "review" => "conditional_journal_restore",
            "undo" | "pop" => "retry_remaining_journal_entries_after_resolving_refusals",
            "extract" => "source_archive_retained_no_undo",
            _ => "no_operation_undo_needed",
        };
        let claim = |name: &str, verdict: &str| j::obj(&[("claim", j::str(name)), ("status", j::str(verdict))]);
        std::println!("{}", j::obj(&[
            ("schema_version", j::num(SCHEMA_VERSION)),
            ("tool_version", j::str(&state.version)),
            ("operation_id", j::str(&id)),
            ("status", j::str(status)),
            ("scope", state.scope.clone()),
            ("observations", etude_core::scan::receipt_json()),
            ("effects", j::obj(&[
                ("outcome", j::str(status)),
                ("counters", j::arr(state.effects.iter().map(|(name, count)| {
                    j::obj(&[("name", j::str(name)), ("count", j::num(count))])
                }))),
                ("unproven", j::arr([j::str("effects outside reported operation accounting; zero counters do not prove no effects")])),
            ])),
            ("verification", j::arr([claim("command_completed", if code == ExitCode::SUCCESS || code == ExitCode::from(1) { "pass" } else { "fail" }), claim("complete_read_coverage", "unproven"), claim("subprocess_network_absence", "unproven")])),
            ("recovery", j::obj(&[("mode", j::str(recovery)), ("conditions", j::arr(if state.journal_enabled && matches!(state.operation.as_str(), "apply" | "stash" | "review" | "undo" | "pop") { vec![j::str("readable_journal_and_same_key"), j::str("unchanged_items_and_vacant_original_paths"), j::str("journal_not_expired_and_progress_writable")] } else { vec![] })), ("verification", j::str("unproven; consult operation details and filesystem before retrying"))])),
            ("disclosure", j::obj(&[("receipt_contains", j::str("read categories and outcomes only; no contents, keys or environment values")), ("zero_counters", j::str("not evidence of no access")), ("details", j::str("may contain operation paths; treat as sensitive"))])),
            ("details", details),
        ]));
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn schema_fields_require_a_version_bump() {
        assert_eq!(super::SCHEMA_VERSION, 2);
        assert_eq!(
            super::FIELDS,
            &[
                "schema_version",
                "tool_version",
                "operation_id",
                "status",
                "scope",
                "observations",
                "effects",
                "verification",
                "recovery",
                "disclosure",
                "details"
            ]
        );
    }
}
