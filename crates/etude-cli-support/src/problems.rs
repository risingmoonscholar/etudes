//! Plan, scan and journal failures in a person's words, shared by `sweep` and
//! `stash` so one condition gets one answer. Each says what failed and the next
//! step, and picks the code that fits the real cause.

use std::path::Path;
use std::process::ExitCode;

use etude_core::journal::JournalError;
use etude_core::plan::PlanFault;
use etude_core::scan::{self, RefusedLocation, ScanError};

use crate::reason::{Code, Reason};
use crate::{plain_io_error, quoted_path, shown, shown_text};

/// A failed request: what to print, the code, and the exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub text: String,
    pub code: Code,
    pub exit: u8,
}

impl Problem {
    pub fn refused(text: String, code: Code) -> Self {
        Self {
            text,
            code,
            exit: 2,
        }
    }

    pub fn failed(text: String, code: Code) -> Self {
        Self {
            text,
            code,
            exit: 3,
        }
    }

    /// Print it, set the reason, and return the exit.
    pub fn report(self) -> ExitCode {
        eprintln!("{}", self.text);
        crate::envelope::reason(Reason::of(self.code));
        ExitCode::from(self.exit)
    }
}

/// The disk has no room left.
pub fn disk_full(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::StorageFull
}

/// The disk is mounted read-only.
pub fn disk_read_only(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::ReadOnlyFilesystem
}

/// The file was opened without following links, and it is a link (ELOOP).
pub fn is_link_loop(error: &std::io::Error) -> bool {
    error.raw_os_error() == Some(if cfg!(target_os = "macos") { 62 } else { 40 })
}

/// How to make a new plan for this folder. The name is a placeholder, because
/// a plan is never written over an old file.
fn new_plan_step(tool: &str, root: &Path) -> String {
    format!("{tool} {} --export-plan NEW_PLAN_FILE", quoted_path(root))
}

/// A saved plan no longer matches its folder. Nothing was moved when this prints.
pub fn plan_stale_text(tool: &str, root: &Path) -> String {
    format!(
        "{tool}: {} changed since this plan was made, or this plan already ran. Nothing was moved.\n\
         Make a new plan: {}",
        shown(root),
        new_plan_step(tool, root)
    )
}

/// A plan that could not be checked against its folder.
pub fn plan_check_problem(tool: &str, root: &Path, error: &std::io::Error) -> Problem {
    let text = match PlanFault::of(error) {
        Some(PlanFault::ToolChanged) => format!(
            "{tool}: this plan was made by another version of {tool}, or under other settings. Nothing was moved.\n\
             Make a new plan: {}",
            new_plan_step(tool, root)
        ),
        _ => plan_stale_text(tool, root),
    };
    Problem::refused(text, Code::PlanStale)
}

/// The folder changed while a command that planned it just now was running.
pub fn folder_changed_just_now(tool: &str, root: &Path) -> String {
    let again = match tool {
        "sweep" => "Scan and apply again.",
        _ => "Run it again.",
    };
    format!(
        "{tool}: {} changed just now, perhaps by another run. Nothing was moved. {again}",
        shown(root)
    )
}

/// The plan named by `--plan` could not be loaded.
pub fn plan_load_problem(tool: &str, file: &Path, error: &std::io::Error) -> Problem {
    let name = shown(file);
    let export = format!("Export a new one with {tool} FOLDER --export-plan NEW_PLAN_FILE.");
    if let Some(fault) = PlanFault::of(error) {
        return match fault {
            PlanFault::DigestArgument => Problem::refused(
                format!(
                    "{tool}: --plan-digest needs the 64-character digest printed when you exported the plan."
                ),
                Code::InvalidArguments,
            ),
            PlanFault::NotAFile if file.is_dir() => Problem::refused(
                format!("{tool}: {name} is a folder. --plan needs the plan file."),
                Code::InvalidArguments,
            ),
            PlanFault::NotAFile => Problem::refused(
                format!("{tool}: {name} is not a regular file. --plan needs the plan file."),
                Code::InvalidArguments,
            ),
            PlanFault::NotAPlan => Problem::refused(
                format!("{tool}: {name} is not an exported plan. {export}"),
                Code::InvalidArguments,
            ),
            _ => Problem::refused(
                format!(
                    "{tool}: this plan file does not match that digest. It was changed, or the digest is from another export. Nothing was moved. {export}"
                ),
                Code::PlanStale,
            ),
        };
    }
    match error.kind() {
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory => Problem::failed(
            format!("{tool}: no plan file at {name}. Check the path. {export}"),
            Code::MissingInput,
        ),
        std::io::ErrorKind::PermissionDenied => Problem::failed(
            format!("{tool}: cannot read {name}: permission denied. Check it with `ls -l`."),
            Code::PermissionDenied,
        ),
        _ if is_link_loop(error) => Problem::refused(
            format!("{tool}: {name} is a symbolic link. --plan needs the plan file itself."),
            Code::InvalidArguments,
        ),
        // The digest matched and the contents still do not form a plan this
        // version can use: forged, or written by a format it does not read.
        std::io::ErrorKind::Other | std::io::ErrorKind::InvalidData => Problem::refused(
            format!(
                "{tool}: {name} is not a plan this version of {tool} can use. Nothing was moved. {export}"
            ),
            Code::PlanStale,
        ),
        _ => Problem::failed(
            format!("{tool}: cannot read {name}: {}.", plain_io_error(error)),
            Code::InternalError,
        ),
    }
}

