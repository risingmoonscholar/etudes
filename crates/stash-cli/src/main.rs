//! stash: clean now, decide later.
//!
//! Moves everything in a folder into one hidden holding directory, and brings
//! it all back on demand. It makes no organisational decisions, which is the
//! whole point: before a screen share or a demo you want the folder empty, not
//! sorted.
//!
//! # Why stash moves files sweep refuses
//!
//! `sweep` never moves a file that looks like a personal record, because sweep
//! chooses a *permanent destination* from a guess. stash chooses nothing. It
//! moves everything to one place, keeps a full reversal record, and brings it
//! back. Leaving the tax scan on the Desktop during a screen share would defeat
//! the only thing the user asked for.
//!
//! The rule that makes this safe is therefore different from sweep's: stash is
//! all-or-nothing and fully reversible, and it says out loud what it took.
//!
//! # Where the deadline lives
//!
//! In the holding directory's own name: `.stash-<restore-by-epoch>`. No sidecar
//! file, no second state store, nothing to fall out of sync. The deadline is
//! derived from the filesystem rather than recorded next to it.

mod batches;
mod selection;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use etude_core::plan::{Group, Plan, Signal};
use etude_core::scan::{self, ScanConfig};

const USAGE: &str = "\
stash: clean now, decide later

USAGE
    stash [PATH] [--for DURATION]   move everything into a hidden holding folder
    stash select PATH...           stash exactly the chosen items; folders move whole
    stash select --from0 FILE|-     read NUL-terminated UTF-8 paths; '-' reads stdin
    --into FOLDER                   where stash select keeps what it holds
    stash pop --id ID               restore exactly one operation
    stash pop --id ID --item ITEM   restore one item from an operation
    stash pop ID                    restore an operation id directly
    stash pop [PATH]                restore the unique batch for PATH (or here)
    stash pop [PATH] --latest       restore the latest live batch; no PATH means all
    stash pop [PATH] --if-due       pop only if the deadline has passed;
                                    exit 2 when early, 1 when nothing stashed,
                                    3 when there is no folder at PATH
    stash status [PATH]             what is stashed, and when it is due back
    stash inspect --id ID           inspect item ids and what remains held
    stash status --all              every stash this machine's journals know,
                                    paths redacted; --paths shows them
    stash contract --json           print the versioned capability contract
    --no-journal                    folder mode only; removes undo. stash select needs undo
    --allow-home                    stash your whole home folder; refused without it
    --json                          machine-readable output (for agents)
    --export-plan FILE               save a private bound plan, without moving
    --plan FILE --plan-digest SHA     apply exactly what was exported
    --version                       print the version and exit
    stash help

DURATION
    30m  2h  3d  1w        default: no deadline, restore whenever

KEY
    ETUDE_JOURNAL_KEY  64 hex digits (32 random bytes), overrides the keychain.
                       Retain the same key for undo; see README for setup.

stash moves everything sweep can see, including Finder-tagged items and the
files sweep would refuse to organise. Hidden items are left in place, and it
says how many. A tagged item that would have to cross volumes is the one thing
it leaves, because the copy would drop the tag; it then says the folder is NOT
clear.
That is deliberate: clearing a folder for a screen share means clearing it.
With a journal, everything is reversible. stash prints what it took.";

macro_rules! println {
    () => { etude_cli_support::envelope::print(String::new()) };
    ($($arg:tt)*) => { etude_cli_support::envelope::print(format!($($arg)*)) };
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let receipt_args = if args.first().is_some_and(|arg| arg == "select") {
        selection::receipt_arguments(&args)
    } else {
        args.clone()
    };
    etude_cli_support::envelope::begin("stash", env!("CARGO_PKG_VERSION"), &receipt_args);
    let code = std::panic::catch_unwind(run_main).unwrap_or_else(|_| {
        refusal_reason(etude_cli_support::reason::Code::InternalError);
        etude_cli_support::envelope::status("error");
        ExitCode::from(3)
    });
    etude_cli_support::envelope::finish(code);
    code
}

fn run_main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.first().is_some_and(|arg| arg == "contract") {
        return etude_cli_support::contract::command(
            etude_cli_support::contract::Tool::Stash,
            env!("CARGO_PKG_VERSION"),
            &args,
            &[],
            None,
        );
    }

    if etude_cli_support::wants_help(&args) {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }

    // The operation lookup must not run the legacy migration used by normal
    // commands: it is a strictly read-only query against the named journal.
    if args
        .first()
        .is_some_and(|arg| matches!(arg.as_str(), "status" | "list"))
        && flag(&args, "--id")
    {
        return match check_flags("status", &args) {
            Ok(()) => cmd_status_id(&args),
            Err(message) => {
                eprintln!("stash: {message}");
                refuse(if message.contains("unknown option") {
                    etude_cli_support::reason::Code::UnknownOption
                } else {
                    etude_cli_support::reason::Code::InvalidArguments
                })
            }
        };
    }

    // One-time move from the old XDG-style state directory to the correct
    // macOS one (issue #23), before anything reads state_dir(). Journals in
    // the shared state directory are namespaced by tool prefix within one
    // directory, not split per tool, so stash's own history needs this same
    // migration whether or not sweep has ever run on this machine.
    etude_core::journal::migrate_legacy_state_dir();

    match args.first().map(String::as_str) {
        Some("help" | "--help" | "-h") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some("version" | "--version" | "-V") => {
            println!("stash {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("select") => cmd_select(&args),
        Some("pop" | "restore") => match check_flags("pop", &args) {
            Ok(()) => cmd_pop(&args),
            Err(m) => {
                eprintln!("stash: {m}");
                refuse(if m.contains("unknown option") {
                    etude_cli_support::reason::Code::UnknownOption
                } else {
                    etude_cli_support::reason::Code::InvalidArguments
                })
            }
        },
        Some("inspect") => match check_flags("inspect", &args) {
            Ok(()) => cmd_inspect(&args),
            Err(m) => {
                eprintln!("stash: {m}");
                refuse(if m.contains("unknown option") {
                    etude_cli_support::reason::Code::UnknownOption
                } else {
                    etude_cli_support::reason::Code::InvalidArguments
                })
            }
        },
        Some("status" | "list") => match check_flags("status", &args) {
            Ok(()) => cmd_status(&args),
            Err(m) => {
                eprintln!("stash: {m}");
                refuse(if m.contains("unknown option") {
                    etude_cli_support::reason::Code::UnknownOption
                } else {
                    etude_cli_support::reason::Code::InvalidArguments
                })
            }
        },
        None => cmd_stash(&std::env::current_dir().unwrap_or_default(), &args),
        // A leading flag means "stash the current directory", which is the most
        // destructive thing this tool does. An unrecognised one must therefore
        // be refused, not treated as consent: `stash --dry-run` used to empty
        // the folder the user was standing in.
        Some(p) if p.starts_with('-') => {
            if STASH_FLAGS.contains(&p) {
                let path = positional_path(&args)
                    .map(|s| PathBuf::from(expand_tilde(s)))
                    .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
                stash_checked(&path, &args)
            } else {
                eprintln!("stash: unknown option {p}. Run `stash help`.");
                refuse(etude_cli_support::reason::Code::UnknownOption)
            }
        }
        Some(p) => stash_checked(&PathBuf::from(expand_tilde(p)), &args),
    }
}

/// Every route that ends in moving a folder passes here. Checking only the
/// leading flag let `stash ~/Desktop --dry-run` empty the folder it was asked
/// to leave alone.
fn stash_checked(path: &Path, args: &[String]) -> ExitCode {
    match check_flags("", args) {
        Ok(()) => cmd_stash(path, args),
        Err(message) => {
            eprintln!("stash: {message}");
            refuse(if message.contains("unknown option") {
                etude_cli_support::reason::Code::UnknownOption
            } else {
                etude_cli_support::reason::Code::InvalidArguments
            })
        }
    }
}

/// Flags that may lead the argument list. Anything else there is a typo, and a
/// typo must not stash the current directory.
const STASH_FLAGS: &[&str] = &[
    "--for",
    "--json",
    "--no-journal",
    "--allow-home",
    "--export-plan",
    "--plan",
    "--plan-digest",
];

/// Every command, and every flag reachable from it. Same construction as
/// sweep's COMMAND_FLAGS and for the same reason: the leading-flag guard
/// above protected exactly one position, so `stash pop --ifdue` -- a typo of
/// the flag that makes the deadline binding -- sailed through and popped
/// early. A tool cannot offer automation a contract while accepting any
/// misspelling of it.
const COMMAND_FLAGS: &[(&str, &[&str])] = &[
    (
        "",
        &[
            "--for",
            "--json",
            "--no-journal",
            "--allow-home",
            "--export-plan",
            "--plan",
            "--plan-digest",
        ],
    ),
    ("pop", &["--id", "--item", "--latest", "--if-due", "--json"]),
    ("inspect", &["--id", "--json"]),
    ("status", &["--json", "--all", "--paths", "--id"]),
];

fn refusal_reason(code: etude_cli_support::reason::Code) {
    etude_cli_support::envelope::reason(etude_cli_support::reason::Reason::of(code));
}

fn refuse(code: etude_cli_support::reason::Code) -> ExitCode {
    refusal_reason(code);
    etude_cli_support::envelope::status("refused");
    ExitCode::from(2)
}

fn refuse_preserving_status(code: etude_cli_support::reason::Code) -> ExitCode {
    refusal_reason(code);
    ExitCode::from(2)
}

fn fail(code: etude_cli_support::reason::Code) -> ExitCode {
    refusal_reason(code);
    ExitCode::from(3)
}

fn scan_reason(error: &scan::ScanError) -> etude_cli_support::reason::Code {
    use etude_cli_support::reason::Code;
    match error {
        scan::ScanError::RefusedProjectRoot { .. } => Code::ProjectFolder,
        scan::ScanError::RefusedInFlightRoot(_) => Code::DownloadInFlight,
        scan::ScanError::NotADirectory(_) => Code::MissingInput,
        scan::ScanError::RootDenied { .. } => Code::PermissionDenied,
        scan::ScanError::Io(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            Code::PermissionDenied
        }
        // "Not a directory" is a path that runs through a regular file: it
        // names nothing, exactly as a missing folder does.
        scan::ScanError::Io(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            Code::MissingInput
        }
        scan::ScanError::Io(_) => Code::InternalError,
        _ if error.is_refusal() => Code::UnsafeInput,
        _ => Code::InternalError,
    }
}

fn apply_reason(error: &etude_core::apply::ApplyError) -> etude_cli_support::reason::Code {
    use etude_cli_support::reason::Code;
    use etude_core::apply::ApplyError::*;
    match error {
        StalePlan(error)
            if error
                .to_string()
                .contains("destination must stay inside the plan root") =>
        {
            Code::DestinationOutsideRoot
        }
        StalePlan(_) | DestinationDirectoryChanged(_) => Code::PlanStale,
        DestinationExists(_) | DestinationCollision(_) => Code::DestinationExists,
        DestinationIsSynced(_) | CannotCompareNames(_) => Code::UnsafeInput,
        DestinationBlocked(_) => Code::DestinationExists,
        Io(error) if etude_cli_support::problems::disk_full(error) => Code::InsufficientSpace,
        Io(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Code::DestinationExists,
        Io(error)
            if error.kind() == std::io::ErrorKind::PermissionDenied
                || etude_cli_support::problems::disk_read_only(error) =>
        {
            Code::PermissionDenied
        }
        Io(error) if error.kind() == std::io::ErrorKind::NotFound => Code::MissingInput,
        Io(_) => Code::InternalError,
        Journal(_) => Code::JournalWriteFailed,
        Injected(_) => Code::OperationIncomplete,
    }
}

/// A folder pop that matches several batches is a choice for the caller, not a
/// damaged journal. Nothing moved, so it is a refusal that names the choices.
fn refuse_ambiguous_pop(
    selector_args: &[String],
    sealer: &dyn etude_core::journal::Sealer,
) -> ExitCode {
    let root = match batches::selector(selector_args) {
        Ok(batches::Selector::Root(root)) => root,
        _ => std::env::current_dir().unwrap_or_default(),
    };
    let ids = batches::live_ids_for(&root, sealer).unwrap_or_default();
    eprintln!(
        "stash: {} batches are stashed from this folder, and nothing was restored. Choose one:",
        ids.len()
    );
    for id in &ids {
        eprintln!("  stash pop --id {id}");
    }
    let folder = root
        .to_str()
        .and_then(etude_cli_support::shell_word)
        .unwrap_or_else(|| "PATH".to_owned());
    eprintln!("Or restore the newest: stash pop {folder} --latest");
    refuse(etude_cli_support::reason::Code::InvalidArguments)
}

fn journal_reason(error: &etude_core::journal::JournalError) -> etude_cli_support::reason::Code {
    use etude_cli_support::reason::Code;
    use etude_core::journal::JournalError;
    match error {
        JournalError::Io(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            Code::PermissionDenied
        }
        _ => Code::JournalUnreadable,
    }
}

fn find_stash_id_for_holding(
    root: &Path,
    holding: &str,
    sealer: &dyn etude_core::journal::Sealer,
) -> Option<String> {
    let expected = root.join(holding);
    etude_core::journal::candidates_unordered("stash")
        .ok()?
        .into_iter()
        .filter_map(|candidate| candidate.load("stash", sealer).ok())
        .find(|journal| {
            journal
                .entries
                .iter()
                .any(|entry| entry.to.starts_with(&expected))
        })
        .map(|journal| journal.id)
}

/// What to say when a flag that takes a value did not get one. A negative number
/// is a bad value, not another option.
fn missing_value_text(flag: &str, next: Option<&str>) -> String {
    let try_these = if flag == "--for" {
        " Try 30m, 2h, 3d or 1w."
    } else {
        ""
    };
    match next.map(etude_cli_support::shown_text) {
        Some(value) if flag == "--for" && value[1..].starts_with(|c: char| c.is_ascii_digit()) => {
            format!("--for must be a length of time, got `{value}`.{try_these}")
        }
        Some(value) => format!("{flag} needs a value, and `{value}` is another option.{try_these}"),
        None => format!("{flag} needs a value.{try_these}"),
    }
}

/// Refuse any flag the command does not read. `--for` takes a value, which is
/// skipped, not judged.
fn check_flags(cmd: &str, args: &[String]) -> Result<(), String> {
    let allowed = COMMAND_FLAGS
        .iter()
        .find(|(c, _)| *c == cmd)
        .map(|(_, f)| *f)
        .unwrap_or(&[]);
    let mut i = 1; // skip the subcommand itself (or index 0 for bare)
    if cmd.is_empty() {
        i = 0;
    }
    let mut folders = 0;
    let mut seen: Vec<&str> = Vec::new();
    while i < args.len() {
        let a = &args[i];
        // --item is the one flag that accumulates. A repeat of any other would
        // silently drop a value the caller believes is in force.
        if a.starts_with('-') && allowed.contains(&a.as_str()) && a != "--item" {
            if seen.contains(&a.as_str()) {
                return Err(format!(
                    "{a} was given more than once, and only the first would have been used. \
                     Remove the ones you do not mean."
                ));
            }
            seen.push(a);
        }
        if matches!(
            a.as_str(),
            "--for" | "--export-plan" | "--plan" | "--plan-digest" | "--id" | "--item"
        ) && allowed.contains(&a.as_str())
        {
            if args.get(i + 1).is_none_or(|value| value.starts_with('-')) {
                return Err(missing_value_text(a, args.get(i + 1).map(String::as_str)));
            }
            i += 2;
            continue;
        }
        if a.starts_with('-') && !allowed.contains(&a.as_str()) {
            return Err(format!(
                "unknown option {a} for this command. Run `stash help`."
            ));
        }
        if cmd.is_empty() && !a.starts_with('-') {
            folders += 1;
            if folders > 1 {
                return Err(format!(
                    "stash takes one folder, and `{a}` is a second. \
                     To stash chosen items, use `stash select PATH...`."
                ));
            }
        }
        if cmd == "status" && !a.starts_with('-') {
            folders += 1;
            if folders > 1 {
                return Err(format!(
                    "status takes one folder, and `{}` is a second.",
                    etude_cli_support::shown_text(a)
                ));
            }
        }
        i += 1;
    }
    if cmd == "status" && seen.contains(&"--paths") && !seen.contains(&"--all") {
        return Err("--paths needs --all.".to_owned());
    }
    Ok(())
}

fn expand_tilde(p: &str) -> String {
    if p == "~" {
        return std::env::var("HOME").unwrap_or_else(|_| p.into());
    }
    match p.strip_prefix("~/") {
        Some(rest) => std::env::var("HOME")
            .map(|h| format!("{h}/{rest}"))
            .unwrap_or(p.into()),
        None => p.to_string(),
    }
}

fn flag(args: &[String], f: &str) -> bool {
    args.iter().any(|a| a == f)
}

fn value(args: &[String], flag: &str) -> Option<String> {
    let i = args.iter().position(|a| a == flag)?;
    args.get(i + 1).cloned()
}

/// The positional PATH in `args`, skipping known leading/interspersed flags
/// and their values. `--for`'s value is the token right after it; `--json`
/// takes no value.
fn positional_path(args: &[String]) -> Option<&str> {
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--for" | "--export-plan" | "--plan" | "--plan-digest" => i += 2,
            "--json" => i += 1,
            a if a.starts_with('-') => i += 1,
            a => return Some(a),
        }
    }
    None
}

