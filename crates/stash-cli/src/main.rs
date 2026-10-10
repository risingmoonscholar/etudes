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
    stash select PATH...           stash exactly the chosen objects; folders move whole
    stash select --from0 FILE|-     read NUL-terminated UTF-8 paths; '-' reads stdin
    --into PARENT                   holding parent for an explicit selection
    stash pop --id ID               restore exactly one operation
    stash pop ID                    restore an operation id directly
    stash pop [PATH]                restore the unique batch for PATH (or here)
    stash pop [PATH] --latest       restore the latest live batch; no PATH means all
    stash pop [PATH] --if-due       pop only if the deadline has passed;
                                    exit 2 when early, 1 when nothing stashed
    stash status [PATH]             what is stashed, and when it is due back
    stash status --all              every stash this machine's journals know,
                                    paths redacted; --paths shows them
    stash contract --json           print the versioned capability contract
    --no-journal                    legacy folder mode only; explicit selections require undo
    --json                          machine-readable output (for agents)
    --export-plan FILE               save a private bound plan, without moving
    --plan FILE --plan-digest SHA     apply the exact exported observation
    --version                       print the version and exit
    stash help

DURATION
    30m  2h  3d  1w        default: no deadline, restore whenever

KEY
    ETUDE_JOURNAL_KEY  64 hex digits (32 random bytes), overrides the keychain.
                       Retain the same key for undo; see README for setup.

stash moves everything sweep can see, including the files sweep would refuse
to organise. Hidden items are left in place, and it says how many.
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
    let code = std::panic::catch_unwind(run_main).unwrap_or(ExitCode::from(3));
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
                ExitCode::from(2)
            }
        },
        Some("status" | "list") => match check_flags("status", &args) {
            Ok(()) => cmd_status(&args),
            Err(m) => {
                eprintln!("stash: {m}");
                ExitCode::from(2)
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
                cmd_stash(&path, &args)
            } else {
                eprintln!("stash: unknown option {p}. Run `stash help`.");
                ExitCode::from(2)
            }
        }
        Some(p) => cmd_stash(&PathBuf::from(expand_tilde(p)), &args),
    }
}

/// Flags that may lead the argument list. Anything else there is a typo, and a
/// typo must not stash the current directory.
const STASH_FLAGS: &[&str] = &[
    "--for",
    "--json",
    "--no-journal",
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
            "--export-plan",
            "--plan",
            "--plan-digest",
        ],
    ),
    ("pop", &["--id", "--latest", "--if-due", "--json"]),
    ("status", &["--json", "--all", "--paths"]),
];

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
    while i < args.len() {
        let a = &args[i];
        if matches!(
            a.as_str(),
            "--for" | "--export-plan" | "--plan" | "--plan-digest" | "--id"
        ) && allowed.contains(&a.as_str())
        {
            if args.get(i + 1).is_none_or(|value| value.starts_with('-')) {
                return Err(format!("{a} needs a value"));
            }
            i += 2;
            continue;
        }
        if a.starts_with('-') && !allowed.contains(&a.as_str()) {
            return Err(format!(
                "unknown option {a} for this command. Run `stash help`."
            ));
        }
        i += 1;
    }
    Ok(())
}