/// A plan that belongs to another tool.
pub fn plan_of_another_tool(tool: &str, maker: &str) -> Problem {
    let maker = shown_text(maker);
    Problem::refused(
        format!(
            "{tool}: this plan was made by {maker}. Run it with {maker}, or export one with {tool}."
        ),
        Code::InvalidArguments,
    )
}

/// A flag that would change what a saved plan already fixed.
pub fn plan_flag_problem(tool: &str, flag: &str) -> Problem {
    Problem::refused(
        format!(
            "{tool}: {flag} cannot change a saved plan. Remove {flag}, or export a new plan with the settings you want."
        ),
        Code::InvalidArguments,
    )
}

/// A folder named beside a saved plan, which already names its own.
pub fn plan_path_problem(tool: &str) -> Problem {
    Problem::refused(
        format!(
            "{tool}: a saved plan already names its folder. Remove the folder from this command."
        ),
        Code::InvalidArguments,
    )
}

/// `--export-plan` failed.
pub fn plan_export_problem(
    tool: &str,
    file: &Path,
    root: &Path,
    error: &std::io::Error,
) -> Problem {
    let name = shown(file);
    let cannot = format!("{tool}: cannot write the plan to {name}");
    match PlanFault::of(error) {
        Some(PlanFault::ExportInsideTree) => {
            return Problem::refused(
                format!(
                    "{cannot}: that is inside the folder being planned. Choose a place outside it."
                ),
                Code::InvalidArguments,
            );
        }
        Some(PlanFault::ExportNoName) => {
            return Problem::refused(
                format!("{cannot}: that does not name a file. Give it a file name."),
                Code::InvalidArguments,
            );
        }
        Some(_) => return plan_check_problem(tool, root, error),
        None => {}
    }
    match error.kind() {
        std::io::ErrorKind::AlreadyExists => Problem::refused(
            format!("{cannot}: that file already exists. Choose a new name."),
            Code::DestinationExists,
        ),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory => Problem::failed(
            format!("{cannot}: no folder there. Check the path."),
            Code::MissingInput,
        ),
        std::io::ErrorKind::PermissionDenied => Problem::failed(
            format!("{cannot}: permission denied. Check that folder with `ls -ld`."),
            Code::PermissionDenied,
        ),
        _ if disk_full(error) => Problem::failed(
            format!("{cannot}: the disk is full. Free some space, or choose another disk."),
            Code::InsufficientSpace,
        ),
        _ if disk_read_only(error) => Problem::failed(
            format!("{cannot}: the disk is read-only. Choose another disk."),
            Code::PermissionDenied,
        ),
        _ => Problem::failed(
            format!("{cannot}: {}.", plain_io_error(error)),
            Code::InternalError,
        ),
    }
}