/// `30m`, `2h`, `3d`, `1w` → seconds.
pub fn parse_duration(s: &str) -> Option<u64> {
    let s = s.trim();
    let (num, unit) = s.split_at(s.find(|c: char| c.is_alphabetic())?);
    let n: u64 = num.parse().ok()?;
    let mult = match unit {
        "m" | "min" => 60,
        "h" | "hr" => 3600,
        "d" | "day" => 86_400,
        "w" | "week" => 604_800,
        _ => return None,
    };
    n.checked_mul(mult)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Holding directory name. The deadline is *in the name*, so there is no second
/// state store to drift.
fn holding_name(deadline: Option<u64>) -> String {
    match deadline {
        Some(t) => format!(".stash-{t}"),
        None => ".stash-0".to_string(),
    }
}

/// Read the deadline back out of a holding directory name.
pub fn deadline_of(name: &str) -> Option<u64> {
    let t: u64 = name
        .strip_prefix(".stash-")?
        .split('-')
        .next()?
        .parse()
        .ok()?;
    (t > 0).then_some(t)
}

fn find_holding(root: &Path) -> Option<PathBuf> {
    etude_core::scan::observe_read("holding_directory_enumeration", std::fs::read_dir(root))
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.is_dir()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(".stash-"))
                && !selection::is_selection_root(p)
        })
}

fn observation_contract_digest() -> String {
    let mut declaration = etude_cli_support::contract::declaration(
        etude_cli_support::contract::Tool::Stash,
        env!("CARGO_PKG_VERSION"),
        &[],
        None,
    );
    let start = declaration
        .find("\"operation_id\":")
        .expect("contract identifier")
        + "\"operation_id\":".len();
    let end = start
        + 1
        + declaration[start + 1..]
            .find('"')
            .expect("contract identifier end")
        + 1;
    declaration.replace_range(start..end, "\"<per-invocation>\"");
    etude_core::plan::binding_digest(declaration.as_bytes())
}

fn current_binding_context(profile: &str) -> etude_core::plan::BindingContext {
    etude_core::plan::BindingContext::new(
        "stash",
        env!("CARGO_PKG_VERSION"),
        profile,
        &observation_contract_digest(),
    )
}

/// Whether `path` is the home folder, however it was written. A bare `stash`
/// typed in the wrong place would otherwise move everything a person keeps there.
fn is_home_folder(path: &Path) -> bool {
    let Some(home) = std::env::var_os("HOME").filter(|home| !home.is_empty()) else {
        return false;
    };
    matches!(
        (path.canonicalize(), Path::new(&home).canonicalize()),
        (Ok(path), Ok(home)) if path == home
    )
}

fn refuse_home_folder(path: &Path) -> ExitCode {
    eprintln!(
        "stash: refused: {} is your home folder, and stashing it would move everything you keep there. Nothing was moved.\n\
         To stash chosen items, use `stash select PATH...`. To stash the whole home folder, pass --allow-home.",
        etude_cli_support::shown(path)
    );
    refuse(etude_cli_support::reason::Code::UnsafeInput)
}

fn replay_stash(args: &[String], file: &str) -> ExitCode {
    use etude_cli_support::problems;
    if positional_path(args).is_some() {
        etude_cli_support::envelope::status("refused");
        return problems::plan_path_problem("stash").report();
    }
    if let Some(name) = ["--for", "--export-plan"]
        .into_iter()
        .find(|name| flag(args, name))
    {
        etude_cli_support::envelope::status("refused");
        return problems::plan_flag_problem("stash", name).report();
    }
    let Some(digest) = value(args, "--plan-digest") else {
        eprintln!("stash: --plan requires the digest printed with the exported plan");
        return refuse(etude_cli_support::reason::Code::InvalidArguments);
    };
    let bound = match etude_core::plan::BoundPlan::load(Path::new(file), &digest) {
        Ok(plan) => plan,
        Err(error) => {
            return problems::plan_load_problem("stash", Path::new(file), &error).report();
        }
    };
    let maker = bound.context().tool.clone();
    if maker != "stash" {
        return problems::plan_of_another_tool("stash", &maker).report();
    }
    if !bound.context().profile.starts_with("stash-metadata-v1;")
        || bound.groups.len() != 1
        || !bound.groups[0].name.starts_with(".stash-")
    {
        eprintln!("{}", problems::plan_stale_text("stash", &bound.root));
        return refuse(etude_cli_support::reason::Code::PlanStale);
    }
    if is_home_folder(&bound.root) && !flag(args, "--allow-home") {
        return refuse_home_folder(&bound.root);
    }
    let context = current_binding_context(&bound.context().profile);
    execute_stash(bound, context, args)
}

fn cmd_stash(path: &Path, args: &[String]) -> ExitCode {
    if let Some(file) = value(args, "--plan") {
        return replay_stash(args, &file);
    }
    if flag(args, "--plan-digest") {
        eprintln!("stash: --plan-digest requires --plan FILE");
        return refuse(etude_cli_support::reason::Code::InvalidArguments);
    }
    if is_home_folder(path) && !flag(args, "--allow-home") {
        return refuse_home_folder(path);
    }
    let deadline = match value(args, "--for") {
        Some(d) => match parse_duration(&d) {
            Some(secs) => Some(now_secs() + secs),
            None => {
                eprintln!("stash: cannot read duration {d:?}. Try 30m, 2h, 3d or 1w.");
                return refuse(etude_cli_support::reason::Code::InvalidArguments);
            }
        },
        None => None,
    };

    // Depth 1: stash clears the folder, it does not restructure a tree.
    let cfg = ScanConfig {
        depth: 1,
        allow_sync: true,
        // Clearing a folder means clearing it: directories and symlinks move
        // too, as whole units.
        whole_units: true,
        // sweep holds tagged items by default; stash exists to clear the
        // folder, so it asks for them and says so here rather than inheriting
        // the default.
        include_tagged: true,
        ..Default::default()
    };
    let outcome = match scan::scan(path, &cfg) {
        Ok(o) => o,
        Err(e) => {
            eprintln!(
                "{}",
                etude_cli_support::problems::scan_problem("stash", path, &e)
            );
            return if e.is_refusal() {
                refuse(scan_reason(&e))
            } else {
                fail(scan_reason(&e))
            };
        }
    };
    if outcome.entries.is_empty() && outcome.skipped_unreadable > 0 {
        // Nothing could be moved, and what could not be read is still there.
        eprintln!(
            "stash: {}. Nothing was moved. The folder is NOT clear.",
            unreadable_note(outcome.skipped_unreadable)
        );
        etude_cli_support::envelope::status("incomplete");
        return fail(etude_cli_support::reason::Code::PermissionDenied);
    }
    if outcome.entries.is_empty() {
        let hidden = outcome.skipped_hidden;
        etude_cli_support::envelope::detail(etude_core::json::obj(&[(
            "skipped_hidden",
            etude_core::json::num(hidden),
        )]));
        if hidden > 0 {
            println!(
                "Nothing to stash. {} still in {}.",
                etude_cli_support::counted(hidden, "hidden item is", "hidden items are"),
                etude_cli_support::shown(path)
            );
        } else {
            println!(
                "Nothing to stash. {} is already clear.",
                etude_cli_support::shown(path)
            );
        }
        return ExitCode::from(1);
    }

    let holding = format!(
        "{}-{}",
        holding_name(deadline),
        etude_cli_support::envelope::operation_id()
    );

    // One group, everything in it. No detectors, no decisions.
    let members: Vec<PathBuf> = outcome.entries.iter().map(|e| e.path.clone()).collect();
    let count = members.len();
    let proposal = Plan::with_groups(
        &outcome,
        vec![Group {
            name: holding.clone(),
            signal: Signal::Collected { count },
            members,
            accepted: true,
        }],
    );
    let profile = format!(
        "stash-metadata-v1;{}",
        etude_core::plan::binding_digest(
            format!(
                "whole_units=true;depth=1;deadline={deadline:?};grace={:?}",
                cfg.grace
            )
            .as_bytes()
        )
    );
    let context = current_binding_context(&profile);
    let bound = match etude_core::plan::BoundPlan::from_plan(proposal, context.clone()) {
        Ok(plan) => plan,
        Err(error) => {
            // The one bind failure that is not a changed folder says so itself.
            let text = error.to_string();
            if text.ends_with("run the scan again") {
                eprintln!("stash: {text}");
            } else {
                eprintln!(
                    "{}",
                    etude_cli_support::problems::folder_changed_just_now("stash", path)
                );
            }
            return refuse(etude_cli_support::reason::Code::PlanStale);
        }
    };
    if let Some(file) = value(args, "--export-plan") {
        if let Err(error) = bound.export(Path::new(&file)) {
            return etude_cli_support::problems::plan_export_problem(
                "stash",
                Path::new(&file),
                &bound.root,
                &error,
            )
            .report();
        }
        if flag(args, "--json") {
            let mut detail = bound.plan.to_json();
            detail.pop();
            detail.push_str(&format!(",\"binding\":{}}}", bound.binding_json()));
            println!("{detail}");
        } else {
            println!(
                "Plan exported to {file}; contains selected paths and private metadata. Digest: {}",
                bound.digest()
            );
        }
        return ExitCode::SUCCESS;
    }
    execute_stash(bound, context, args)
}