fn expand_tilde(p: &str) -> String {
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

fn replay_stash(args: &[String], file: &str) -> ExitCode {
    if positional_path(args).is_some() {
        eprintln!("stash: exported plan fixes its original root; do not supply another path");
        return ExitCode::from(2);
    }
    if flag(args, "--for") || flag(args, "--export-plan") {
        eprintln!(
            "stash: exported plan fixes its holding deadline; replan instead of overriding it"
        );
        return ExitCode::from(2);
    }
    let Some(digest) = value(args, "--plan-digest") else {
        eprintln!("stash: --plan requires the digest printed with the exported plan");
        return ExitCode::from(2);
    };
    let bound = match etude_core::plan::BoundPlan::load(Path::new(file), &digest) {
        Ok(plan) => plan,
        Err(error) => {
            eprintln!("stash: {error}; replan required");
            return ExitCode::from(2);
        }
    };
    if !bound.context().profile.starts_with("stash-metadata-v1;")
        || bound.groups.len() != 1
        || !bound.groups[0].name.starts_with(".stash-")
    {
        eprintln!("stash: plan scheme changed; replan required");
        return ExitCode::from(2);
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
        return ExitCode::from(2);
    }
    let deadline = match value(args, "--for") {
        Some(d) => match parse_duration(&d) {
            Some(secs) => Some(now_secs() + secs),
            None => {
                eprintln!("stash: cannot read duration {d:?}. Try 30m, 2h, 3d or 1w.");
                return ExitCode::from(2);
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
        ..Default::default()
    };
    let outcome = match scan::scan(path, &cfg) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("stash: {e}");
            return if e.is_refusal() {
                ExitCode::from(2)
            } else {
                ExitCode::from(3)
            };
        }
    };
    if outcome.entries.is_empty() {
        println!("Nothing to stash. {} is already clear.", path.display());
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
            eprintln!("stash: {error}; replan required");
            return ExitCode::from(2);
        }
    };
    if let Some(file) = value(args, "--export-plan") {
        if let Err(error) = bound.export(Path::new(&file)) {
            eprintln!("stash: cannot export plan ({error})");
            return ExitCode::from(2);
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
        eprintln!("stash: {error}; replan required");
        return ExitCode::from(2);
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
        let Some(sl) = sealer() else {
            return ExitCode::from(2);
        };
        Some(sl)
    };
    let mut progress = etude_cli_support::ProgressReporter::stderr("stash", count);
    let result = etude_core::apply::apply_with_progress(
        &bound,
        &context,
        sl.as_ref().map(|s| s as &dyn etude_core::journal::Sealer),
        None,
        |p| progress.update(p.completed, p.total),
    );
    drop(progress);
    match result {
        Ok(r) => {
            etude_cli_support::envelope::effect("items_moved", r.moved);
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
                        ("holding", j::str(&bound.groups[0].name)),
                        ("binding", bound.binding_json()),
                        ("due", deadline.map(j::num).unwrap_or_else(|| "null".into())),
                        ("skipped_hidden", j::num(bound.skipped_hidden)),
                        ("skipped_unreadable", j::num(bound.skipped_unreadable)),
                    ])
                );
                return ExitCode::SUCCESS;
            }
            println!("\nStashed {} items.", r.moved);
            println!("The selected folder is clear.\n");
            if sl.is_some() {
                println!(
                    "  Operation id: {}. Restore with: stash pop --id {}",
                    r.journal_id, r.journal_id
                );
                match deadline {
                    Some(t) => {
                        println!("  Due back: {}", human_time(t));
                        println!("  stash does not run in the background. Run `stash pop`,");
                        println!("  or `stash status` to see what is overdue.");
                    }
                    None => println!("  No deadline. Restore with: stash pop"),
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
            ExitCode::SUCCESS
        }
        Err(e) => {
            if matches!(e, etude_core::apply::ApplyError::StalePlan(_)) {
                eprintln!("stash: {e}");
                etude_cli_support::envelope::status("refused");
                return ExitCode::from(2);
            }
            etude_cli_support::envelope::status("incomplete");
            eprintln!(
                "stash: operation could not finish ({})",
                apply_error_reason(&e)
            );
            if sl.is_some() {
                eprintln!("Nothing further was moved. `stash pop` reverses what did happen.");
            } else {
                eprintln!(
                    "Nothing further was moved. --no-journal removed undo; restore files manually."
                );
            }
            apply_exit_code(&e)
        }
    }
}