/// What a person can do about a move the system denied. EPERM on a move usually
/// means a locked file, so that is checked first. `paths` are the items the move
/// was working through, and their folders count too. `within` is where to look
/// for the locked item; file names stay out of the message, so the step finds them.
pub fn move_denied_advice(
    error: &std::io::Error,
    paths: &[std::path::PathBuf],
    within: &Path,
) -> Option<String> {
    if error.raw_os_error() != Some(1) {
        return scan::permission_advice(error).map(str::to_owned);
    }
    let mut locked: Vec<&Path> = paths
        .iter()
        .flat_map(|path| [Some(path.as_path()), path.parent()])
        .flatten()
        .filter(|path| scan::is_locked(path))
        .collect();
    locked.sort();
    locked.dedup();
    let Some(first) = locked.first() else {
        return Some(format!(
            "A file may be locked. {UNLOCK}\n       See which with: find {} -flags +uchg\n       If none is locked, macOS may be blocking the app running this command. Check System Settings > Privacy & Security > Files & Folders for that app.",
            crate::quoted_path(within),
        ));
    };
    let folder = if first.starts_with(within) {
        within
    } else if first.is_dir() {
        first
    } else {
        first.parent().unwrap_or(first)
    };
    let system = locked.iter().any(|path| scan::has_system_lock_flag(path));
    Some(locked_move_advice(locked.len(), folder, system))
}

fn locked_move_advice(count: usize, folder: &Path, system: bool) -> String {
    let items = crate::counted(count, "item is", "items are");
    if system {
        return format!(
            "{items} locked by a system flag, and a locked item cannot be moved. An administrator can clear it with sudo chflags noschg on the item.\n       See which with: find {} -flags +schg",
            crate::quoted_path(folder),
        );
    }
    format!(
        "{items} locked, and a locked item cannot be moved. {}",
        unlock_step(folder)
    )
}

pub const UNLOCK: &str =
    "In Finder, choose Get Info and clear Locked, or run chflags nouchg on the item.";
pub const UNLOCK_FOLDER: &str =
    "In Finder, choose Get Info and clear Locked, or run chflags nouchg on the folder.";

/// How to clear a lock. Scans count locked items without naming them, so the
/// step also finds them.
pub fn unlock_step(folder: &Path) -> String {
    format!(
        "{UNLOCK}\n       See which with: find {} -flags +uchg",
        crate::quoted_path(folder)
    )
}

/// One wording for items a scan held because of a lock, in every tool. Sweep
/// also holds a group whose existing folder is locked, so the item itself may
/// not carry the flag.
pub fn locked_note(count: usize) -> String {
    if count == 1 {
        "1 item is locked or headed for a locked folder, and stays in place".into()
    } else {
        format!("{count} items are locked or headed for a locked folder, and stay in place")
    }
}

/// A folder the plan needs is locked. Nothing can move into or out of it. The
/// line after this one says what became of the files.
pub fn locked_folder_message(tool: &str, folder: &Path) -> String {
    format!(
        "{tool}: {} is locked, so nothing can move into or out of it.\n       {UNLOCK_FOLDER} Then run this again.",
        crate::shown(folder)
    )
}

/// A folder named on the command line that cannot be opened. One condition, one
/// answer, on every command of both tools.
pub fn folder_open_problem(tool: &str, typed: &Path, error: &std::io::Error) -> Problem {
    let folder = shown(typed);
    match error.kind() {
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory => Problem::failed(
            format!("{tool}: no folder at {folder}."),
            Code::MissingInput,
        ),
        std::io::ErrorKind::PermissionDenied => {
            let advice = scan::permission_advice(error).unwrap_or("Check who may open it.");
            Problem::failed(
                format!("{tool}: cannot open {folder}: permission denied. {advice}"),
                Code::PermissionDenied,
            )
        }
        _ => Problem::failed(
            format!("{tool}: cannot open {folder}: {}.", plain_io_error(error)),
            Code::MissingInput,
        ),
    }
}