fn execute_stash(
    bound: etude_core::plan::BoundPlan,
    context: etude_core::plan::BindingContext,
    args: &[String],
) -> ExitCode {
    if let Err(error) = bound.validate(&context) {
        return etude_cli_support::problems::plan_check_problem("stash", &bound.root, &error)
            .report();
    }
    let count = bound.moves();
    let deadline = bound
        .groups
        .first()
        .and_then(|group| deadline_of(&group.name));

    let json = flag(args, "--json");
    let sl = if flag(args, "--no-journal") {
        eprintln!("stash: --no-journal removes undo. stash pop cannot restore this operation.");
        None
    } else {
        let Some(sl) = sealer(true) else {
            return refuse(etude_cli_support::reason::Code::JournalKeyUnavailable);
        };
        Some(sl)
    };
    let mut progress = etude_cli_support::ProgressReporter::stderr("stash", count);
    let result = etude_core::apply::apply_with_progress_outcome(
        &bound,
        &context,
        sl.as_ref().map(|s| s as &dyn etude_core::journal::Sealer),
        None,
        |p| progress.update(p.completed, p.total),
    );
    drop(progress);
    match result {
        Ok(r) => {
            if sl.is_some() {
                etude_cli_support::envelope::recovery_operation(&r.journal_id);
            }
            etude_cli_support::envelope::effect("items_moved", r.moved);
            let clear = folder_is_clear(&r) && bound.skipped_unreadable == 0;
            if json {
                use etude_core::json as j;
                println!(
                    "{}",
                    j::obj(&[
                        ("action", j::str("stash")),
                        (
                            "stash_id",
                            if sl.is_some() {
                                j::str(&r.journal_id)
                            } else {
                                "null".into()
                            }
                        ),
                        ("root", j::str("<redacted>")),
                        ("moved", j::num(r.moved)),
                        ("clear", j::bool(clear)),
                        ("held_cross_volume", j::num(r.held_cross_volume)),
                        ("held_tagged", j::num(r.held_tagged)),
                        ("tagged_not_returned", j::num(r.tagged_not_returned)),
                        ("holding", j::str(&bound.groups[0].name)),
                        ("binding", bound.binding_json()),
                        ("due", deadline.map(j::num).unwrap_or_else(|| "null".into())),
                        ("skipped_hidden", j::num(bound.skipped_hidden)),
                        ("skipped_unreadable", j::num(bound.skipped_unreadable)),
                    ])
                );
                print_stash_warnings(&bound);
                if !clear {
                    etude_cli_support::envelope::status("incomplete");
                    return fail(etude_cli_support::reason::Code::OperationIncomplete);
                }
                return ExitCode::SUCCESS;
            }
            println!(
                "\nStashed {}.",
                etude_cli_support::counted(r.moved, "item", "items")
            );
            if clear {
                println!("The selected folder is clear.\n");
            } else {
                print_stash_holds(&r, true);
                if bound.skipped_unreadable > 0 {
                    println!(
                        "{}. The selected folder is NOT clear.\n",
                        unreadable_note(bound.skipped_unreadable)
                    );
                }
            }
            if sl.is_some() && r.moved > 0 {
                println!(
                    "  Operation id: {}. Restore with: stash pop --id {}",
                    r.journal_id, r.journal_id
                );
                match deadline {
                    Some(t) => {
                        println!("  Due back: {}", human_time(t));
                        println!(
                            "  stash does not run in the background. Run `stash pop --id {}`,",
                            r.journal_id
                        );
                        println!(
                            "  or `stash status {}` to see what is overdue.",
                            etude_cli_support::quoted_path(&bound.root)
                        );
                    }
                    None => println!("  No deadline."),
                }
            }
            if bound.skipped_hidden > 0 {
                println!(
                    "\n  {} hidden {} left in place.",
                    bound.skipped_hidden,
                    if bound.skipped_hidden == 1 {
                        "item was"
                    } else {
                        "items were"
                    }
                );
            }
            print_stash_warnings(&bound);
            if !clear {
                etude_cli_support::envelope::status("incomplete");
                return fail(etude_cli_support::reason::Code::OperationIncomplete);
            }
            ExitCode::SUCCESS
        }
        Err(failure) => {
            let moved = failure.moved;
            let e = failure.error;
            etude_cli_support::envelope::effect("items_moved", moved);
            if let Some(id) = failure.durable_journal_id.as_deref() {
                etude_cli_support::envelope::recovery_operation(id);
            }
            if matches!(e, etude_core::apply::ApplyError::StalePlan(_)) && moved == 0 {
                let root = &bound.root;
                if bound.is_live() {
                    eprintln!(
                        "{}",
                        etude_cli_support::problems::folder_changed_just_now("stash", root)
                    );
                } else {
                    eprintln!(
                        "{}",
                        etude_cli_support::problems::plan_stale_text("stash", root)
                    );
                }
                etude_cli_support::envelope::status("refused");
                return refuse_preserving_status(apply_reason(&e));
            }
            print_stash_failure(
                &e,
                &format!(
                    "could not move {} out of {}",
                    if moved == 0 { "anything" } else { "everything" },
                    etude_cli_support::shown(&bound.root)
                ),
            );
            // Decided from the count of moves, never from the kind of failure.
            if moved == 0 {
                etude_cli_support::envelope::status("refused");
                eprintln!("Nothing was moved, so there is nothing to restore.");
                return apply_exit_code(&e);
            }
            etude_cli_support::envelope::status("incomplete");
            let what = format!(
                "{} moved before this stopped.",
                etude_cli_support::counted(moved, "item was", "items were")
            );
            let them = if moved == 1 { "it" } else { "them" };
            match failure.durable_journal_id.as_deref() {
                Some(id) => eprintln!("{what} To put {them} back: stash pop --id {id}"),
                None => eprintln!(
                    "{what} --no-journal removed undo. Move {them} back by hand from {}.",
                    etude_cli_support::shown(&bound.root.join(&bound.groups[0].name))
                ),
            }
            apply_exit_code(&e)
        }
    }
}

/// Stash moves these on purpose. They are said, and the exit does not change.
fn print_stash_warnings(bound: &etude_core::plan::BoundPlan) {
    if scan::is_synced(&bound.root) {
        println!(
            "warning: this folder is inside a cloud-synced tree, so these moves reach your other devices too."
        );
    }
    let downloads = bound
        .groups
        .iter()
        .flat_map(|group| &group.members)
        .filter(|member| {
            let name = member
                .file_name()
                .map(|name| name.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            scan::IN_FLIGHT_SUFFIXES
                .iter()
                .any(|suffix| name.ends_with(suffix))
        })
        .count();
    if downloads > 0 {
        println!(
            "{} stashed too; {} may fail.",
            etude_cli_support::counted(
                downloads,
                "download still in progress was",
                "downloads still in progress were"
            ),
            if downloads == 1 { "it" } else { "they" }
        );
    }
}

/// What failed, in the words of what the person asked for. The line after this
/// one says what became of the files.
fn print_stash_failure(error: &etude_core::apply::ApplyError, doing: &str) {
    use etude_core::apply::ApplyError as E;
    match error {
        E::Io(source) if etude_cli_support::problems::disk_full(source) => {
            eprintln!("stash: {doing}: the disk is full.");
            eprintln!("       Free some space, then run this again.");
        }
        E::Io(source) if etude_cli_support::problems::disk_read_only(source) => {
            eprintln!("stash: {doing}: the disk is read-only.");
        }
        E::Io(source) => {
            eprintln!(
                "stash: {doing}: {}",
                etude_cli_support::plain_io_error(source)
            );
            if let Some(advice) = scan::permission_advice(source) {
                eprintln!("       {advice}");
            }
        }
        E::Journal(journal) => {
            eprintln!(
                "stash: {}",
                etude_cli_support::journal_write_problem(journal)
            );
            eprintln!(
                "       Free some space or fix that folder, or set ETUDE_STATE_DIR to another one."
            );
        }
        other => eprintln!("stash: {doing}: {}", apply_error_reason(other)),
    }
}

fn cmd_select(args: &[String]) -> ExitCode {
    // Nothing has moved at any of these, whatever the exit.
    let refused = |problem: etude_cli_support::problems::Problem| {
        etude_cli_support::envelope::status("refused");
        problem.report()
    };
    let options = match selection::parse(args) {
        Ok(options) => options,
        Err(problem) => return refused(problem),
    };
    let deadline = match options.duration.as_deref() {
        Some(value) => {
            match parse_duration(value).and_then(|seconds| now_secs().checked_add(seconds)) {
                Some(deadline) => Some(deadline),
                None => {
                    eprintln!("stash: cannot read duration {value:?}. Try 30m, 2h, 3d or 1w.");
                    return refuse(etude_cli_support::reason::Code::InvalidArguments);
                }
            }
        }
        None => None,
    };
    let prepared = match selection::prepare(&options) {
        Ok(prepared) => prepared,
        Err(problem) => return refused(problem),
    };
    if prepared.sources.is_empty() {
        println!("Nothing selected to stash.");
        return ExitCode::from(1);
    }
    let Some(sealer) = sealer(false) else {
        return refuse(etude_cli_support::reason::Code::JournalKeyUnavailable);
    };
    let root = match selection::reserve(&prepared.parent, options.into.is_some()) {
        Ok(root) => root,
        Err(problem) => return refused(problem),
    };
    let plan = selection::plan(&prepared, root.clone(), &holding_name(deadline));
    let mut progress = etude_cli_support::ProgressReporter::stderr("stash", prepared.sources.len());
    let result = etude_core::apply::apply_with_structured_progress_outcome(
        &plan,
        "stash",
        Some(&sealer),
        None,
        |event| {
            if event.phase == etude_core::apply::ProgressPhase::Advancing {
                progress.update(event.completed, event.planned);
            }
        },
    );
    drop(progress);
    match result {
        Ok(report) => {
            etude_cli_support::envelope::recovery_operation(&report.journal_id);
            etude_cli_support::envelope::effect("items_moved", report.moved);
            let held = report.held_tagged
                + report.held_cross_volume
                + report.tagged_not_returned
                + report.hold_record_failed;
            if options.json {
                use etude_core::json as j;
                println!(
                    "{}",
                    j::obj(&[
                        ("action", j::str("stash_selected")),
                        ("stash_id", j::str(&report.journal_id)),
                        ("moved", j::num(report.moved)),
                        ("held_cross_volume", j::num(report.held_cross_volume)),
                        ("held_tagged", j::num(report.held_tagged)),
                        ("tagged_not_returned", j::num(report.tagged_not_returned)),
                        ("selected", j::num(prepared.sources.len())),
                        ("holding_root", j::str("<redacted>")),
                        ("due", deadline.map(j::num).unwrap_or_else(|| "null".into()))
                    ])
                );
            } else {
                println!(
                    "Stashed {} selected {}. Operation id: {}. Restore with: stash pop --id {}",
                    report.moved,
                    if report.moved == 1 { "item" } else { "items" },
                    report.journal_id,
                    report.journal_id
                );
                if held > 0 {
                    print_stash_holds(&report, false);
                }
            }
            if held > 0 {
                etude_cli_support::envelope::status("incomplete");
                return fail(etude_cli_support::reason::Code::OperationIncomplete);
            }
            ExitCode::SUCCESS
        }
        Err(failure) => {
            let moved = failure.moved;
            let error = failure.error;
            etude_cli_support::envelope::effect("items_moved", moved);
            let id = failure
                .durable_journal_id
                .or_else(|| find_stash_id_for_holding(&root, &holding_name(deadline), &sealer));
            if let Some(id) = &id {
                etude_cli_support::envelope::recovery_operation(id);
            }
            print_stash_failure(
                &error,
                if moved == 0 {
                    "could not move the selected items"
                } else {
                    "could not move every selected item"
                },
            );
            // Decided from the count of moves, never from the kind of failure.
            if moved == 0 {
                // The private folder made for this attempt holds nothing.
                let _ = std::fs::remove_dir(&root);
                etude_cli_support::envelope::status("refused");
                eprintln!("Nothing was moved, so there is nothing to restore.");
                return apply_exit_code(&error);
            }
            etude_cli_support::envelope::status("incomplete");
            let what = format!(
                "{} moved before this stopped.",
                etude_cli_support::counted(moved, "item was", "items were")
            );
            let them = if moved == 1 { "it" } else { "them" };
            match id {
                Some(id) => eprintln!("{what} To put {them} back: stash pop --id {id}"),
                None => eprintln!(
                    "{what} See what is held with: stash status --all. {} in {}.",
                    if moved == 1 { "It is" } else { "They are" },
                    etude_cli_support::shown(&root)
                ),
            }
            apply_exit_code(&error)
        }
    }
}

/// What stash left in the folder, with the reason. Counts only: no names, no
/// paths. A held item means the folder is not clear, and this says so.
fn print_stash_holds(r: &etude_core::apply::ApplyReport, folder: bool) {
    if r.held_cross_volume > 0 {
        println!(
            "{} Finder-tagged {} left in the folder: moving to another volume would drop the tag.",
            r.held_cross_volume,
            if r.held_cross_volume == 1 {
                "item was"
            } else {
                "items were"
            }
        );
    }
    if r.held_tagged > 0 {
        println!(
            "{} {} left in the folder because of a Finder tag.",
            r.held_tagged,
            if r.held_tagged == 1 {
                "item was"
            } else {
                "items were"
            }
        );
    }
    if r.tagged_not_returned > 0 {
        println!(
            "{} Finder-tagged {} could not be returned and stays in the holding folder.",
            r.tagged_not_returned,
            if r.tagged_not_returned == 1 {
                "item"
            } else {
                "items"
            }
        );
    }
    if r.hold_record_failed > 0 {
        eprintln!(
            "stash: the undo record could not note an item left in place, so stash stopped. To put back what moved: stash pop --id {}",
            r.journal_id
        );
    }
    if !folder_is_clear(r) {
        if folder {
            println!("The selected folder is NOT clear.\n");
        } else {
            println!("Not every selected item was stashed.");
        }
    }
}

fn unreadable_note(count: usize) -> String {
    if count == 1 {
        "1 item could not be read and stays in place".into()
    } else {
        format!("{count} items could not be read and stay in place")
    }
}

/// The folder is clear only when apply left nothing behind and wrote a
/// journal that agrees. Any held, stranded or unrecorded item says otherwise.
fn folder_is_clear(r: &etude_core::apply::ApplyReport) -> bool {
    r.held_tagged + r.held_cross_volume + r.tagged_not_returned + r.hold_record_failed == 0
}

/// Destination* variants are safety refusals (2); Io/Journal/Injected are failures (3).
fn apply_error_reason(error: &etude_core::apply::ApplyError) -> &'static str {
    use etude_core::apply::ApplyError::*;
    match error {
        DestinationExists(_) => "destination exists",
        DestinationCollision(_) => "destination names collide",
        DestinationIsSynced(_) => "destination is synced",
        DestinationBlocked(_) => "a file holds the name of the holding folder",
        DestinationDirectoryChanged(_) => "destination folder identity changed or was unavailable",
        CannotCompareNames(_) => "destination names cannot be compared",
        Io(_) => "I/O failure",
        Journal(_) => "journal failure",
        Injected(_) => "interrupted apply",
        StalePlan(_) => "selected plan changed",
    }
}