fn cmd_select(args: &[String]) -> ExitCode {
    let options = match selection::parse(args) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("stash: {}", error.message());
            return ExitCode::from(error.code());
        }
    };
    let deadline = match options.duration.as_deref() {
        Some(value) => {
            match parse_duration(value).and_then(|seconds| now_secs().checked_add(seconds)) {
                Some(deadline) => Some(deadline),
                None => {
                    eprintln!("stash: invalid selection duration");
                    return ExitCode::from(2);
                }
            }
        }
        None => None,
    };
    let prepared = match selection::prepare(&options) {
        Ok(prepared) => prepared,
        Err(error) => {
            eprintln!("stash: {}", error.message());
            return ExitCode::from(error.code());
        }
    };
    if prepared.sources.is_empty() {
        println!("Nothing selected to stash.");
        return ExitCode::from(1);
    }
    let Some(sealer) = sealer() else {
        return ExitCode::from(2);
    };
    let root = match selection::reserve(&prepared.parent) {
        Ok(root) => root,
        Err(error) => {
            eprintln!("stash: {}", error.message());
            return ExitCode::from(error.code());
        }
    };
    let plan = selection::plan(&prepared, root.clone(), &holding_name(deadline));
    let mut progress = etude_cli_support::ProgressReporter::stderr("stash", prepared.sources.len());
    let result = etude_core::apply::apply_with_structured_progress(
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
            etude_cli_support::envelope::effect("items_moved", report.moved);
            if options.json {
                use etude_core::json as j;
                println!(
                    "{}",
                    j::obj(&[
                        ("action", j::str("stash_selected")),
                        ("stash_id", j::str(&report.journal_id)),
                        ("moved", j::num(report.moved)),
                        ("selected", j::num(prepared.sources.len())),
                        ("holding_root", j::path(&root)),
                        ("due", deadline.map(j::num).unwrap_or_else(|| "null".into()))
                    ])
                );
            } else {
                println!(
                    "Stashed {} selected {}. Restore with: stash pop {}",
                    report.moved,
                    if report.moved == 1 {
                        "object"
                    } else {
                        "objects"
                    },
                    etude_core::redact::path(&root)
                );
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            etude_cli_support::envelope::status("incomplete");
            eprintln!(
                "stash: selection could not finish; inspect its holding locator before retrying."
            );
            eprintln!(
                "Recovery: stash status --all --json lists operation ids for stash pop --id ID."
            );
            apply_exit_code(&error)
        }
    }
}