/// A scan failure, naming the folder the person gave. Files the tool found by
/// itself are never named here.
pub fn scan_problem(tool: &str, typed: &Path, error: &ScanError) -> String {
    let folder = shown(typed);
    match error {
        ScanError::NotADirectory(_) if typed.is_file() => {
            format!("{tool}: {folder} is a file. Give the folder that holds it.")
        }
        ScanError::NotADirectory(_) => format!("{tool}: {folder} is not a folder."),
        ScanError::Io(source)
            if matches!(
                source.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            format!("{tool}: no folder at {folder}.")
        }
        ScanError::Io(source) => {
            let mut text = format!("{tool}: cannot read {folder}: {}.", plain_io_error(source));
            if let Some(advice) = scan::permission_advice(source) {
                text.push('\n');
                text.push_str(advice);
            }
            text
        }
        ScanError::RefusedSystemLocation(found) => match scan::refused_location(found) {
            Some(RefusedLocation::SystemRoot(root)) if Path::new(root) == found => {
                format!(
                    "{tool}: refused: {root} is a system folder. These tools never organise it."
                )
            }
            Some(RefusedLocation::SystemRoot(root)) => format!(
                "{tool}: refused: {folder} is inside {root}, a system folder. These tools never organise it."
            ),
            Some(RefusedLocation::NamedFolder(name)) => {
                let name = shown_text(&name);
                let place = if found
                    .file_name()
                    .is_some_and(|last| last.to_string_lossy() == name)
                {
                    "is a folder named"
                } else {
                    "is inside a folder named"
                };
                format!(
                    "{tool}: refused: {folder} {place} {name}, which these tools never organise. Move the files elsewhere to tidy them."
                )
            }
            None => format!("{tool}: refused: these tools never organise {folder}."),
        },
        ScanError::RefusedSyncRoot(found) => format!(
            "{tool}: refused: {folder} looks cloud-synced (its path contains \"{}\"). If it is not, or you accept that, pass --allow-sync.",
            scan::sync_marker(found).unwrap_or("a sync folder's name")
        ),
        ScanError::RefusedInFlightRoot(_) => format!(
            "{tool}: refused: {folder} is a download still in progress. Run this again once it has finished arriving."
        ),
        ScanError::RefusedProjectRoot { root, marker } => {
            let itself = root
                .file_name()
                .is_some_and(|name| name.to_string_lossy() == marker.as_str());
            let because = if itself {
                "it is a project bundle".to_owned()
            } else {
                format!("{} is in it", shown_text(marker))
            };
            let next = match tool {
                "sweep" => "so sorting them into type folders would break it. Sweep the folder that contains it instead.".to_owned(),
                _ => format!(
                    "so moving them out would break it. Nothing was moved. To stash the whole folder in one piece, run: stash select {}",
                    quoted_path(typed)
                ),
            };
            format!(
                "{tool}: refused: {folder} looks like a project ({because}). Its files reference each other by relative path, {next}"
            )
        }
        ScanError::TooManyEntries { found, cap } => format!(
            "{tool}: refused: {folder} has {found} items, and {tool} handles {cap} at a time. Choose a smaller folder."
        ),
        ScanError::RootDenied { .. } | ScanError::RefusedRunningAsRoot => {
            format!("{tool}: {error}")
        }
    }
}

/// Why a saved undo record cannot be opened, for a person.
pub fn record_open_reason(error: &JournalError) -> String {
    match error {
        JournalError::Seal(_) => "wrong key, or it was altered".to_owned(),
        JournalError::Malformed(what) if what.contains("regular file") => {
            "it is not a regular file".to_owned()
        }
        JournalError::Malformed(_) => "it is damaged".to_owned(),
        JournalError::Io(source) => plain_io_error(source),
        JournalError::NotFound => "it is gone".to_owned(),
    }
}

/// The undo records folder itself cannot be used.
pub fn record_folder_problem(tool: &str, error: &JournalError) -> String {
    format!(
        "{tool}: cannot use the undo records folder {}: {}. Nothing was moved.\n\
         Set ETUDE_STATE_DIR to another folder, or fix this one.",
        shown(&etude_core::journal::state_dir()),
        match error {
            JournalError::Malformed(_) => "two records carry the same time".to_owned(),
            other => record_open_reason(other),
        }
    )
}