fn apply_exit_code(e: &etude_core::apply::ApplyError) -> ExitCode {
    use etude_core::apply::ApplyError::*;
    match e {
        StalePlan(_)
        | DestinationExists(_)
        | DestinationCollision(_)
        | DestinationIsSynced(_)
        | DestinationBlocked(_)
        | DestinationDirectoryChanged(_)
        | CannotCompareNames(_) => refuse_preserving_status(apply_reason(e)),
        Io(_) | Journal(_) | Injected(_) => fail(apply_reason(e)),
    }
}

/// No done entries means pop already ran. Exit 1. Don't call undo again.
/// Is there nothing left for pop to restore?
///
/// A journal whose tail was cut short cannot answer this from its entries
/// alone. stash moves an item and only then records it, so a lost record is a
/// move that happened and is not written down: every entry can read Planned
/// while the items sit in the stash. Short-circuiting on that says "already
/// popped" and leaves them there, which is the silent stranding this area
/// exists to prevent.
///
/// When the tail is damaged the answer is no, so the restore runs and its
/// successor-entry recovery checks the filesystem instead of the journal.
fn journal_is_fully_undone(j: &etude_core::Journal) -> bool {
    if j.progress_tail_damaged {
        return false;
    }
    // Ask the disk, not just the entries. A journal cut back to its base frame
    // has every entry reading Planned and no torn tail to notice, so the check
    // above passes and the entries agree there is nothing to reverse -- while
    // every file is still at its destination. Answering "already restored"
    // there is not a partial job, it is untrue.
    if etude_core::apply::unrecorded_moves(j) > 0 {
        return false;
    }
    !j.entries.iter().any(|e| e.is_moved())
}

/// Load an enumerated journal, warning on every failure. Even NotFound is
/// a barrier here: a journal that disappeared after discovery may describe
/// a newer operation on the same files. It cannot authorize skipping ahead.
fn load_or_warn(
    tool: &str,
    candidate: &etude_core::journal::JournalCandidate,
    sealer: &dyn etude_core::journal::Sealer,
    damaged: &mut bool,
) -> Option<etude_core::Journal> {
    let id = &candidate.id;
    match candidate.load(tool, sealer) {
        Ok(j) => Some(j),
        Err(e) => {
            *damaged = true;
            eprintln!(
                "{tool}: {}",
                etude_cli_support::problems::record_open_problem(tool, id, &e)
            );
            None
        }
    }
}

/// A journal for `target`, or `None` alongside whether at least one journal
/// on disk was found but refused to load (`damaged`). Distinguishing the two
/// `None` cases matters: a caller that treats "damaged and dropped" the same
/// as "genuinely never existed" reports the wrong exit class. This is the
/// same severity distinction `sweep undo` already makes between `NotFound` (exit
/// 1) and any other load failure (exit 3).
#[cfg(test)]
fn journal_for_root(
    tool: &str,
    sealer: &dyn etude_core::journal::Sealer,
    target: &Path,
) -> (Option<etude_core::Journal>, bool) {
    let mut damaged = false;
    let ids = match etude_core::journal::candidates_by_recency(tool) {
        Ok(ids) => ids,
        Err(etude_core::journal::JournalError::NotFound) => return (None, false),
        Err(e) => {
            eprintln!("{tool}: could not discover journals: {e}");
            return (None, true);
        }
    };
    for id in ids {
        let j = load_or_warn(tool, &id, sealer, &mut damaged);
        if damaged {
            return (None, true);
        }
        if let Some(j) = j
            && j.root
                .canonicalize()
                .is_ok_and(|root| root == target && find_holding(&root).is_some())
        {
            return (Some(j), false);
        }
    }
    (None, false)
}

fn journal_roots(tool: &str, sealer: &dyn etude_core::journal::Sealer) -> Vec<PathBuf> {
    let mut damaged = false;
    etude_core::journal::candidates_by_recency(tool)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|id| load_or_warn(tool, &id, sealer, &mut damaged))
        .filter_map(|j| j.root.canonicalize().ok())
        .filter(|root| find_holding(root).is_some())
        .collect()
}

/// What pop says when it finds nothing to restore. It names what was looked for.
fn no_stash_text(selector: &batches::Selector) -> String {
    use etude_cli_support::{shown, shown_text};
    const SEE_ALL: &str = "See all of them with: stash status --all";
    match selector {
        batches::Selector::Id(id) => {
            format!("stash: no stash with id {}. {SEE_ALL}", shown_text(id))
        }
        batches::Selector::Root(path) | batches::Selector::Latest(Some(path)) => {
            format!("stash: no stash from {}. {SEE_ALL}", shown(path))
        }
        batches::Selector::Latest(None) => "stash: nothing is stashed.".to_owned(),
    }
}

/// The records that would not open, up to three, each with what it needs.
fn print_unreadable(unreadable: &[batches::Unreadable]) {
    for record in unreadable.iter().take(3) {
        eprintln!(
            "stash: {}",
            etude_cli_support::problems::record_open_problem("stash", &record.id, &record.error)
        );
    }
    if unreadable.len() > 3 {
        eprintln!(
            "stash: {} cannot be opened.",
            etude_cli_support::counted(unreadable.len() - 3, "more record", "more records")
        );
    }
}

/// One record that would not open blocks a pop by folder, because it may be the
/// one that belongs to that folder. An id still reaches a record that opens.
fn refuse_unopened_pop(
    asked_id: Option<&str>,
    error: &etude_core::journal::JournalError,
    sealer: &dyn etude_core::journal::Sealer,
) -> ExitCode {
    if let Some(id) = asked_id {
        eprintln!(
            "stash: {} Nothing was restored.",
            etude_cli_support::problems::record_open_problem("stash", id, error)
        );
        return fail(journal_reason(error));
    }
    match batches::survey(sealer) {
        Ok((_, unreadable)) if !unreadable.is_empty() => {
            print_unreadable(&unreadable);
            eprintln!(
                "Such a record may belong to this folder, so nothing was restored.\n\
                 If you know the id printed when you stashed, run: stash pop --id ID\n\
                 See the stashes that do open with: stash status --all"
            );
            fail(journal_reason(&unreadable[0].error))
        }
        // Every record opens, and still none can be chosen: their times tie.
        Ok(_) if matches!(error, etude_core::journal::JournalError::Malformed(_)) => {
            eprintln!(
                "{}",
                etude_cli_support::problems::record_folder_problem("stash", error)
            );
            fail(journal_reason(error))
        }
        Ok(_) => {
            eprintln!(
                "stash: cannot restore: {}. Nothing was restored.",
                etude_cli_support::plain_journal_error(error)
            );
            fail(journal_reason(error))
        }
        Err(store) => {
            eprintln!(
                "{}",
                etude_cli_support::problems::record_folder_problem("stash", &store)
            );
            fail(journal_reason(&store))
        }
    }
}