/// Destination* variants are safety refusals (2); Io/Journal/Injected are failures (3).
fn apply_error_reason(error: &etude_core::apply::ApplyError) -> &'static str {
    use etude_core::apply::ApplyError::*;
    match error {
        DestinationExists(_) => "destination exists",
        DestinationCollision(_) => "destination names collide",
        DestinationIsSynced(_) => "destination is synced",
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
        | CannotCompareNames(_) => ExitCode::from(2),
        Io(_) | Journal(_) | Injected(_) => ExitCode::from(3),
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
            eprintln!("{tool}: journal {id} is unreadable or damaged: {e}");
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

fn cmd_pop(args: &[String]) -> ExitCode {
    let selector = match batches::selector(args) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("stash: {error}");
            return ExitCode::from(2);
        }
    };
    let Some(sl) = sealer() else {
        return ExitCode::from(2);
    };
    let mut j = match batches::select(selector, &sl) {
        Ok(Some(journal)) => journal,
        Ok(None) => {
            eprintln!("stash: no stash found for this id or folder; nothing is stashed here");
            return ExitCode::from(1);
        }
        Err(error) => {
            if matches!(&error, etude_core::journal::JournalError::Io(io) if io.kind() == std::io::ErrorKind::PermissionDenied)
                || matches!(&error, etude_core::journal::JournalError::Seal(_))
            {
                eprintln!(
                    "stash: journal is unreadable with the current permissions or key; cannot restore: {error}"
                );
            } else {
                eprintln!("stash: cannot restore: {error}");
            }
            return ExitCode::from(3);
        }
    };
    let due = match batches::due(&j) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("stash: {error}");
            return ExitCode::from(3);
        }
    };
    match pop_clock(due, now_secs(), flag(args, "--if-due")) {
        ClockDecision::RefuseEarly { human } => {
            eprintln!("stash: not due for {human}. Refusing --if-due.");
            return ExitCode::from(2);
        }
        ClockDecision::NoticeEarly { human, due } => {
            println!("due {}; popping {human} early.", iso_utc(due))
        }
        ClockDecision::Pop => {}
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
    if j.progress_tail_damaged {
        eprintln!(
            "stash: this journal is damaged: a progress record was truncated, so the\n\
             \x20      last move it began is not written down. Everything it did record is\n\
             \x20      being restored, and the item whose record was lost is recovered by\n\
             \x20      checking the filesystem rather than the journal."
        );
    }

    let mut progress = etude_cli_support::ProgressReporter::stderr("stash pop", j.entries.len());
    let r = etude_core::apply::undo_with_progress(&mut j, &sl, |p| {
        progress.update(p.completed, p.total)
    });
    drop(progress);
    if r.unrecorded_moves > 1 {
        eprintln!(
            "stash: refused. This journal is missing more than one record: {n} items are\n\
             at their destinations while the journal says they were never moved.\n\n\
             A crash between a move and its record loses exactly one record, and that\n\
             one is recoverable. Losing several means the journal itself was damaged,\n\
             and pop can only reach the first of them -- so it would put a few back,\n\
             leave the rest where they are, and report success. Nothing has been\n\
             touched instead.\n\n\
             The items are still at their destinations. Nothing is lost.",
            n = r.unrecorded_moves
        );
        return ExitCode::from(3);
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
            "skipped_missing",
            etude_core::json::num(r.skipped_missing.len()),
        ),
    ]));
    if !r.skipped_changed.is_empty() || r.error.is_some() {
        etude_cli_support::envelope::status("incomplete");
    }
    etude_cli_support::envelope::effect("items_restored", r.restored);
    etude_cli_support::envelope::effect("changed_items_left", r.skipped_changed.len());
    etude_cli_support::envelope::effect("missing_items", r.skipped_missing.len());
    println!("\nRestored {} items.", r.restored);
    if !r.skipped_changed.is_empty() {
        println!(
            "  {} changed while stashed and were left alone:",
            r.skipped_changed.len()
        );
    }
    if !r.skipped_missing.is_empty() {
        println!("  {} were already gone.", r.skipped_missing.len());
    }
    if r.already_reversed > 0 {
        println!(
            "  {} were already restored by an earlier run that did not finish.",
            r.already_reversed
        );
    }
    if !r.reconciled.is_empty() {
        // Different from healed: nothing was reachable by two names here. An
        // earlier undo moved these home and was killed before it could write
        // that down, so this run only had to agree with the disk.
        println!(
            "  {} were already home from an interrupted restore; the journal now says so.",
            r.reconciled.len()
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
    if r.error.is_some() {
        eprintln!(
            "stash: restore could not finish; operation {} remains incomplete",
            j.id
        );
        match saved {
            Ok(()) => {
                eprintln!(
                    "The journal is resumable. `stash pop` will pick up where this left off."
                );
            }
            Err(save_err) => eprintln!(
                "stash: additionally, the journal could not be updated ({save_err}). It may not reflect the items just restored."
            ),
        }
        return ExitCode::from(3);
    }
    if let Err(save_err) = saved {
        eprintln!("stash: pop finished, but the journal could not be saved: {save_err}");
        return ExitCode::from(3);
    }
    if r.skipped_changed.is_empty() {
        selection::remove_empty_root(&j);
    }
    ExitCode::SUCCESS
}

/// The --paths disclosure gate, before ANY other work. Split out and called
/// first so it is the deterministic refusal for a non-interactive --paths
/// call -- Codex found it running after sealer(), so a caller with no
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
        return ExitCode::from(2);
    }
    let Some(sl) = sealer() else {
        return ExitCode::from(2);
    };
    let stashes = match batches::inventory(&sl) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("stash: could not inspect batches: {error}");
            return ExitCode::from(3);
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
                eprintln!("stash: could not inspect batches: {error}");
                return ExitCode::from(3);
            }
        };
        println!(
            "{}",
            j::obj(&[
                ("stashes", j::arr(rows)),
                ("paths_shown", j::bool(show_paths))
            ])
        );
    } else if stashes.is_empty() {
        println!("Nothing is stashed.");
    } else {
        println!("{} live operations:", stashes.len());
        for batch in &stashes {
            let due = match batches::due(batch) {
                Ok(value) => value,
                Err(error) => {
                    eprintln!("stash: {error}");
                    return ExitCode::from(3);
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
        }
        if !show_paths {
            println!("  paths are redacted; `stash status --all --paths` shows them.");
        }
    }
    if stashes.is_empty() {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn cmd_status(args: &[String]) -> ExitCode {
    if flag(args, "--all") {
        return cmd_status_all(args);
    }
    let root = args
        .iter()
        .skip(1)
        .find(|a| !a.starts_with('-'))
        .map(|p| PathBuf::from(expand_tilde(p)))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    let Ok(root) = root.canonicalize() else {
        eprintln!("stash: cannot read that folder");
        return ExitCode::from(2);
    };

    if let Ok(key) = etude_keep::key() {
        let sealer = KeychainSeal { key };
        let batches = match batches::inventory(&sealer) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("stash: could not inspect batches: {error}");
                return ExitCode::from(3);
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
                    eprintln!("stash: {error}");
                    return ExitCode::from(3);
                }
            };
            let counts = match batches
                .iter()
                .map(batches::held)
                .collect::<Result<Vec<_>, _>>()
            {
                Ok(value) => value,
                Err(error) => {
                    eprintln!("stash: {error}");
                    return ExitCode::from(3);
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
                    "{} items in {} operations stashed from {}.",
                    counts.iter().sum::<usize>(),
                    batches.len(),
                    "the selected folder"
                );
                for batch in &batches {
                    println!("  {}: restore with stash pop --id {}", batch.id, batch.id);
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
            println!("\n{n} items stashed from {}.", root.display());
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

/// A live stash in another folder, for `status` to point at.
///
/// A user who stashes one folder and asks for status in another should not get
/// a flat denial of something that just happened.
/// What the clock says about a pop, decided purely so every branch is testable
/// without a keychain, a stash, or the passage of time. Codex asked for the
/// due-pop path to be proven; making it deterministic meant not baking the
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
        format!("{} day(s)", left.div_ceil(86_400))
    } else if left >= 3_600 {
        format!("{} hour(s)", left.div_ceil(3_600))
    } else {
        format!("{} minute(s)", left.div_ceil(60))
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

/// Refuses rather than writing an unencrypted record of what was stashed.
fn sealer() -> Option<KeychainSeal> {
    let result = etude_keep::key();
    etude_core::scan::record_read_outcome("key_material", result.is_ok());
    match result {
        Ok(key) => Some(KeychainSeal { key }),
        Err(e) => {
            eprintln!("stash: {e}");
            eprintln!(
                "Refusing to record a stash in the clear. The only alternative is --no-journal; it removes undo."
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
        }
    }

    #[test]
    fn journal_is_fully_undone_when_no_entry_is_done() {
        let undone = etude_core::Journal {
            id: "t".into(),
            tool: "stash".into(),
            root: PathBuf::from("/tmp"),
            entries: vec![sample_entry(false), sample_entry(false)],
            progress_tail_damaged: false,
        };
        assert!(journal_is_fully_undone(&undone));

        let pending = etude_core::Journal {
            id: "t".into(),
            tool: "stash".into(),
            root: PathBuf::from("/tmp"),
            entries: vec![sample_entry(false), sample_entry(true)],
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
            assert_eq!(human, "1 minute(s)");
        } else {
            panic!("one second early was not refused");
        }
    }

    /// The disclosure gate answers on the tty alone, before any keychain.
    ///
    /// Codex found it running after sealer(), so a caller with no key got the
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
}