/// A saved undo record cannot be opened. Where it lives is a path the tool
/// chose, and the person needs it to act.
pub fn record_open_problem(tool: &str, id: &str, error: &JournalError) -> String {
    let path = etude_core::journal::state_dir().join(format!("{tool}-{id}.journal"));
    let fix = match error {
        JournalError::Seal(_) => "Use the key it was made with.",
        JournalError::Io(source) if source.kind() == std::io::ErrorKind::PermissionDenied => {
            "Check that file with `ls -l`."
        }
        _ => "It cannot be repaired.",
    };
    format!(
        "the undo record {} cannot be opened ({}). {fix}",
        shown(&path),
        record_open_reason(error)
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_denied_move_names_a_locked_file_first_and_the_privacy_setting_second() {
        let eperm = std::io::Error::from_raw_os_error(1);
        let advice = move_denied_advice(&eperm, &[], Path::new("/nowhere")).unwrap();
        let locked = advice.find("may be locked").unwrap();
        let privacy = advice.find("Privacy & Security").unwrap();
        assert!(locked < privacy, "{advice}");
        assert!(
            advice.contains("find '/nowhere' -flags +uchg"),
            "no locked item in the plan still gets the step that finds it: {advice}"
        );
        let quoted = move_denied_advice(&eperm, &[], Path::new("/no where/it's")).unwrap();
        assert!(
            quoted.contains("find '/no where/it'\\''s' -flags +uchg"),
            "{quoted}"
        );
        let eacces = std::io::Error::from_raw_os_error(13);
        let modes = move_denied_advice(&eacces, &[], Path::new("/nowhere")).unwrap();
        assert!(modes.contains("ls -ld"), "{modes}");
        assert!(!modes.contains("locked"), "{modes}");
        let enoent = std::io::Error::from_raw_os_error(2);
        assert!(move_denied_advice(&enoent, &[], Path::new("/nowhere")).is_none());
        let plain = locked_move_advice(2, Path::new("/nowhere"), false);
        assert!(plain.starts_with("2 items are locked"), "{plain}");
        assert!(plain.contains("chflags nouchg"), "{plain}");
        assert!(plain.contains("-flags +uchg"), "{plain}");
        // Finder and nouchg cannot clear the system flag, and +uchg cannot find it.
        let system = locked_move_advice(1, Path::new("/nowhere"), true);
        assert!(system.contains("sudo chflags noschg"), "{system}");
        assert!(system.contains("-flags +schg"), "{system}");
        assert!(!system.contains("nouchg"), "{system}");
        assert!(!system.contains("+uchg"), "{system}");
    }

    use super::*;
    use std::path::PathBuf;

    #[test]
    fn a_missing_folder_has_one_answer_and_a_denied_one_does_not_share_it() {
        let folder = Path::new("/o/gone");
        for errno in [2, 20] {
            let problem =
                folder_open_problem("sweep", folder, &std::io::Error::from_raw_os_error(errno));
            assert_eq!(problem.text, "sweep: no folder at /o/gone.");
            assert_eq!((problem.code, problem.exit), (Code::MissingInput, 3));
        }
        let denied = folder_open_problem("stash", folder, &std::io::Error::from_raw_os_error(13));
        assert!(denied.text.contains("permission denied"), "{}", denied.text);
        assert_eq!((denied.code, denied.exit), (Code::PermissionDenied, 3));
    }

    #[test]
    fn a_full_disk_and_a_read_only_disk_get_their_own_code_when_a_plan_cannot_be_saved() {
        let (file, root) = (Path::new("/o/plan"), Path::new("/o/tree"));
        let full = plan_export_problem("sweep", file, root, &std::io::Error::from_raw_os_error(28));
        assert_eq!((full.code, full.exit), (Code::InsufficientSpace, 3));
        assert!(full.text.contains("the disk is full"), "{}", full.text);
        let locked =
            plan_export_problem("stash", file, root, &std::io::Error::from_raw_os_error(30));
        assert_eq!((locked.code, locked.exit), (Code::PermissionDenied, 3));
        assert!(locked.text.contains("read-only"), "{}", locked.text);
    }

    #[test]
    fn the_rule_that_refused_a_folder_is_the_rule_the_words_state() {
        let line = |path: &str| {
            let found = PathBuf::from(path);
            scan_problem(
                "sweep",
                &found,
                &ScanError::RefusedSystemLocation(found.clone()),
            )
        };
        assert!(line("/System").contains("/System is a system folder"));
        assert!(line("/System/x").contains("/System/x is inside /System"));
        assert!(line("/a/Library/b").contains("is inside a folder named Library"));
        assert!(line("/a/Library").contains("is a folder named Library"));
        assert!(line("/a/node_modules/b").contains("a folder named node_modules"));
    }

    #[test]
    fn control_characters_in_a_typed_name_never_reach_the_terminal() {
        let found = PathBuf::from("/a/x\x1b[31m");
        let text = scan_problem("sweep", &found, &ScanError::RefusedSyncRoot(found.clone()));
        assert!(!text.contains('\x1b'), "{text}");
    }

    #[test]
    fn nothing_the_person_reads_uses_a_bare_os_error() {
        let file = Path::new("/o/plan");
        for code in [1, 2, 5, 13, 20, 21, 28, 30, 62, 63] {
            let error = std::io::Error::from_raw_os_error(code);
            let load = plan_load_problem("sweep", file, &error);
            let export = plan_export_problem("sweep", file, file, &error);
            for text in [load.text, export.text] {
                assert!(!text.contains("os error"), "{text}");
            }
        }
    }
}