/// Folders, one to a line, for a message.
fn folder_lines(folders: &[PathBuf]) -> String {
    folders
        .iter()
        .map(|path| format!("  {}", etude_cli_support::shown(path)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Up to three folders in one phrase.
fn folder_phrase(mut folders: Vec<PathBuf>) -> String {
    folders.sort();
    folders.dedup();
    let named: Vec<String> = folders
        .iter()
        .take(3)
        .map(|path| etude_cli_support::shown(path))
        .collect();
    match folders.len().saturating_sub(3) {
        0 => named.join(", "),
        more => format!(
            "{}, and {}",
            named.join(", "),
            etude_cli_support::counted(more, "more folder", "more folders")
        ),
    }
}

/// An undo record that lost more than one entry. Pop moved nothing, and the
/// items can only go home by hand, so this says where they are and where to.
fn refuse_missing_entries(journal: &etude_core::Journal, count: usize) -> ExitCode {
    let held = etude_core::apply::unrecorded_move_folders(journal);
    let home: Vec<PathBuf> = journal
        .entries
        .iter()
        .filter(|entry| entry.state == etude_core::journal::EntryState::Planned)
        .filter(|entry| {
            entry.to.symlink_metadata().is_ok() && entry.from.symlink_metadata().is_err()
        })
        .filter_map(|entry| entry.from.parent().map(Path::to_path_buf))
        .collect();
    eprintln!(
        "stash: cannot restore: this undo record is missing more than one entry. {count} items are in\n\
         the folders below, but the record says they were never moved. Pop could reach only\n\
         the first of them, so it moved nothing. Nothing is lost.\n\
         {}\n\
         To restore them, move them back by hand into {}.",
        folder_lines(&held),
        folder_phrase(home)
    );
    etude_cli_support::envelope::status("refused");
    fail(etude_cli_support::reason::Code::OperationIncomplete)
}

/// Where a journal's items are held, for a person to act on. Stash chose this
/// folder itself.
fn holding_shown(journal: &etude_core::Journal) -> String {
    etude_cli_support::shown(&batches::holding(journal).unwrap_or_else(|_| journal.root.clone()))
}

/// Restoring stopped on `error`. `folders` is where the items were going.
fn print_restore_failure(error: &etude_core::apply::ApplyError, folders: &str) {
    use etude_cli_support::problems::{disk_full, disk_read_only};
    use etude_core::apply::ApplyError as E;
    let doing = format!("stash: could not restore into {folders}");
    match error {
        E::Io(source) if disk_full(source) => eprintln!("{doing}: the disk is full."),
        E::Io(source) if disk_read_only(source) => eprintln!("{doing}: the disk is read-only."),
        E::Io(source) => {
            eprintln!("{doing}: {}.", etude_cli_support::plain_io_error(source));
            if let Some(advice) = scan::permission_advice(source) {
                eprintln!("       {advice}");
            }
        }
        E::Journal(journal) => eprintln!(
            "stash: {}",
            etude_cli_support::journal_write_problem(journal)
        ),
        E::DestinationDirectoryChanged(path) => eprintln!(
            "{doing}: {} was moved or replaced while stash was working.",
            etude_cli_support::shown(path)
        ),
        other => eprintln!("{doing}: {}.", apply_error_reason(other)),
    }
}

/// What pop left behind and how to finish. Names of files stash found by itself
/// stay redacted; the folder that holds them is its own.
/// The newest other stash, for a step past one whose items are gone.
fn other_stash(sealer: &dyn etude_core::journal::Sealer, id: &str) -> Option<etude_core::Journal> {
    let (live, _) = batches::survey(sealer).ok()?;
    live.into_iter()
        .filter(|journal| journal.id != id)
        .max_by_key(|journal| batches::creation(&journal.id))
}

fn print_pop_left(
    journal: &etude_core::Journal,
    r: &etude_core::apply::UndoReport,
    sealer: &dyn etude_core::journal::Sealer,
) {
    use etude_cli_support::counted;
    let holding = holding_shown(journal);
    let step = format!("stash pop --id {}", journal.id);
    if !r.skipped_changed.is_empty() {
        let one = r.skipped_changed.len() == 1;
        println!(
            "  {} while stashed and {} left alone. {} still held at:",
            counted(r.skipped_changed.len(), "item changed", "items changed"),
            if one { "was" } else { "were" },
            if one { "It is" } else { "They are" },
        );
        for path in &r.skipped_changed {
            println!("    {}", etude_core::redact::path(path));
        }
        println!(
            "  See what is still held with: stash inspect --id {}",
            journal.id
        );
    }
    if !r.skipped_occupied.is_empty() {
        let one = r.skipped_occupied.len() == 1;
        println!(
            "  {} not restored because {} original {} taken by another file. {} still held at:",
            counted(r.skipped_occupied.len(), "item was", "items were"),
            if one { "its" } else { "their" },
            if one { "place is" } else { "places are" },
            if one { "It is" } else { "They are" },
        );
        for path in &r.skipped_occupied {
            println!("    {}", etude_core::redact::path(path));
        }
        println!(
            "  Move the other {} away, then run: {step}",
            if one { "file" } else { "files" }
        );
    }
    if !r.skipped_missing.is_empty() {
        let one = r.skipped_missing.len() == 1;
        println!(
            "  {} not where stash left {} ({holding}).",
            counted(r.skipped_missing.len(), "item is", "items are"),
            if one { "it" } else { "them" },
        );
        println!("  If you moved that folder, move it back and run: {step}");
        println!(
            "  If you deleted {}, there is nothing to restore.",
            if one { "it" } else { "them" }
        );
        if let Some(other) = other_stash(sealer, &journal.id) {
            println!(
                "  To restore the stash from {} instead, run: stash pop --id {}",
                etude_cli_support::shown(&other.root),
                other.id
            );
        }
    }
}

fn cmd_pop(args: &[String]) -> ExitCode {
    let (selector_args, item_id) = match pop_item_args(args) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("stash: {error}");
            return refuse(etude_cli_support::reason::Code::InvalidArguments);
        }
    };
    let selector = match batches::selector(&selector_args) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("stash: {error}");
            return refuse(etude_cli_support::reason::Code::InvalidArguments);
        }
    };
    if item_id.is_some() && !matches!(&selector, batches::Selector::Id(_)) {
        eprintln!("stash: --item requires --id OPERATION");
        return refuse(etude_cli_support::reason::Code::InvalidArguments);
    }
    if let batches::Selector::Root(path) | batches::Selector::Latest(Some(path)) = &selector {
        const SEE_ALL: &str = "See what is stashed with: stash status --all";
        if path.is_file() {
            eprintln!(
                "stash: {} is a file. Give the folder you stashed. {SEE_ALL}",
                etude_cli_support::shown(path)
            );
            return fail(etude_cli_support::reason::Code::MissingInput);
        }
        if let Err(error) = path.canonicalize() {
            let mut problem =
                etude_cli_support::problems::folder_open_problem("stash", path, &error);
            problem.text = format!("{} {SEE_ALL}", problem.text);
            return problem.report();
        }
    }
    let Some(sl) = sealer(false) else {
        return refuse(etude_cli_support::reason::Code::JournalKeyUnavailable);
    };
    // An id no journal holds is the same answer `status --id` gives. A folder
    // with nothing stashed is a different question and keeps its own reason.
    let asked_id = match &selector {
        batches::Selector::Id(id) => Some(id.clone()),
        _ => None,
    };
    let nothing_found = no_stash_text(&selector);
    let mut j = match batches::select(selector, &sl) {
        Ok(Some(journal)) => journal,
        Ok(None) => {
            eprintln!("{nothing_found}");
            refusal_reason(if asked_id.is_some() {
                etude_cli_support::reason::Code::UnknownOperation
            } else {
                etude_cli_support::reason::Code::MissingInput
            });
            etude_cli_support::envelope::status("nothing_to_do");
            return ExitCode::from(1);
        }
        Err(etude_core::journal::JournalError::Malformed(batches::AMBIGUOUS)) => {
            return refuse_ambiguous_pop(&selector_args, &sl);
        }
        Err(error) => return refuse_unopened_pop(asked_id.as_deref(), &error, &sl),
    };
    etude_cli_support::envelope::recovery_operation(&j.id);
    let due = match batches::due(&j) {
        Ok(value) => value,
        Err(error) => return refuse_unopened_pop(Some(&j.id), &error, &sl),
    };
    match pop_clock(due, now_secs(), flag(args, "--if-due")) {
        ClockDecision::RefuseEarly { human } => {
            eprintln!(
                "stash: due in {human}, so nothing was restored. To restore now, run it without --if-due."
            );
            return refuse(etude_cli_support::reason::Code::NotDueYet);
        }
        ClockDecision::NoticeEarly { human, due } => {
            println!("due {}; popping {human} early.", iso_utc(due))
        }
        ClockDecision::Pop => {}
    }
    // --if-due on a stash with no deadline restores: no deadline is left to wait for.
    let no_deadline = flag(args, "--if-due") && due.is_none();
    const NO_DEADLINE: &str = "This stash had no deadline, so it was restored.";
    if let Some(item_id) = item_id {
        let code = pop_one_item(&mut j, &item_id, &sl);
        if no_deadline && code == ExitCode::SUCCESS {
            println!("{NO_DEADLINE}");
        }
        return code;
    }
    if journal_is_fully_undone(&j) {
        println!("\nNothing to restore. This stash was already popped.");
        return ExitCode::from(1);
    }
    // Before the counts, because it changes what they mean. A journal cut
    // short describes less than the stash actually did, so the count below is
    // a floor. Restoring quietly from damaged state is the failure this
    // disclosure exists to prevent -- the same reason `sweep undo` says it.
    // Said, not signalled. A torn tail is the ordinary outcome of an
    // interrupted run -- stash moves an item and only then records it, so any
    // kill mid-run leaves one -- and the items are back. Exiting non-zero for
    // that would make routine crash recovery read as failure to anything
    // checking the code. The damage goes to stderr where a person sees it;
    // the exit code stays the truth about whether the restore worked.
    //
    // This used to exit 3 while `sweep undo` exited 0 on the same condition.
    // The asymmetry was an accident, not a design.
    //
    // Only a pop that is going ahead says so. More than one lost record is
    // refused below, before anything moves.
    if j.progress_tail_damaged && etude_core::apply::unrecorded_moves(&j) <= 1 {
        eprintln!(
            "stash: this undo record is damaged: it was cut short, so its last move is not on record.\n\
             \x20      Pop restores everything it lists, and checks the folder for the\n\
             \x20      item whose entry was lost."
        );
    }

    let mut progress = etude_cli_support::ProgressReporter::stderr("stash pop", j.entries.len());
    let r = etude_core::apply::undo_with_progress(&mut j, &sl, |p| {
        progress.update(p.completed, p.total)
    });
    drop(progress);
    if r.unrecorded_moves > 1 {
        return refuse_missing_entries(&j, r.unrecorded_moves);
    }
    // Report what actually happened before anything about the outcome: this
    // count is real even when `r.error` is set below.
    etude_cli_support::envelope::detail(etude_core::json::obj(&[
        ("restored", etude_core::json::num(r.restored)),
        ("stash_id", etude_core::json::str(&j.id)),
        (
            "skipped_changed",
            etude_core::json::num(r.skipped_changed.len()),
        ),
        (
            "skipped_occupied",
            etude_core::json::num(r.skipped_occupied.len()),
        ),
        (
            "skipped_missing",
            etude_core::json::num(r.skipped_missing.len()),
        ),
    ]));
    // Any item not restored leaves the request unfinished, whatever the cause.
    let left = r.skipped_changed.len() + r.skipped_occupied.len() + r.skipped_missing.len();
    if left > 0 || r.error.is_some() {
        etude_cli_support::envelope::status("incomplete");
    }
    etude_cli_support::envelope::effect("items_restored", r.restored);
    etude_cli_support::envelope::effect("changed_items_left", r.skipped_changed.len());
    etude_cli_support::envelope::effect("occupied_items_left", r.skipped_occupied.len());
    etude_cli_support::envelope::effect("missing_items", r.skipped_missing.len());
    let still_out = j
        .entries
        .iter()
        .filter(|entry| entry.is_moved())
        .count()
        .max(left);
    if still_out > 0 {
        println!(
            "\nRestored {} of {}.",
            r.restored,
            etude_cli_support::counted(r.restored + still_out, "item", "items")
        );
    } else {
        println!(
            "\nRestored {}.",
            etude_cli_support::counted(r.restored, "item", "items")
        );
    }
    if no_deadline {
        println!("{NO_DEADLINE}");
    }
    print_pop_left(&j, &r, &sl);
    if r.already_reversed > 0 {
        println!(
            "  {} already restored by an earlier run that did not finish.",
            etude_cli_support::counted(r.already_reversed, "item was", "items were")
        );
    }
    if !r.reconciled.is_empty() {
        // Different from healed: nothing was reachable by two names here. An
        // earlier undo moved these home and was killed before it could write
        // that down, so this run only had to agree with the disk.
        println!(
            "  {} already home from an interrupted restore; the undo record now says so.",
            etude_cli_support::counted(r.reconciled.len(), "item was", "items were")
        );
    }
    if !r.healed.is_empty() {
        println!(
            "  {} left over from an interrupted run: one file was reachable by two\n  names and the extra name has been removed.",
            r.healed.len()
        );
    }
    // Persist regardless of outcome: on error just as much as on success, so
    // the on-disk journal matches what was actually restored rather than
    // still claiming every entry is pending. Whether *this* save itself
    // succeeded changes what we can honestly tell the user next -- claiming
    // "resumable" while the save failed would repeat the exact lie this fix
    // exists to remove, just moved one line later.
    let saved = j.save_sealed(&sl);
    let step = format!("stash pop --id {}", j.id);
    let next_pop_corrects =
        format!("The next `{step}` will see which items are already home and correct the record.");
    if let Some(error) = &r.error {
        let going_home: Vec<PathBuf> = j
            .entries
            .iter()
            .filter(|entry| entry.is_moved())
            .filter_map(|entry| entry.from.parent().map(Path::to_path_buf))
            .collect();
        print_restore_failure(error, &folder_phrase(going_home));
        match saved {
            Ok(()) => eprintln!("Fix that, then run: {step}"),
            Err(save_err) => eprintln!(
                "stash: the undo record could not be updated either: {}. {next_pop_corrects}",
                etude_cli_support::plain_journal_error(&save_err)
            ),
        }
        if matches!(error, etude_core::apply::ApplyError::Io(source) if etude_cli_support::problems::disk_full(source))
        {
            return fail(etude_cli_support::reason::Code::InsufficientSpace);
        }
        return fail(etude_cli_support::reason::Code::OperationIncomplete);
    }
    if let Err(save_err) = saved {
        eprintln!(
            "stash: the restored items are back, but the undo record could not be updated: {}. {next_pop_corrects}",
            etude_cli_support::plain_journal_error(&save_err)
        );
        return fail(etude_cli_support::reason::Code::JournalWriteFailed);
    }
    if r.skipped_changed.is_empty() && r.skipped_occupied.is_empty() {
        selection::remove_empty_root(&j);
    }
    if left > 0 {
        // Exit 0 means the whole request was done, and this one was not.
        return fail(etude_cli_support::reason::Code::OperationIncomplete);
    }
    ExitCode::SUCCESS
}

fn pop_item_args(args: &[String]) -> Result<(Vec<String>, Option<String>), &'static str> {
    let mut cleaned = Vec::with_capacity(args.len());
    let mut item = None;
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--item" {
            if item.is_some() {
                return Err("--item may be supplied once");
            }
            let value = args.get(index + 1).ok_or("--item requires an item id")?;
            if value.starts_with('-') {
                return Err("--item requires an item id");
            }
            item = Some(value.clone());
            index += 2;
        } else {
            cleaned.push(args[index].clone());
            index += 1;
        }
    }
    if item.is_some() && !cleaned.iter().any(|arg| arg == "--id") {
        return Err("--item requires --id OPERATION");
    }
    Ok((cleaned, item))
}

fn opaque_item_id(journal: &etude_core::Journal, index: usize) -> String {
    // A stable, path-independent handle. The id is only a selector, not an
    // authentication token; journal access remains protected by its sealer.
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in journal.id.bytes().chain((index as u64).to_le_bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    format!("item-{hash:016x}")
}

fn item_state(journal: &etude_core::Journal, index: usize) -> Result<&'static str, std::io::Error> {
    let entry = &journal.entries[index];
    if entry.state == etude_core::journal::EntryState::Reversed {
        return Ok("restored");
    }
    match std::fs::symlink_metadata(&entry.to) {
        Ok(_) => Ok("held"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok("not_held"),
        Err(error) => Err(error),
    }
}

fn pop_one_item(
    journal: &mut etude_core::Journal,
    item_id: &str,
    sealer: &dyn etude_core::journal::Sealer,
) -> ExitCode {
    use etude_cli_support::counted;
    etude_cli_support::envelope::recovery_operation(&journal.id);
    let shown_item = etude_cli_support::shown_text(item_id);
    let whole_stash = format!("stash pop --id {}", journal.id);
    let inspect = format!("stash inspect --id {}", journal.id);
    let mut selected = None;
    for index in 0..journal.entries.len() {
        if opaque_item_id(journal, index) == item_id && selected.replace(index).is_some() {
            eprintln!(
                "stash: two items in this stash share the id {shown_item}, so nothing was restored. Restore the whole stash with: {whole_stash}"
            );
            return fail(etude_cli_support::reason::Code::InternalError);
        }
    }
    let Some(index) = selected else {
        eprintln!(
            "stash: no item {shown_item} in this stash. Nothing was restored. List them with: {inspect}"
        );
        return refuse(etude_cli_support::reason::Code::UnknownItem);
    };
    if journal.entries[index].state == etude_core::journal::EntryState::Reversed {
        println!("That item is already restored.");
        return ExitCode::from(1);
    }

    let going_home = journal.entries[index]
        .from
        .parent()
        .map(etude_cli_support::shown)
        .unwrap_or_default();
    let report = match etude_core::apply::undo_entry(journal, index, sealer) {
        Ok(report) => report,
        Err(_) => {
            eprintln!(
                "stash: that item is not in this stash's undo record. Nothing was restored. See what it holds with: {inspect}"
            );
            return fail(etude_cli_support::reason::Code::OperationIncomplete);
        }
    };
    if report.unrecorded_moves > 1 {
        return refuse_missing_entries(journal, report.unrecorded_moves);
    }
    let saved = journal.save_sealed(sealer);
    let remaining = match batches::held(journal) {
        Ok(value) => value,
        Err(error) => {
            eprintln!(
                "stash: restored {}, but could not count what is still held: {}. See it with: {inspect}",
                counted(report.restored, "item", "items"),
                etude_cli_support::plain_journal_error(&error)
            );
            return fail(etude_cli_support::reason::Code::PermissionDenied);
        }
    };
    etude_cli_support::envelope::detail(etude_core::json::obj(&[
        ("stash_id", etude_core::json::str(&journal.id)),
        ("item_id", etude_core::json::str(item_id)),
        ("restored", etude_core::json::num(report.restored)),
        ("remaining_held", etude_core::json::num(remaining)),
        (
            "skipped_changed",
            etude_core::json::num(report.skipped_changed.len()),
        ),
        (
            "skipped_occupied",
            etude_core::json::num(report.skipped_occupied.len()),
        ),
        (
            "skipped_missing",
            etude_core::json::num(report.skipped_missing.len()),
        ),
    ]));
    etude_cli_support::envelope::effect("items_restored", report.restored);
    etude_cli_support::envelope::effect("items_still_held", remaining);
    if report.error.is_some() || report.restored == 0 {
        etude_cli_support::envelope::status("incomplete");
    }
    let still_held = format!(
        "{} held in this operation.",
        counted(remaining, "item remains", "items remain")
    );
    if let Err(error) = saved {
        let cause = etude_cli_support::plain_journal_error(&error);
        if report.restored > 0 {
            eprintln!(
                "stash: the item is back, but the undo record could not be updated: {cause}. The next `{whole_stash}` will see which items are already home and correct the record."
            );
        } else {
            eprintln!("stash: the undo record could not be updated: {cause}.");
        }
        return fail(etude_cli_support::reason::Code::JournalWriteFailed);
    }
    let this_item = format!("{whole_stash} --item {shown_item}");
    if let Some(error) = report.error {
        print_restore_failure(&error, &going_home);
        eprintln!("{still_held} Fix that, then run: {this_item}");
        return fail(apply_reason(&error));
    }
    if report.restored > 0 {
        println!(
            "Restored {}; {}",
            counted(report.restored, "item", "items"),
            still_held
        );
    } else {
        println!("Nothing was restored. {still_held}");
    }
    let holding = holding_shown(journal);
    if !report.skipped_changed.is_empty() {
        println!(
            "  That item changed while stashed and was left alone. It is still held in {holding}."
        );
    }
    if !report.skipped_occupied.is_empty() {
        println!(
            "  Its original place is taken by another file. It is still held in {holding}. Move the other file away, then run: {this_item}"
        );
    }
    if !report.skipped_missing.is_empty() {
        println!(
            "  That item is not where stash left it ({holding}). If you moved that folder, move it back and run: {this_item}\n  If you deleted it, there is nothing to restore."
        );
    }
    if report.restored == 0 {
        refuse_preserving_status(etude_cli_support::reason::Code::OperationIncomplete)
    } else {
        selection::remove_empty_root(journal);
        ExitCode::SUCCESS
    }
}

fn cmd_inspect(args: &[String]) -> ExitCode {
    let (selector, id) = match batches::selector(args) {
        Ok(batches::Selector::Id(id)) => (batches::Selector::Id(id.clone()), id),
        Ok(_) => {
            eprintln!("stash: inspect requires --id OPERATION");
            return refuse(etude_cli_support::reason::Code::InvalidArguments);
        }
        Err(error) => {
            eprintln!("stash: {error}");
            return refuse(etude_cli_support::reason::Code::InvalidArguments);
        }
    };
    let Some(sealer) = sealer(false) else {
        return refuse(etude_cli_support::reason::Code::JournalKeyUnavailable);
    };
    let journal = match batches::select(selector, &sealer) {
        Ok(Some(journal)) => journal,
        Ok(None) => {
            eprintln!("stash: no operation found for that id");
            refusal_reason(etude_cli_support::reason::Code::UnknownOperation);
            etude_cli_support::envelope::status("nothing_to_do");
            return ExitCode::from(1);
        }
        Err(error) => {
            eprintln!(
                "stash: {}",
                etude_cli_support::problems::record_open_problem("stash", &id, &error)
            );
            return fail(journal_reason(&error));
        }
    };
    let mut items = Vec::with_capacity(journal.entries.len());
    let mut held = 0usize;
    for index in 0..journal.entries.len() {
        let state = match item_state(&journal, index) {
            Ok(state) => state,
            Err(error) => {
                eprintln!(
                    "stash: cannot check what is held: {}.",
                    etude_cli_support::plain_io_error(&error)
                );
                return fail(if error.kind() == std::io::ErrorKind::PermissionDenied {
                    etude_cli_support::reason::Code::PermissionDenied
                } else {
                    etude_cli_support::reason::Code::InternalError
                });
            }
        };
        held += usize::from(state == "held");
        items.push((opaque_item_id(&journal, index), state));
    }
    if flag(args, "--json") {
        use etude_core::json as j;
        let rows = items
            .iter()
            .map(|(id, state)| {
                j::obj(&[
                    ("id", j::str(id)),
                    ("state", j::str(state)),
                    ("held", j::bool(*state == "held")),
                ])
            })
            .collect::<Vec<_>>();
        println!(
            "{}",
            j::obj(&[
                ("id", j::str(&journal.id)),
                ("held", j::num(held)),
                ("total", j::num(items.len())),
                ("items", j::arr(rows)),
                ("paths_shown", j::bool(false)),
            ])
        );
    } else {
        println!(
            "Operation {}: {held} of {} {} held.",
            journal.id,
            items.len(),
            if items.len() == 1 {
                "item remains"
            } else {
                "items remain"
            }
        );
        for (id, state) in &items {
            println!("  {id}  {state}");
        }
    }
    ExitCode::SUCCESS
}

/// The --paths disclosure gate, before ANY other work. Split out and called
/// first so it is the deterministic refusal for a non-interactive --paths
/// call. It once ran after sealer(), so a caller with no
/// keychain got "could not store the key" instead of the disclosure reason,
/// and the contract (refuse, and say it is because --paths discloses
/// machine-wide paths) was violated by ordering. Takes an explicit tty so a
/// test can drive both branches without a real terminal.
fn paths_gate(show_paths: bool, is_tty: bool) -> Result<(), String> {
    if show_paths && !is_tty {
        return Err(
            "stash: --paths reveals every stashed folder's location on this\n\
             machine, so it answers only to a person at a terminal. Without\n\
             --paths you still get ids, deadlines and redacted roots -- enough\n\
             to schedule against. Refusing."
                .to_string(),
        );
    }
    Ok(())
}

fn cmd_status_all(args: &[String]) -> ExitCode {
    let show_paths = flag(args, "--paths");
    // Disclosure gate BEFORE the keychain: a machine-wide-read refusal must
    // not be pre-empted by an unrelated key error, or the reason is wrong.
    if let Err(msg) = paths_gate(
        show_paths,
        std::io::IsTerminal::is_terminal(&std::io::stdin()),
    ) {
        eprintln!("{msg}");
        return refuse(etude_cli_support::reason::Code::PermissionDenied);
    }
    let Some(sl) = sealer(false) else {
        return refuse(etude_cli_support::reason::Code::JournalKeyUnavailable);
    };
    let (stashes, unreadable) = match batches::survey(&sl) {
        Ok(value) => value,
        Err(error) => {
            eprintln!(
                "{}",
                etude_cli_support::problems::record_folder_problem("stash", &error)
            );
            return fail(journal_reason(&error));
        }
    };
    if flag(args, "--json") {
        use etude_core::json as j;
        let rows = match stashes
            .iter()
            .map(|batch| batches::row(batch, show_paths))
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(rows) => rows,
            Err(error) => {
                eprintln!(
                    "stash: cannot check what is held: {}.",
                    etude_cli_support::plain_journal_error(&error)
                );
                return fail(journal_reason(&error));
            }
        };
        println!(
            "{}",
            j::obj(&[
                ("stashes", j::arr(rows)),
                (
                    "unreadable",
                    j::arr(
                        unreadable
                            .iter()
                            .map(|record| j::str(&record.id))
                            .collect::<Vec<_>>()
                    )
                ),
                ("paths_shown", j::bool(show_paths))
            ])
        );
    } else if stashes.is_empty() {
        if unreadable.is_empty() {
            println!("Nothing is stashed.");
        }
    } else {
        println!(
            "{}:",
            etude_cli_support::counted(stashes.len(), "live operation", "live operations")
        );
        for batch in &stashes {
            let due = match batches::due(batch) {
                Ok(value) => value,
                Err(error) => {
                    eprintln!(
                        "stash: {}",
                        etude_cli_support::problems::record_open_problem(
                            "stash", &batch.id, &error
                        )
                    );
                    return fail(journal_reason(&error));
                }
            };
            let shown = if show_paths {
                batch.root.display().to_string()
            } else {
                "<redacted>".to_string()
            };
            match due {
                Some(value) if value <= now_secs() => {
                    println!("  {}  {shown}  OVERDUE since {}", batch.id, iso_utc(value))
                }
                Some(value) => println!("  {}  {shown}  due {}", batch.id, iso_utc(value)),
                None => println!("  {}  {shown}  no deadline", batch.id),
            }
            if let Some(note) = damage_note(batch) {
                println!("{note}");
            }
        }
        if !show_paths {
            println!("  paths are redacted; `stash status --all --paths` shows them.");
        }
    }
    if let Some(first) = unreadable.first() {
        print_unreadable(&unreadable);
        if !stashes.is_empty() {
            println!("The stashes listed above still open with: stash pop --id ID");
            etude_cli_support::envelope::status("incomplete");
        }
        return fail(journal_reason(&first.error));
    }
    if stashes.is_empty() {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

/// What a damaged undo record means for the step printed beside it.
fn damage_note(batch: &etude_core::Journal) -> Option<String> {
    if etude_core::apply::unrecorded_moves(batch) > 1 {
        return Some(format!(
            "  {}: this undo record is damaged, and stash pop will refuse it. The items are still in {}.",
            batch.id,
            holding_shown(batch)
        ));
    }
    batch.progress_tail_damaged.then(|| {
        format!(
            "  {}: this undo record is damaged. Its last move is not on record, so stash pop checks the folder for that item.",
            batch.id
        )
    })
}

fn cmd_status(args: &[String]) -> ExitCode {
    if flag(args, "--id") {
        return cmd_status_id(args);
    }
    if flag(args, "--all") {
        return cmd_status_all(args);
    }
    let root = args
        .iter()
        .skip(1)
        .find(|a| !a.starts_with('-'))
        .map(|p| PathBuf::from(expand_tilde(p)))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    if root.is_file() {
        eprintln!(
            "stash: {} is a file. Give the folder you stashed.",
            etude_cli_support::shown(&root)
        );
        return fail(etude_cli_support::reason::Code::MissingInput);
    }
    let root = match root.canonicalize() {
        Ok(root) => root,
        Err(error) => {
            return etude_cli_support::problems::folder_open_problem("stash", &root, &error)
                .report();
        }
    };

    let Some(sealer) = sealer(false) else {
        return refuse(etude_cli_support::reason::Code::JournalKeyUnavailable);
    };
    {
        let batches = match batches::survey(&sealer) {
            Ok((batches, unreadable)) => match unreadable.first() {
                None => batches,
                Some(first) => {
                    print_unreadable(&unreadable);
                    eprintln!(
                        "Such a record may belong to this folder, so stash cannot say what is held here.\n\
                         See the stashes that do open with: stash status --all"
                    );
                    return fail(journal_reason(&first.error));
                }
            },
            Err(error) => {
                eprintln!(
                    "{}",
                    etude_cli_support::problems::record_folder_problem("stash", &error)
                );
                return fail(journal_reason(&error));
            }
        };
        let batches: Vec<_> = batches
            .into_iter()
            .filter(|batch| batches::matches_root(batch, &root))
            .collect();
        if !batches.is_empty() {
            let rows = match batches
                .iter()
                .map(|batch| batches::row(batch, false))
                .collect::<Result<Vec<_>, _>>()
            {
                Ok(value) => value,
                Err(error) => {
                    eprintln!(
                        "stash: cannot check what is held: {}.",
                        etude_cli_support::plain_journal_error(&error)
                    );
                    return fail(journal_reason(&error));
                }
            };
            let counts = match batches
                .iter()
                .map(batches::held)
                .collect::<Result<Vec<_>, _>>()
            {
                Ok(value) => value,
                Err(error) => {
                    eprintln!(
                        "stash: cannot check what is held: {}.",
                        etude_cli_support::plain_journal_error(&error)
                    );
                    return fail(journal_reason(&error));
                }
            };
            let unique = batches.len() == 1;
            let due = if unique {
                batches::due(&batches[0]).ok().flatten()
            } else {
                None
            };
            if flag(args, "--json") {
                use etude_core::json as j;
                println!(
                    "{}",
                    j::obj(&[
                        ("root", j::str("<redacted>")),
                        ("stashed", j::num(counts.iter().sum::<usize>())),
                        (
                            "id",
                            if unique {
                                j::str(&batches[0].id)
                            } else {
                                "null".into()
                            }
                        ),
                        ("due", due.map(j::num).unwrap_or_else(|| "null".into())),
                        (
                            "due_iso",
                            due.map(|value| j::str(&iso_utc(value)))
                                .unwrap_or_else(|| "null".into())
                        ),
                        (
                            "overdue",
                            j::bool(due.is_some_and(|value| value <= now_secs()))
                        ),
                        ("elsewhere", "null".into()),
                        ("stashes", j::arr(rows))
                    ])
                );
            } else {
                println!(
                    "{} in {} stashed from the selected folder.",
                    etude_cli_support::counted(counts.iter().sum::<usize>(), "item", "items"),
                    etude_cli_support::counted(batches.len(), "operation", "operations"),
                );
                for batch in &batches {
                    match damage_note(batch) {
                        Some(note) if etude_core::apply::unrecorded_moves(batch) > 1 => {
                            println!("{note}")
                        }
                        Some(note) => {
                            println!("{note}");
                            println!("  {}: restore with stash pop --id {}", batch.id, batch.id);
                        }
                        None => {
                            println!("  {}: restore with stash pop --id {}", batch.id, batch.id)
                        }
                    }
                }
                if let Some(due) = due {
                    if due <= now_secs() {
                        println!("  OVERDUE since {}", human_time(due));
                    } else {
                        println!("  Due back {}", human_time(due));
                    }
                }
            }
            return ExitCode::SUCCESS;
        }
    }

    // Was a folder named on the command line? If so the user asked about that
    // one folder and an answer about a different one would be noise.
    let asked_for_a_folder = args.iter().skip(1).any(|a| !a.starts_with('-'));

    match find_holding(&root) {
        None => {
            let other = if asked_for_a_folder {
                None
            } else {
                stash_elsewhere(&root)
            };
            if flag(args, "--json") {
                use etude_core::json as j;
                println!(
                    "{}",
                    j::obj(&[
                        ("root", j::path(&root)),
                        ("stashed", j::num(0)),
                        ("due", "null".into()),
                        ("overdue", j::bool(false)),
                        (
                            "elsewhere",
                            other
                                .as_deref()
                                .map(j::path)
                                .unwrap_or_else(|| "null".into())
                        ),
                    ])
                );
                return ExitCode::from(1);
            }
            println!("{}", nothing_here(&root, other.as_deref()));
            ExitCode::from(1)
        }
        Some(dir) => {
            let n = etude_core::scan::observe_read(
                "holding_directory_enumeration",
                std::fs::read_dir(&dir),
            )
            .map(|r| r.flatten().count())
            .unwrap_or(0);
            let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            if flag(args, "--json") {
                use etude_core::json as j;
                let due = deadline_of(name);
                println!(
                    "{}",
                    j::obj(&[
                        ("root", j::path(&root)),
                        ("stashed", j::num(n)),
                        ("id", j::str(name)),
                        ("due", due.map(j::num).unwrap_or_else(|| "null".into())),
                        (
                            "due_iso",
                            due.map(|t| j::str(&iso_utc(t)))
                                .unwrap_or_else(|| "null".into()),
                        ),
                        ("overdue", j::bool(due.is_some_and(|t| t <= now_secs()))),
                        ("elsewhere", "null".into()),
                    ])
                );
                return ExitCode::SUCCESS;
            }
            println!(
                "\n{} stashed from {}.",
                etude_cli_support::counted(n, "item", "items"),
                root.display()
            );
            match deadline_of(name) {
                Some(t) if t <= now_secs() => {
                    println!(
                        "  OVERDUE since {}. run `stash pop {}`",
                        human_time(t),
                        root.display()
                    );
                }
                Some(t) => println!("  Due back {}", human_time(t)),
                None => println!("  No deadline set."),
            }
            println!("\nRestore with: stash pop {}", root.display());
            ExitCode::SUCCESS
        }
    }
}

fn status_id_payload(journal: &etude_core::Journal) -> String {
    use etude_core::json as j;
    let summary = journal.summary();
    let state = match summary.state {
        etude_core::journal::JournalState::Refused => "refused",
        etude_core::journal::JournalState::Completed => "completed",
        etude_core::journal::JournalState::Interrupted => "interrupted",
        etude_core::journal::JournalState::Reversed => "reversed",
        etude_core::journal::JournalState::PartlyReversed => "partly_reversed",
    };
    j::obj(&[
        ("operation_id", j::str(&summary.id)),
        ("tool", j::str(&summary.tool)),
        ("state", j::str(state)),
        ("planned", j::num(summary.planned)),
        ("moved", j::num(summary.moved)),
        ("reversed", j::num(summary.reversed)),
        (
            "resume_or_undo",
            j::str(&format!("stash pop --id {}", summary.id)),
        ),
    ])
}

fn cmd_status_id(args: &[String]) -> ExitCode {
    if args.iter().filter(|arg| arg.as_str() == "--id").count() != 1
        || flag(args, "--all")
        || flag(args, "--paths")
        || args
            .iter()
            .skip(1)
            .filter(|arg| !arg.starts_with('-'))
            .count()
            != 1
    {
        eprintln!("stash: status --id accepts one operation id and --json");
        return refuse(etude_cli_support::reason::Code::InvalidArguments);
    }
    let Some(id) = value(args, "--id") else {
        eprintln!("stash: status --id requires an operation id");
        return refuse(etude_cli_support::reason::Code::InvalidArguments);
    };
    if etude_core::journal::validate_id(&id).is_err() {
        eprintln!("stash: invalid operation id");
        return refuse(etude_cli_support::reason::Code::InvalidArguments);
    }
    let path = etude_core::journal::state_dir().join(format!("stash-{id}.journal"));
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("stash: no operation found for that id");
            refusal_reason(etude_cli_support::reason::Code::UnknownOperation);
            etude_cli_support::envelope::status("nothing_to_do");
            return ExitCode::from(1);
        }
        Err(error) => {
            eprintln!(
                "stash: cannot read the undo record for {}: {}.",
                etude_cli_support::shown_text(&id),
                etude_cli_support::plain_io_error(&error)
            );
            return fail(etude_cli_support::reason::Code::JournalUnreadable);
        }
        Ok(_) => {}
    }
    let key = match etude_keep::key_if_present() {
        Ok(Some(key)) => key,
        Ok(None) => {
            eprintln!(
                "{}",
                etude_cli_support::journal_key_problem(
                    "stash",
                    &"there is no key in the login keychain, so undo records cannot be read",
                    false,
                    false,
                )
            );
            return refuse(etude_cli_support::reason::Code::JournalKeyUnavailable);
        }
        Err(error) => {
            eprintln!(
                "{}",
                etude_cli_support::journal_key_problem(
                    "stash",
                    &error,
                    matches!(error, etude_keep::KeepError::SuppliedKey),
                    false,
                )
            );
            return refuse(etude_cli_support::reason::Code::JournalKeyUnavailable);
        }
    };
    let sealer = KeychainSeal { key };
    match etude_core::Journal::load_sealed_exact("stash", &id, &sealer) {
        Ok(journal) => {
            if flag(args, "--json") {
                println!("{}", status_id_payload(&journal));
            } else {
                let summary = journal.summary();
                let state = match summary.state {
                    etude_core::journal::JournalState::Refused => "refused",
                    etude_core::journal::JournalState::Completed => "completed",
                    etude_core::journal::JournalState::Interrupted => "interrupted",
                    etude_core::journal::JournalState::Reversed => "reversed",
                    etude_core::journal::JournalState::PartlyReversed => "partly reversed",
                };
                println!(
                    "Operation {}: {state}; planned {}, moved {}, reversed {}. Resume or undo with: stash pop --id {}",
                    summary.id, summary.planned, summary.moved, summary.reversed, summary.id
                );
            }
            ExitCode::SUCCESS
        }
        Err(etude_core::journal::JournalError::NotFound) => {
            eprintln!("stash: no operation found for that id");
            refusal_reason(etude_cli_support::reason::Code::UnknownOperation);
            etude_cli_support::envelope::status("nothing_to_do");
            ExitCode::from(1)
        }
        Err(error) => {
            eprintln!(
                "stash: {}",
                etude_cli_support::problems::record_open_problem("stash", &id, &error)
            );
            fail(etude_cli_support::reason::Code::JournalUnreadable)
        }
    }
}

/// A live stash in another folder, for `status` to point at.
///
/// A user who stashes one folder and asks for status in another should not get
/// a flat denial of something that just happened.
/// What the clock says about a pop, decided purely so every branch is testable
/// without a keychain, a stash, or the passage of time. The
/// due-pop path needs a proof; making it deterministic means not baking the
/// decision into a real overdue stash (whose journal paths a test cannot
/// backdate) but extracting the arithmetic, exactly as `paths_gate` did for
/// the disclosure gate.
#[derive(Debug, PartialEq, Eq)]
enum ClockDecision {
    /// --if-due, and the deadline has not passed: refuse (exit 2).
    RefuseEarly { human: String },
    /// A plain pop before the deadline: pop, but say it is early.
    NoticeEarly { human: String, due: u64 },
    /// Due, past due, or no deadline at all: just pop.
    Pop,
}

fn pop_clock(deadline: Option<u64>, now: u64, if_due: bool) -> ClockDecision {
    let Some(due) = deadline else {
        return ClockDecision::Pop; // no deadline: nothing to be early against
    };
    if now >= due {
        return ClockDecision::Pop;
    }
    let left = due - now;
    let human = if left >= 86_400 {
        etude_cli_support::counted(left.div_ceil(86_400) as usize, "day", "days")
    } else if left >= 3_600 {
        etude_cli_support::counted(left.div_ceil(3_600) as usize, "hour", "hours")
    } else {
        etude_cli_support::counted(left.div_ceil(60) as usize, "minute", "minutes")
    };
    if if_due {
        ClockDecision::RefuseEarly { human }
    } else {
        ClockDecision::NoticeEarly { human, due }
    }
}

/// Epoch seconds as UTC ISO-8601, no dependencies: days-from-civil in
/// reverse, the standard Howard Hinnant construction.
fn iso_utc(epoch: u64) -> String {
    let days = (epoch / 86400) as i64;
    let secs = epoch % 86400;
    let (h, m, sec) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mth = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mth <= 2 { y + 1 } else { y };
    format!("{y:04}-{mth:02}-{d:02}T{h:02}:{m:02}:{sec:02}Z")
}

fn stash_elsewhere(here: &Path) -> Option<PathBuf> {
    // Quietly: a missing key means status cannot look, which is not worth an
    // error on a read-only command.
    let sl = KeychainSeal {
        key: etude_keep::key().ok()?,
    };
    journal_roots("stash", &sl)
        .into_iter()
        .find(|there| there != here && find_holding(there).is_some())
}

/// What `status` says when this folder is clear. Split out so both wordings are
/// covered by a test without reaching for the keychain.
fn nothing_here(here: &Path, elsewhere: Option<&Path>) -> String {
    match elsewhere {
        None => format!("Nothing stashed in {}.", here.display()),
        Some(there) => format!(
            "Nothing stashed in {}.\n\n  There is a stash in {}.\n  See it with: stash status {}\n  Restore it with: stash pop {}",
            here.display(),
            there.display(),
            there.display(),
            there.display()
        ),
    }
}

/// Epoch seconds to a readable local-ish stamp, without a date dependency.
fn human_time(epoch: u64) -> String {
    let now = now_secs();
    let delta = epoch as i64 - now as i64;
    let abs = delta.unsigned_abs();
    // Round to the nearest unit rather than truncating. A stash made `--for 3d`
    // is already a second old by the time this prints, and "in 2 days" for a
    // three-day hold is the kind of small lie that costs trust in the rest.
    // The boundaries round too, so a one-day hold reads "in 1 day" and never
    // "in 24 hours".
    let (n, unit) = if abs < 3600 - 30 {
        ((abs + 30) / 60, "minute")
    } else if abs < 86_400 - 1800 {
        ((abs + 1800) / 3600, "hour")
    } else {
        ((abs + 43_200) / 86_400, "day")
    };
    let s = if n == 1 { "" } else { "s" };
    if delta >= 0 {
        format!("in {n} {unit}{s}")
    } else {
        format!("{n} {unit}{s} ago")
    }
}

struct KeychainSeal {
    key: [u8; 32],
}

impl etude_core::journal::Sealer for KeychainSeal {
    fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>, &'static str> {
        etude_keep::seal(&self.key, plaintext).map_err(|_| "could not seal the record")
    }
    fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, &'static str> {
        etude_keep::open(&self.key, sealed).map_err(|_| "wrong key or the record was altered")
    }
}

/// Refuses rather than writing an unencrypted record of what was stashed. Only
/// folder stash takes --no-journal, so only folder stash may mention it.
fn sealer(takes_no_journal: bool) -> Option<KeychainSeal> {
    let result = etude_keep::key();
    etude_core::scan::record_read_outcome("key_material", result.is_ok());
    match result {
        Ok(key) => Some(KeychainSeal { key }),
        Err(e) => {
            refusal_reason(etude_cli_support::reason::Code::JournalKeyUnavailable);
            eprintln!(
                "{}",
                etude_cli_support::journal_key_problem(
                    "stash",
                    &e,
                    matches!(e, etude_keep::KeepError::SuppliedKey),
                    takes_no_journal,
                )
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    /// `ETUDE_STATE_DIR` is process-global; serialise tests that replace it.
    fn lock() -> MutexGuard<'static, ()> {
        static L: OnceLock<Mutex<()>> = OnceLock::new();
        L.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn status_id_payload_reports_path_free_durable_recovery_counts() {
        let journal = etude_core::Journal {
            id: "2026-10-12-123456-abcd".into(),
            tool: "stash".into(),
            root: PathBuf::from("/private/example"),
            progress_tail_damaged: false,
            ..Default::default()
        };
        let payload = status_id_payload(&journal);
        // A record with no entries moved nothing: `refused`, not `completed`.
        assert!(payload.contains("\"state\":\"refused\""));
        assert!(payload.contains("\"planned\":0"));
        assert!(payload.contains("\"moved\":0"));
        assert!(payload.contains("\"reversed\":0"));
        assert!(payload.contains("stash pop --id 2026-10-12-123456-abcd"));
        assert!(!payload.contains("/private/example"));
    }

    #[test]
    fn destination_outside_root_remains_distinct_from_a_stale_plan() {
        let outside = etude_core::apply::ApplyError::StalePlan(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "destination must stay inside the plan root; choose a relative group",
        ));
        let stale =
            etude_core::apply::ApplyError::StalePlan(std::io::Error::other("bound plan changed"));
        assert_eq!(
            apply_reason(&outside),
            etude_cli_support::reason::Code::DestinationOutsideRoot
        );
        assert_eq!(
            apply_reason(&stale),
            etude_cli_support::reason::Code::PlanStale
        );
    }

    struct TestSeal;
    impl etude_core::journal::Sealer for TestSeal {
        fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>, &'static str> {
            Ok(plaintext.iter().map(|b| b ^ 0x5a).collect())
        }

        fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, &'static str> {
            Ok(sealed.iter().map(|b| b ^ 0x5a).collect())
        }
    }

    fn stash_plan(root: &Path) -> etude_core::plan::BoundPlan {
        let root = root.canonicalize().expect("root");
        let members = vec![root.join("one.txt"), root.join("two.txt")];
        let out = scan::scan(
            &root,
            &ScanConfig {
                depth: 1,
                allow_sync: true,
                whole_units: true,
                include_tagged: true,
                ..Default::default()
            },
        )
        .expect("fixture scan");
        let proposal = Plan::with_groups(
            &out,
            vec![Group {
                name: ".stash-0".into(),
                signal: Signal::Collected {
                    count: members.len(),
                },
                members,
                accepted: true,
            }],
        );
        etude_core::plan::BoundPlan::from_plan(proposal, test_binding_context())
            .expect("bound fixture")
    }

    fn test_binding_context() -> etude_core::plan::BindingContext {
        etude_core::plan::BindingContext::new(
            "stash",
            env!("CARGO_PKG_VERSION"),
            "fixture-v1",
            "metadata-v1",
        )
    }

    #[test]
    fn durations_parse_the_forms_a_person_actually_types() {
        assert_eq!(parse_duration("30m"), Some(1800));
        assert_eq!(parse_duration("2h"), Some(7200));
        assert_eq!(parse_duration("3d"), Some(259_200));
        assert_eq!(parse_duration("1w"), Some(604_800));
        assert_eq!(parse_duration("3"), None, "bare number accepted");
        assert_eq!(parse_duration("3y"), None, "unknown unit accepted");
        assert_eq!(parse_duration(""), None);
    }

    #[test]
    fn a_huge_duration_does_not_overflow_into_the_past() {
        // A deadline that wrapped would read as permanently overdue.
        assert_eq!(parse_duration("99999999999999999999w"), None);
    }

    #[test]
    fn a_mistyped_leading_flag_is_not_treated_as_consent_to_stash() {
        // `stash --version` used to empty the current directory, because any
        // leading flag meant "stash here". Only these declared flags may lead.
        assert_eq!(
            STASH_FLAGS,
            &[
                "--for",
                "--json",
                "--no-journal",
                "--allow-home",
                "--export-plan",
                "--plan",
                "--plan-digest"
            ]
        );
        for typo in ["--dry-run", "--yes", "-n", "--all", "--force"] {
            assert!(
                !STASH_FLAGS.contains(&typo),
                "{typo} would stash the current directory"
            );
        }
    }

    #[test]
    fn a_path_after_leading_flags_is_not_silently_discarded() {
        let args = |parts: &[&str]| parts.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
        assert_eq!(
            positional_path(&args(&["--for", "3d", "/tmp/there"])),
            Some("/tmp/there")
        );
        assert_eq!(
            positional_path(&args(&["--json", "/tmp/there"])),
            Some("/tmp/there")
        );
        assert_eq!(
            positional_path(&args(&["--for", "3d", "--json", "/tmp/there"])),
            Some("/tmp/there")
        );
        assert_eq!(
            positional_path(&args(&["--json", "--for", "3d", "/tmp/there"])),
            Some("/tmp/there")
        );
        assert_eq!(positional_path(&args(&["--json"])), None);
        assert_eq!(positional_path(&args(&["--for", "3d"])), None);
    }

    #[test]
    fn status_and_pop_agree_when_this_folders_stash_is_not_newest() {
        let _g = lock();
        let base = std::env::temp_dir().join(format!(
            "stash_select_{}_{}",
            std::process::id(),
            now_secs()
        ));
        let first = base.join("first");
        let second = base.join("second");
        let state = base.join("state");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&first).expect("first folder");
        std::fs::create_dir_all(&second).expect("second folder");
        for root in [&first, &second] {
            std::fs::write(root.join("one.txt"), b"one").expect("first file");
            std::fs::write(root.join("two.txt"), b"two").expect("second file");
        }
        unsafe { std::env::set_var("ETUDE_STATE_DIR", &state) };

        etude_core::apply::apply(
            &stash_plan(&first),
            &test_binding_context(),
            Some(&TestSeal),
            None,
        )
        .expect("first stash");
        etude_core::apply::apply(
            &stash_plan(&second),
            &test_binding_context(),
            Some(&TestSeal),
            None,
        )
        .expect("second stash");

        let target = first.canonicalize().expect("first canonical path");
        let selected = journal_for_root("stash", &TestSeal, &target)
            .0
            .expect("the older journal remains selectable");
        assert_eq!(selected.root.canonicalize().unwrap(), target);
        assert!(
            find_holding(&first).is_some(),
            "status could not see the stash pop selected"
        );

        let _ = std::fs::remove_dir_all(&base);
        unsafe { std::env::remove_var("ETUDE_STATE_DIR") };
    }

    #[test]
    fn an_already_restored_stash_is_not_selected_or_reported_as_live() {
        let _g = lock();
        let base = std::env::temp_dir().join(format!(
            "stash_restored_{}_{}",
            std::process::id(),
            now_secs()
        ));
        let root = base.join("folder");
        let state = base.join("state");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&root).expect("folder");
        std::fs::write(root.join("one.txt"), b"one").expect("first file");
        std::fs::write(root.join("two.txt"), b"two").expect("second file");
        unsafe { std::env::set_var("ETUDE_STATE_DIR", &state) };

        etude_core::apply::apply(
            &stash_plan(&root),
            &test_binding_context(),
            Some(&TestSeal),
            None,
        )
        .expect("stash");
        let target = root.canonicalize().expect("canonical path");
        let mut journal = journal_for_root("stash", &TestSeal, &target)
            .0
            .expect("live journal");
        let r = etude_core::apply::undo(&mut journal, &TestSeal);
        assert!(r.error.is_none(), "unexpected undo error: {:?}", r.error);
        journal.save_sealed(&TestSeal).expect("resave journal");

        assert!(journal.path().is_file(), "restore removed the journal");
        assert_eq!(find_holding(&root), None, "holding directory survived pop");
        assert!(
            journal_for_root("stash", &TestSeal, &target).0.is_none(),
            "restored journal remained selectable"
        );
        assert!(
            !journal_roots("stash", &TestSeal).contains(&target),
            "restored root was still reported as live"
        );

        let _ = std::fs::remove_dir_all(&base);
        unsafe { std::env::remove_var("ETUDE_STATE_DIR") };
    }

    #[test]
    fn status_points_at_a_stash_in_another_folder_instead_of_denying_it() {
        // The bug: stash a folder, ask for status somewhere else, and status
        // said "Nothing stashed". That is a denial of something `stash pop`
        // would happily restore.
        let here = Path::new("/tmp/here");
        let plain = nothing_here(here, None);
        assert_eq!(plain, "Nothing stashed in /tmp/here.");
        assert!(
            !plain.contains("stash pop"),
            "offered pop with nothing to pop"
        );

        let told = nothing_here(here, Some(Path::new("/tmp/there")));
        assert!(told.starts_with("Nothing stashed in /tmp/here."));
        assert!(
            told.contains("/tmp/there"),
            "did not say where the stash is"
        );
        assert!(told.contains("stash pop"), "did not say how to get it back");
    }

    #[test]
    fn a_holding_directory_is_recognised_and_an_ordinary_one_is_not() {
        // find_holding is what decides whether a folder counts as stashed, in
        // the current directory and in the one status now points at.
        let root = std::env::temp_dir().join(format!("stash-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Screenshots")).unwrap();
        assert_eq!(
            find_holding(&root),
            None,
            "an ordinary folder read as a stash"
        );

        std::fs::create_dir_all(root.join(holding_name(Some(1_800_000_000)))).unwrap();
        assert_eq!(
            find_holding(&root)
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())),
            Some(".stash-1800000000".to_string())
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_deadline_round_trips_through_the_directory_name() {
        // The deadline lives in the name, so this IS the storage layer.
        let name = holding_name(Some(1_800_000_000));
        assert_eq!(deadline_of(&name), Some(1_800_000_000));
        assert_eq!(
            deadline_of(&holding_name(None)),
            None,
            "0 should read as no deadline"
        );
        assert_eq!(
            deadline_of("Screenshots"),
            None,
            "an ordinary folder read as a stash"
        );
    }

    #[test]
    fn apply_exit_code_maps_refusals_and_failures() {
        let collision = etude_core::apply::ApplyError::DestinationCollision(PathBuf::from("x"));
        let io = etude_core::apply::ApplyError::Io(std::io::Error::other("x"));
        assert_eq!(apply_exit_code(&collision), ExitCode::from(2));
        assert_eq!(apply_exit_code(&io), ExitCode::from(3));
    }

    fn sample_entry(moved: bool) -> etude_core::journal::Entry {
        etude_core::journal::Entry {
            from: PathBuf::from("/tmp/a"),
            to: PathBuf::from("/tmp/b"),
            method: etude_core::journal::Method::Rename,
            size: 1,
            mtime_secs: 0,
            inode: 0,
            edge_hash: 0,
            state: if moved {
                etude_core::journal::EntryState::Moved
            } else {
                etude_core::journal::EntryState::Planned
            },
            tag: None,
        }
    }

    #[test]
    fn journal_is_fully_undone_when_no_entry_is_done() {
        let undone = etude_core::Journal {
            id: "t".into(),
            tool: "stash".into(),
            root: PathBuf::from("/tmp"),
            entries: vec![sample_entry(false), sample_entry(false)],
            created_directories: Vec::new(),
            validated_directories: Vec::new(),
            progress_tail_damaged: false,
        };
        assert!(journal_is_fully_undone(&undone));

        let pending = etude_core::Journal {
            id: "t".into(),
            tool: "stash".into(),
            root: PathBuf::from("/tmp"),
            entries: vec![sample_entry(false), sample_entry(true)],
            created_directories: Vec::new(),
            validated_directories: Vec::new(),
            progress_tail_damaged: false,
        };
        assert!(!journal_is_fully_undone(&pending));
    }

    /// Every flag a command reads is in the help, and every misspelling is
    /// refused. sweep grew this guard after --map shipped undocumented; stash
    /// gets it the same week --if-due shipped as an automation contract --
    /// a contract a tool cannot offer while accepting typos of it.
    #[test]
    fn every_stash_flag_is_documented_and_typos_refuse() {
        for (_, flags) in COMMAND_FLAGS {
            for f in *flags {
                assert!(
                    USAGE.contains(f),
                    "{f} is accepted but undocumented in `stash help`"
                );
            }
        }
        assert!(check_flags("pop", &["pop".into(), "--ifdue".into()]).is_err());
        assert!(check_flags("status", &["status".into(), "--al".into()]).is_err());
        assert!(check_flags("pop", &["pop".into(), "--if-due".into()]).is_ok());
        assert!(
            check_flags(
                "status",
                &["status".into(), "--all".into(), "--paths".into()]
            )
            .is_ok()
        );
    }

    /// The ISO renderer against known fixed points, including a leap year.
    #[test]
    fn iso_rendering_matches_known_dates() {
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_utc(951_827_696), "2000-02-29T12:34:56Z");
        assert_eq!(iso_utc(1_787_519_993), "2026-08-23T21:19:53Z");
        // day rollover, both sides of midnight
        assert_eq!(iso_utc(86_399), "1970-01-01T23:59:59Z");
        assert_eq!(iso_utc(86_400), "1970-01-02T00:00:00Z");
        // a non-leap century (2100 is not a leap year) and a far-future date
        assert_eq!(iso_utc(4_107_542_400), "2100-03-01T00:00:00Z");
        assert_eq!(iso_utc(4_107_456_000), "2100-02-28T00:00:00Z");
    }

    /// Every clock branch, without a stash, a key, or a real clock.
    #[test]
    fn pop_clock_covers_every_branch() {
        // no deadline: always pops
        assert_eq!(pop_clock(None, 100, true), ClockDecision::Pop);
        assert_eq!(pop_clock(None, 100, false), ClockDecision::Pop);
        // exactly due, and past due: pops, --if-due or not
        assert_eq!(pop_clock(Some(100), 100, true), ClockDecision::Pop);
        assert_eq!(pop_clock(Some(100), 200, true), ClockDecision::Pop);
        assert_eq!(pop_clock(Some(100), 200, false), ClockDecision::Pop);
        // early + --if-due: refused
        assert!(matches!(
            pop_clock(Some(1000), 100, true),
            ClockDecision::RefuseEarly { .. }
        ));
        // early + plain: notice, still pops
        assert!(matches!(
            pop_clock(Some(1000), 100, false),
            ClockDecision::NoticeEarly { due: 1000, .. }
        ));
        // the one-second-early boundary rounds up to a minute, never "0"
        if let ClockDecision::RefuseEarly { human } = pop_clock(Some(100), 99, true) {
            assert_eq!(human, "1 minute");
        } else {
            panic!("one second early was not refused");
        }
    }

    /// The disclosure gate answers on the tty alone, before any keychain.
    ///
    /// It once ran after sealer(), so a caller with no key got the
    /// wrong refusal. This drives both branches with an explicit tty and needs
    /// no keychain -- the regression the ordering bug demanded.
    #[test]
    fn the_paths_gate_refuses_a_non_terminal_and_needs_no_keychain() {
        assert!(
            paths_gate(true, false).is_err(),
            "--paths from a pipe was allowed"
        );
        let msg = paths_gate(true, false).unwrap_err();
        assert!(
            msg.contains("person at a terminal"),
            "the refusal lost its reason"
        );
        assert!(
            paths_gate(true, true).is_ok(),
            "--paths at a terminal was refused"
        );
        assert!(
            paths_gate(false, false).is_ok(),
            "redacted --all from a pipe was refused"
        );
    }

    #[test]
    fn the_folder_is_reported_clear_only_when_nothing_was_left_behind() {
        use etude_core::apply::ApplyReport;
        let moved = ApplyReport {
            moved: 4,
            ..Default::default()
        };
        assert!(folder_is_clear(&moved));
        for left_behind in [
            ApplyReport {
                moved: 3,
                held_cross_volume: 1,
                ..Default::default()
            },
            ApplyReport {
                moved: 3,
                held_tagged: 1,
                ..Default::default()
            },
            ApplyReport {
                moved: 4,
                tagged_not_returned: 1,
                ..Default::default()
            },
            ApplyReport {
                moved: 3,
                hold_record_failed: 1,
                ..Default::default()
            },
        ] {
            assert!(!folder_is_clear(&left_behind), "{left_behind:?}");
        }
    }
}
