//! sweep: organise the obvious, leave the private alone.
//!
//! `sweep PATH` plans and changes nothing. `apply` executes, `undo` reverses.
//! The undo journal is sealed with a key held in the login keychain; if sealing
//! is unavailable sweep refuses rather than writing plaintext.

macro_rules! println {
    () => { etude_cli_support::envelope::print(String::new()) };
    ($($arg:tt)*) => { etude_cli_support::envelope::print(format!($($arg)*)) };
}

macro_rules! print {
    ($($arg:tt)*) => { etude_cli_support::envelope::prompt(format!($($arg)*)) };
}

mod existing_profile;
mod inspect;
mod review;
mod tag_output;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use etude_core::plan;
use etude_core::scan::{self, ScanConfig};

const USAGE: &str = "\
sweep: organise the obvious, leave the private alone

USAGE
    sweep [PATH] [FLAGS]         analyse and build a plan; changes nothing
    sweep review PATH            walk each group, rename or skip, then apply
    sweep apply PATH --yes       move every proposed group
    sweep PATH --scheme tags --map EXT=TAG --export-plan F
                                 plan Finder tags to add; nothing is moved
    sweep apply PATH --only NAME move one group
    sweep propose --plan F --plan-digest SHA --proposal P --export-plan OUT
                                 check per-file decisions against an exported
                                 plan and write a new plan; moves nothing
    sweep undo [PATH]            reverse the most recent apply, or that folder's
    sweep undo --id ID           reverse exactly one apply, opening only its record
    sweep status --id ID --json  report one encrypted journal without names
    sweep status PATH --json     report that folder's journals, newest first
    sweep forget                 remove sweep's journals; ask before destroying
                                 a key stash also relies on
    sweep verify                 print sweep's own privacy posture
    sweep contract --json        print the versioned capability contract
    sweep lesson [N]             seven exercises against a folder you throw away

FLAGS
    --map EXT=Folder  route an unknown extension into a folder, this run only
                      (repeatable; refusals still win; journal required)
    --save-routes    apply: remember --map routes after all moves succeed
                      (existing direct folders only; journal required).
                      Undo moves files back but keeps saved routes
    --scheme folders  group by file type and other built-in folder rules (default).
                      A new folder needs 3 or more files; a folder of that
                      name already here takes any number
    --scheme existing use the saved, versioned existing-folder profile. A
                      route moves any number of files; a missing route
                      folder is created only when files move
    --scheme tags     add Finder tags and move nothing (macOS); needs --map
    --depth N       recursion depth (default 1, max 8)
    --since N[h|d]  leave files changed in the last N alone (default 1d, 0 off)
    --map EXT=TAG     with --scheme tags: add the tag TAG to files with that
                      extension. Tags a file already has stay. Sweep never
                      makes up a tag; the plan marks each new one
    --json          machine-readable results on stdout (for agents)
    --export-plan F save a private bound plan outside the selected tree
    apply --plan F --plan-digest SHA --yes applies that exact observed plan
                      (the only way tags are added)
    --proposal P    propose: a JSON file of per-file folder decisions
    --progress-json bounded versioned progress on stderr for apply, review or undo
    --quiet         counts and signals only; never prints a filename
    --explain       print the signal trace for every file
    --allow-sync    proceed even inside a cloud-synced folder
    --include-tagged  LOUD: also organise Finder-tagged items (default: left
                      alone and counted). Tags stay on the item; a tagged item
                      that would cross volumes is left in place
    --no-journal    apply without recording undo
    --version       print the version and exit
    --inspect-content  read text file contents to refuse MORE files
                       (asks first; never affects where anything moves)

KEY
    ETUDE_JOURNAL_KEY  64 hex digits (32 random bytes), overrides the keychain.
                       Retain the same key for undo; see README for setup.

Only `apply` changes anything: it moves files, or with --scheme tags adds
tags and moves nothing. Files that look like personal records are never
moved or tagged, in any mode.

Sweep decides from filesystem and Finder organization metadata only; it never
reads file contents to decide. When it moves a file it reads the first and last
4 KiB, so undo can tell if that file changed. With --scheme tags it reads each
file's tag list, never its contents, and adds to the list.";

fn main() -> ExitCode {
    let args = match etude_cli_support::args::collect("sweep", env!("CARGO_PKG_VERSION")) {
        Ok(args) => args,
        Err(code) => return code,
    };
    etude_cli_support::envelope::begin("sweep", env!("CARGO_PKG_VERSION"), &args);
    let code = std::panic::catch_unwind(|| run_main(args)).unwrap_or_else(|_| {
        set_reason(etude_cli_support::reason::Code::InternalError, None);
        ExitCode::from(3)
    });
    etude_cli_support::envelope::finish(code);
    code
}

fn run_main(args: Vec<String>) -> ExitCode {
    if args.first().is_some_and(|arg| arg == "contract") {
        return etude_cli_support::contract::command(
            etude_cli_support::contract::Tool::Sweep,
            env!("CARGO_PKG_VERSION"),
            &args,
            etude_read::scan::TEXT_EXTS,
            Some(etude_read::buf::MAX_READ),
        );
    }

    if etude_cli_support::wants_help(&args) {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }

    // Status is a strictly read-only journal query. It must run before legacy
    // migration and expiry pruning, both of which can modify journal state.
    if args.first().is_some_and(|arg| arg == "status") {
        if let Err(msg) = check_flags("status", &args) {
            eprintln!("sweep: {msg}");
            let code = if msg.contains("unknown option") {
                etude_cli_support::reason::Code::UnknownOption
            } else {
                etude_cli_support::reason::Code::InvalidArguments
            };
            return refusal(code, None);
        }
        if let Err(msg) = check_positionals("status", &args) {
            eprintln!("sweep: {msg}");
            return refusal(etude_cli_support::reason::Code::InvalidArguments, None);
        }
        disable_core_dumps();
        return cmd_status(&args);
    }

    // Core dumps can carry filenames from the heap into a file the OS may offer
    // to upload. Close that before doing any work. THREAT-MODEL § T5.
    disable_core_dumps();

    // Every command's arguments are checked here, once, before the command runs
    // and before startup touches any state, so a refusal changes nothing.
    // Doing it per-command is what let three of them ship with no checking at
    // all, and `sweep forget --frobnicate` destroy a journal on a typo.
    // Which command was named, or "" for a bare scan. Derived from the table
    // rather than listed again here: a second list of subcommands is the kind
    // of duplicate that goes stale, and clippy's suggestion to simplify this
    // to unwrap_or_default() would put a PATH in `cmd`, find no entry, and
    // silently check nothing.
    //
    // A command dispatched but missing from the table lands on "" and is
    // checked against the SCAN flags, which is neither open nor closed: it
    // would allow --json and refuse a flag that command actually needed. That
    // is a wrong answer rather than a safe one, so the real defence is the
    // test every_dispatched_command_declares_its_flags, which stops such a
    // command from shipping at all. An earlier version of this comment
    // claimed it failed closed; a review pointed out that it does not.
    let first = args.first().map(String::as_str).unwrap_or_default();
    let cmd = if COMMAND_FLAGS
        .iter()
        .any(|(c, _)| !c.is_empty() && *c == first)
    {
        first
    } else {
        ""
    };
    if let Err(msg) = check_flags(cmd, &args) {
        eprintln!("sweep: {msg}");
        let code = if msg.contains("unknown option") {
            etude_cli_support::reason::Code::UnknownOption
        } else {
            etude_cli_support::reason::Code::InvalidArguments
        };
        return refusal(code, None);
    }
    if let Err(msg) = check_positionals(cmd, &args) {
        eprintln!("sweep: {msg}");
        return refusal(etude_cli_support::reason::Code::InvalidArguments, None);
    }

    // One-time move from the old XDG-style state directory to the correct
    // macOS one (issue #23). Before anything reads state_dir(), so nothing
    // else in this run can look in the new location and find it empty while
    // the old one still has what it's looking for.
    etude_core::journal::migrate_legacy_state_dir();

    match args.first().map(String::as_str) {
        None => run_scan(&std::env::current_dir().unwrap_or_default(), &args),
        Some("help" | "--help" | "-h") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some("version" | "--version" | "-V") => {
            println!("sweep {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("verify") => verify(),
        Some("lesson") => cmd_lesson(&args),
        Some("apply") => cmd_apply(&args),
        Some("propose") => cmd_propose(&args),
        Some("undo") => cmd_undo(&args),
        Some("status") => cmd_status(&args),
        Some("forget") => cmd_forget(&args),
        Some("review") => cmd_review(&args),
        // Every remaining shape -- a bare path, a path behind flags, `--`
        // -- goes through the one finder. The arm this replaces matched any
        // leading flag and scanned the current directory, so
        // `sweep --depth 2 ~/Downloads` reported on wherever the user
        // happened to be standing and never looked at the path they typed.
        _ => match path_arg("", &args) {
            Ok(Some(p)) => run_scan(&p, &args),
            Ok(None) => run_scan(&std::env::current_dir().unwrap_or_default(), &args),
            Err(msg) => {
                eprintln!("sweep: {msg}");
                refusal(etude_cli_support::reason::Code::InvalidArguments, None)
            }
        },
    }
}

/// Read-only journal status by durable id or operation root. This deliberately
/// bypasses startup migration and pruning in `run_main`.
fn cmd_status(args: &[String]) -> ExitCode {
    use etude_cli_support::reason::{Code, Reason};
    let json = has(args, "--json");
    let tail: Vec<&str> = args
        .iter()
        .skip(1)
        .map(String::as_str)
        .filter(|arg| *arg != "--json")
        .collect();
    let id_arg = if tail.first() == Some(&"--id") && tail.len() == 2 {
        Some(tail[1].to_owned())
    } else {
        None
    };
    let path = if id_arg.is_none() && tail.len() == 1 && tail[0] != "--id" {
        Some(PathBuf::from(tail[0]))
    } else {
        None
    };
    if id_arg.is_none() == path.is_none() {
        eprintln!("sweep: status expects either --id OPERATION or one PATH");
        etude_cli_support::envelope::reason(Reason::refusal(Code::InvalidArguments, None));
        return ExitCode::from(2);
    }
    if let Some(file) = path.as_deref().filter(|path| path.is_file()) {
        return path_is_a_file(file);
    }
    if let Some(folder) = path.as_deref()
        && let Err(error) = std::fs::canonicalize(folder)
    {
        return etude_cli_support::problems::folder_open_problem("sweep", folder, &error).report();
    }
    let candidates = match etude_core::journal::candidates_by_recency("sweep") {
        Ok(candidates) => candidates,
        Err(etude_core::journal::JournalError::NotFound) => {
            if let Some(id) = id_arg {
                return status_unknown(&id);
            }
            if !json {
                println!("No apply of that folder is on record.");
            }
            etude_cli_support::envelope::reason(Reason::nothing_to_do());
            etude_cli_support::envelope::status("nothing_to_do");
            etude_cli_support::envelope::detail(etude_core::json::arr([]));
            return ExitCode::from(1);
        }
        Err(error) => return status_journal_error(None, error),
    };
    let key = match etude_keep::key_if_present() {
        Ok(Some(key)) => key,
        Ok(None) => {
            etude_core::scan::record_read_outcome("key_material", false);
            set_journal_key_reason();
            eprintln!(
                "{}",
                etude_cli_support::journal_key_problem(
                    "sweep",
                    &"there is no key in the login keychain, so undo records cannot be read",
                    false,
                    false,
                )
            );
            return ExitCode::from(2);
        }
        Err(error) => {
            etude_core::scan::record_read_outcome("key_material", false);
            set_journal_key_reason();
            eprintln!(
                "{}",
                etude_cli_support::journal_key_problem(
                    "sweep",
                    &error,
                    matches!(error, etude_keep::KeepError::SuppliedKey),
                    false,
                )
            );
            return ExitCode::from(2);
        }
    };
    etude_core::scan::record_read_outcome("key_material", true);
    let sealer = KeychainSeal { key };
    let selected_root = path.as_deref().and_then(|p| std::fs::canonicalize(p).ok());
    let mut summaries = Vec::new();
    for candidate in candidates {
        if id_arg.as_deref().is_some_and(|id| id != candidate.id) {
            continue;
        }
        let journal = match candidate.load("sweep", &sealer) {
            Ok(journal) => journal,
            Err(error) => {
                let code = status_journal_error(Some(&candidate.id), error);
                if id_arg.is_none() {
                    print_other_records(&candidate.id, &sealer);
                }
                return code;
            }
        };
        if id_arg.is_none() && selected_root.as_deref() != Some(journal.root.as_path()) {
            continue;
        }
        let not_as_left = etude_core::apply::items_not_as_left(&journal);
        let damaged =
            journal.progress_tail_damaged || etude_core::apply::unrecorded_moves(&journal) > 0;
        summaries.push((
            journal.summary(),
            not_as_left,
            damaged,
            journal.root.clone(),
        ));
        if id_arg.is_some() {
            break;
        }
    }
    if summaries.is_empty() {
        if let Some(id) = id_arg {
            return status_unknown(&id);
        }
        if !json {
            println!("No apply of that folder is on record.");
        }
        etude_cli_support::envelope::reason(Reason::nothing_to_do());
        etude_cli_support::envelope::status("nothing_to_do");
        etude_cli_support::envelope::detail(etude_core::json::arr([]));
        return ExitCode::from(1);
    }
    etude_cli_support::envelope::reason(Reason::completed());
    etude_cli_support::envelope::status("done");
    for (summary, not_as_left, damaged, folder) in summaries {
        let state = match summary.state {
            etude_core::journal::JournalState::Refused => "refused",
            etude_core::journal::JournalState::Completed => "completed",
            etude_core::journal::JournalState::Interrupted => "interrupted",
            etude_core::journal::JournalState::Reversed => "reversed",
            etude_core::journal::JournalState::PartlyReversed => "partly_reversed",
        };
        let command = if summary.moved > summary.reversed {
            undo_id_step(&summary.id)
        } else {
            String::new()
        };
        let item = etude_core::json::obj(&[
            ("operation_id", etude_core::json::str(&summary.id)),
            ("tool", etude_core::json::str(&summary.tool)),
            ("state", etude_core::json::str(state)),
            (
                "counts",
                etude_core::json::obj(&[
                    ("planned", etude_core::json::num(summary.planned)),
                    ("moved", etude_core::json::num(summary.moved)),
                    ("reversed", etude_core::json::num(summary.reversed)),
                    ("not_as_left", etude_core::json::num(not_as_left)),
                ]),
            ),
            ("damaged", etude_core::json::bool(damaged)),
            (
                "folder",
                etude_core::json::str(&etude_cli_support::shown(&folder)),
            ),
            (
                "command",
                if command.is_empty() {
                    "null".into()
                } else {
                    etude_core::json::str(&command)
                },
            ),
        ]);
        etude_cli_support::envelope::detail(item);
        if !json {
            println!(
                "{} {} planned={} moved={} reversed={}{}",
                summary.id,
                state,
                summary.planned,
                summary.moved,
                summary.reversed,
                if command.is_empty() {
                    String::new()
                } else {
                    format!("; {command}")
                }
            );
            println!("  folder: {}", etude_cli_support::shown(&folder));
            if not_as_left > 0 {
                println!(
                    "  {} not as apply left {}: gone from where it put {}, changed, or the original place is taken.",
                    etude_cli_support::counted(not_as_left, "file is", "files are"),
                    if not_as_left == 1 { "it" } else { "them" },
                    if not_as_left == 1 { "it" } else { "them" },
                );
            }
            if damaged {
                println!(
                    "  This undo record is damaged, so these counts may be low. Files may still be moved."
                );
            }
        }
    }
    ExitCode::SUCCESS
}

/// `undo` and `status` take the folder that was applied, and this is a file.
fn path_is_a_file(path: &Path) -> ExitCode {
    eprintln!(
        "sweep: {} is a file. Give the folder you applied.",
        etude_cli_support::shown(path)
    );
    failure(
        etude_cli_support::reason::Code::MissingInput,
        Some(etude_cli_support::reason::Subject::wait("input", None)),
    )
}

fn status_unknown(id: &str) -> ExitCode {
    use etude_cli_support::reason::{Code, Reason};
    let _ = id;
    etude_cli_support::envelope::reason(Reason::refusal(Code::UnknownOperation, None));
    etude_cli_support::envelope::status("nothing_to_do");
    eprintln!("sweep: unknown operation id");
    ExitCode::from(1)
}

fn status_journal_error(id: Option<&str>, error: etude_core::journal::JournalError) -> ExitCode {
    use etude_cli_support::reason::{Code, Reason, Subject};
    etude_cli_support::envelope::reason(Reason::refusal(
        Code::JournalUnreadable,
        Some(Subject::wait("journal", None)),
    ));
    etude_cli_support::envelope::status("error");
    match id {
        Some(id) => eprintln!(
            "sweep: cannot report it: {}",
            etude_cli_support::problems::record_open_problem("sweep", id, &error)
        ),
        None => eprintln!(
            "{}",
            etude_cli_support::problems::record_folder_problem("sweep", &error)
        ),
    }
    ExitCode::from(3)
}

fn set_reason(
    code: etude_cli_support::reason::Code,
    subject: Option<etude_cli_support::reason::Subject>,
) {
    etude_cli_support::envelope::reason(etude_cli_support::reason::Reason::refusal(code, subject));
}

fn refusal(
    code: etude_cli_support::reason::Code,
    subject: Option<etude_cli_support::reason::Subject>,
) -> ExitCode {
    set_reason(code, subject);
    ExitCode::from(2)
}

fn failure(
    code: etude_cli_support::reason::Code,
    subject: Option<etude_cli_support::reason::Subject>,
) -> ExitCode {
    set_reason(code, subject);
    ExitCode::from(3)
}

/// Seven exercises. Each one is a command to run and one thing to look at.
///
/// The lesson is in the binary rather than in a README because a tool that
/// cannot teach itself is a tool you have to be handheld through. It never
/// runs a command for you: reading output you did not ask for teaches nothing.
const LESSON: &[(&str, &str)] = &[
    (
        "Point it at a folder",
        "    mkdir -p ~/sweep-practice && cd ~/sweep-practice\n\
             mkfx Desktop && sweep Desktop\n\
         \n\
         A hundred plausible files, and a plan for them. Nothing moved.\n\
         \n\
         Open the folder in Finder and leave the window visible. Every step\n\
         after this is worth watching happen.\n\
         \n\
         Find the line saying some files look like personal records and were\n\
         not touched. Open one. That is the file this tool exists to not\n\
         touch.\n\
         \n\
         `mkfx` builds throwaway fixtures and ships with the etudes source. Any\n\
         folder of junk works as well.",
    ),
    (
        "Ask why",
        "    sweep Desktop --explain\n\
         \n\
         Every file, and the signals that placed it. This is how you disagree\n\
         with a grouping instead of just distrusting it.\n\
         \n\
         Contents were not read to decide. The signals are filesystem and Finder\n\
         organization metadata.\n\
         \n\
         The same plan comes in two other shapes:\n\
         \n\
             sweep Desktop --json     for something that will parse it\n\
             sweep Desktop --quiet    counts only, never a filename",
    ),
    (
        "Move one group instead of all of them",
        "    sweep apply Desktop --only Screenshots\n\
         \n\
         All-or-nothing is a bad default for someone else's folder. Run the\n\
         plan, pick one name out of it, move only that.\n\
         \n\
             sweep review Desktop\n\
         \n\
         Same idea, one group at a time, with a chance to rename before it\n\
         moves.",
    ),
    (
        "Try to move a personal record",
        "    sweep apply Desktop --yes\n\
         \n\
         Watch Finder. Groups move. Now look for the file you opened in step 1.\n\
         \n\
         It is still there. There is no flag in `sweep help` that moves it, and\n\
         that is the claim the whole tool rests on. A refusal you can override\n\
         is not a refusal.",
    ),
    (
        "Put it back",
        "    sweep undo\n\
         \n\
         Finder returns to before the apply in step 4. Run it again and it goes\n\
         back another step, to before the one group you moved in step 3.\n\
         \n\
             sweep undo\n\
             sweep undo Desktop\n\
         \n\
         Each run reverses the most recent apply that has not been reversed yet.\n\
         Naming a folder finds that folder's, which is what makes two applies to\n\
         two different folders both reachable.\n\
         \n\
         That worked because the apply wrote a journal. Skip the journal and you\n\
         give up the undo:\n\
         \n\
             sweep apply Desktop --yes --no-journal\n\
             sweep undo\n\
         \n\
         It refuses. Nothing recorded where anything came from.",
    ),
    (
        "Destroy what it remembered",
        "    sweep undo\n\
         \n\
         It refuses now: everything it recorded has already been put back.\n\
         \n\
         The journal is still on disk. It can no longer undo anything, and it is\n\
         still an index of your filenames for the rest of its thirty days. That\n\
         is what this removes:\n\
         \n\
             sweep forget\n\
         \n\
         It asks first, because a key stash can rely on the same store. Read\n\
         what it asks before answering.",
    ),
    (
        "Make it state its promise",
        "    sweep verify\n\
         \n\
         In step 3 you tried to break the promise and could not. This is that\n\
         promise written down by the tool itself, and it is the thing to argue\n\
         with if you want to argue with anything.\n\
         \n\
         A tool that tells you its posture can be checked against its own\n\
         behaviour. One that does not has to be trusted.",
    ),
];

fn cmd_lesson(args: &[String]) -> ExitCode {
    let n = match args.get(1) {
        None => {
            println!("sweep lesson: {} exercises\n", LESSON.len());
            for (i, (title, _)) in LESSON.iter().enumerate() {
                println!("    {}  {title}", i + 1);
            }
            println!("\nRun `sweep lesson 1` to start. Each step names the next.");
            return ExitCode::SUCCESS;
        }
        Some(v) => match v.parse::<usize>() {
            Ok(n) if (1..=LESSON.len()).contains(&n) => n,
            _ => {
                eprintln!(
                    "sweep: lesson takes a step from 1 to {}, got {}",
                    LESSON.len(),
                    etude_cli_support::shown_text(v)
                );
                return refusal(etude_cli_support::reason::Code::InvalidArguments, None);
            }
        },
    };

    let (title, body) = LESSON[n - 1];
    println!("sweep lesson {n}/{}  ·  {title}\n", LESSON.len());
    println!("{body}");
    if n < LESSON.len() {
        println!("\nNext: sweep lesson {}", n + 1);
    } else {
        println!("\nThat is the tool. `sweep help` is the rest.");
    }
    ExitCode::SUCCESS
}

fn has(args: &[String], flag: &str) -> bool {
    args.iter().any(|a| a == flag)
}

fn value(args: &[String], flag: &str) -> Option<String> {
    let i = args.iter().position(|a| a == flag)?;
    args.get(i + 1).cloned()
}

/// `folders` is the currently implemented organization scheme. Keep its name
/// explicit in the CLI so later schemes cannot silently change today's
/// default behavior.
fn parse_scheme(args: &[String]) -> Result<(), String> {
    match value(args, "--scheme") {
        None => Ok(()),
        Some(name) if name == "folders" || name == "existing" => Ok(()),
        Some(name) if name == "tags" && cfg!(target_os = "macos") => Ok(()),
        Some(name) if name == "tags" => Err(
            "`--scheme tags` needs macOS: Finder tags are stored in a macOS file attribute"
                .to_string(),
        ),
        Some(name) => Err(format!(
            "unsupported scheme {}; this version supports `--scheme folders`, `--scheme existing` and `--scheme tags`",
            etude_cli_support::shown_text(&name)
        )),
    }
}

/// Parses `--depth N`. Absent means the documented default of 1.
/// Present-but-invalid (not a number, or outside 1..=8) is an explicit
/// error, never a silent fallback -- a wrong depth changes what gets
/// scanned and must not fail quietly.
fn parse_depth(args: &[String]) -> Result<u8, String> {
    match value(args, "--depth") {
        None => Ok(1),
        Some(v) => match v.parse::<u8>() {
            Ok(d) if (1..=8).contains(&d) => Ok(d),
            _ => Err(format!(
                "--depth must be a number from 1 to 8, got {}",
                etude_cli_support::shown_text(&v)
            )),
        },
    }
}

/// Every command, and every flag reachable from it.
///
/// One table beside the dispatch, rather than a list per command. This bug
/// class appeared five times — `stash --version` stashing the whole working
/// directory, `sweep PATH --explainn` printing an ordinary scan, `unpack
/// --frobnicate` reading a typo as an archive name, and `undo`, `verify` and
/// `forget` ignoring flags outright — and each time the fix was to hand-add
/// one more list, which left the next command exposed. Adding a command here
/// *is* declaring its flags, so forgetting stops being possible rather than
/// merely discouraged.
///
/// The sets come from each command's call graph, not from the body of its
/// function. `--depth` is read by `parse_depth`, which `apply`, `review` and
/// the scan path all call, so it belongs to all three. An earlier
/// hand-written scan list missed exactly this kind of reach and omitted
/// `--no-journal` and `--help`, which would have refused working commands.
///
/// An empty set is not a special case. The rule is that a flag nobody reads
/// is an error, and `undo` and `verify` read none.
/// `--since N[h|d]`: replace the grace window. `--since 0` disables it.
///
/// The default holds back anything changed in the last day. Someone who knows
/// their folder is idle can say so; someone tidying right after a download
/// session can widen it. Returns None when the flag is absent, meaning "use
/// the default", which is distinct from Some(ZERO), meaning "no window".
fn since_flag(args: &[String]) -> Result<Option<std::time::Duration>, String> {
    // The flag is read before the environment variable on purpose. Someone who
    // types --since means it, and a stale SWEEP_GRACE_SECS exported into a
    // shell hours ago should not quietly outrank what they just typed.
    if let Some(i) = args.iter().position(|a| a == "--since") {
        let raw = args.get(i + 1).ok_or_else(|| {
            "--since needs a value, like --since 6h. Run `sweep help`.".to_string()
        })?;
        let (digits, unit) = match raw.chars().last() {
            Some('h' | 'H') => (&raw[..raw.len() - 1], 60 * 60),
            Some('d' | 'D') => (&raw[..raw.len() - 1], 24 * 60 * 60),
            _ => (raw.as_str(), 24 * 60 * 60),
        };
        // Wrong window, loud. This mirrors parse_depth, and for the same
        // reason: --since decides which files are held back, so a value the
        // parser could not read must never become the default silently. A user
        // who typos --since 6hh and is shown the ordinary 24h result has been
        // told their instruction was obeyed when it was discarded.
        let n: u64 = digits.parse().map_err(|_| {
            format!(
                "--since must be a number, optionally with h or d, got {}",
                etude_cli_support::shown_text(raw)
            )
        })?;
        // Overflow is a wrong window, and a wrong window silently changes
        // which files are held back. Unchecked, this panicked in debug and
        // wrapped in release -- the release build accepted a nonsense value
        // and swept with a window nobody chose.
        // The limit is per unit, so state the one that applies. Naming a
        // single number was wrong twice over: it said 213503d when 213504d is
        // accepted, and it said days to someone who typed hours.
        let secs = n.checked_mul(unit).ok_or_else(|| {
            let (most, sym) = match unit {
                3600 => (u64::MAX / 3600, "h"),
                _ => (u64::MAX / (24 * 60 * 60), "d"),
            };
            format!(
                "--since {} is too large. The longest window is {most}{sym}",
                etude_cli_support::shown_text(raw)
            )
        })?;
        return Ok(Some(std::time::Duration::from_secs(secs)));
    }

    // SWEEP_GRACE_SECS exists for the stress harness, which builds a tree and
    // sweeps it in the same second. Every one of its 35 scenarios is inside
    // the default window by construction, and none of them is about the
    // window -- they are about collisions, interruptions, volumes. Threading
    // --since 0 through each would put a flag in 35 places that nobody meant
    // to test, and the next scenario would forget it.
    //
    // Not documented in --help on purpose: a user has --since, which is
    // discoverable and says what it does. This is a harness affordance.
    if let Ok(v) = std::env::var("SWEEP_GRACE_SECS")
        && let Ok(n) = v.parse::<u64>()
    {
        return Ok(Some(std::time::Duration::from_secs(n)));
    }
    Ok(None)
}

const COMMAND_FLAGS: &[(&str, &[(&str, bool)])] = &[
    (
        // The bare `sweep [PATH]` scan.
        "",
        &[
            ("--depth", true),
            ("--json", false),
            ("--export-plan", true),
            ("--quiet", false),
            ("--explain", false),
            ("--allow-sync", false),
            ("--include-tagged", false),
            ("--inspect-content", false),
            ("--since", true),
            ("--map", true),
            ("--scheme", true),
        ],
    ),
    (
        "apply",
        &[
            ("--yes", false),
            ("--save-routes", false),
            ("--json", false),
            ("--plan", true),
            ("--plan-digest", true),
            ("--progress-json", false),
            ("--only", true),
            ("--no-journal", false),
            ("--depth", true),
            ("--allow-sync", false),
            ("--include-tagged", false),
            ("--since", true),
            ("--map", true),
            ("--scheme", true),
        ],
    ),
    (
        "review",
        &[
            ("--progress-json", false),
            ("--allow-sync", false),
            ("--include-tagged", false),
            ("--no-journal", false),
            ("--depth", true),
            ("--since", true),
            ("--scheme", true),
        ],
    ),
    (
        "propose",
        &[
            ("--plan", true),
            ("--plan-digest", true),
            ("--proposal", true),
            ("--export-plan", true),
            ("--json", false),
        ],
    ),
    ("forget", &[("--yes", false)]),
    (
        "undo",
        &[
            ("--json", false),
            ("--id", true),
            ("--progress-json", false),
        ],
    ),
    ("status", &[("--json", false), ("--id", true)]),
    ("verify", &[]),
    ("lesson", &[]),
    ("contract", &[("--json", false)]),
];

/// Flags that exist, but not on the command they were given to. Naming the
/// right place beats calling a real flag unknown.
fn flag_belongs_to(flag: &str, not: &str) -> Vec<&'static str> {
    COMMAND_FLAGS
        .iter()
        .filter(|(cmd, flags)| *cmd != not && flags.iter().any(|(f, _)| *f == flag))
        .map(|(cmd, _)| if cmd.is_empty() { "a scan" } else { *cmd })
        .collect()
}

/// The closest known flag, when it is close enough to be a plausible typo.
/// A cap of 2 keeps `--frobnicate` from confidently suggesting `--json`.
fn nearest_flag(typo: &str, flags: &[(&'static str, bool)]) -> Option<&'static str> {
    fn dist(a: &str, b: &str) -> usize {
        let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
        let mut prev: Vec<usize> = (0..=b.len()).collect();
        for i in 1..=a.len() {
            let mut cur = vec![i];
            for j in 1..=b.len() {
                let c = usize::from(a[i - 1] != b[j - 1]);
                cur.push((prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + c));
            }
            prev = cur;
        }
        prev[b.len()]
    }
    flags
        .iter()
        .map(|(name, _)| (*name, dist(typo, name)))
        .min_by_key(|(_, d)| *d)
        .filter(|(_, d)| *d <= 2)
        .map(|(name, _)| name)
}

/// Refuse any flag the named command does not read.
///
/// `cmd` is the subcommand name, or `""` for a bare scan. Runs before the
/// command does anything, so a refusal cannot produce output that looks like
/// the work succeeding.
fn check_flags(cmd: &str, args: &[String]) -> Result<(), String> {
    let Some((_, allowed)) = COMMAND_FLAGS.iter().find(|(c, _)| *c == cmd) else {
        return Ok(());
    };
    // Flags already seen, so a repeat is refused rather than silently
    // dropped. Found by scoping a rustc-style caret: the argument errors
    // where naming the token is genuinely ambiguous are the ones where a
    // token appears twice, and measuring the main such case showed sweep did
    // not report it at all. It took the first occurrence and discarded the
    // rest without a word.
    //
    // A review corrected an earlier version of this comment that said this
    // was the ONLY ambiguous case. It is not. `--yes --only --yes` reports
    // "--only needs a value, and `--yes` is another option" while two
    // `--yes` tokens exist -- the second is rejected as a missing value, not
    // as a repeat, so this check does not cover it. The message still names
    // the flag whose value is missing, which is enough to locate the
    // mistake, but a caret would genuinely be clearer there and that case is
    // real rather than dismissed.
    //
    //   sweep DIR --depth 1 --depth 3    scanned 6   (--depth 3 ignored)
    //   sweep DIR --depth 3 --depth 1    scanned 12  (--depth 1 ignored)
    //
    // The same command in a different order means a different thing, and
    // nothing said so. On apply it decides which files move: `--only A
    // --only B --yes` moved A and left every file in B where it was.
    //
    // One flag is repeatable: --map accumulates one mapping per occurrence,
    // and validate_maps rejects a duplicate EXTENSION inside the set with a
    // message naming it. Every other flag stays non-repeatable -- a repeat is
    // a mistake where the user believes something is happening that is not.
    // (The first version of this comment said "no flag here is repeatable"
    // and the guard enforced it against --map too, which made the repeatable
    // flag unusable and let a vacuous stress test pass on the wrong exit 2.)
    let mut seen: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        // Everything after `--` is a path, however flag-shaped.
        if a == "--" {
            break;
        }
        // A single dash counts too. The old per-command checks only looked at
        // `--`, so `-n` and `-x` stayed silent everywhere, which is the same
        // defect one level down. `-h` and `-V` are answered at dispatch and
        // never reach here.
        if a.starts_with('-') && !matches!(a.as_str(), "--help" | "--version" | "-h" | "-V") {
            if a != "--map" && seen.contains(&a.as_str()) {
                return Err(format!(
                    "{a} was given more than once, and only the first would\n       \
                     have been used. Remove the ones you do not mean."
                ));
            }
            seen.push(a);
            match allowed.iter().find(|(name, _)| *name == a.as_str()) {
                Some((_, takes_value)) => {
                    if *takes_value {
                        // A value that is itself flag-shaped means the value
                        // was forgotten. `--only --yes` used to take "--yes"
                        // as the group name, filter for a group called that,
                        // find none and exit 1 saying there was nothing to
                        // apply, which reads as "no work" rather than "you
                        // typed it wrong".
                        match args.get(i + 1) {
                            // `-1` is a bad value, not an option; the flag's
                            // own parser says so.
                            Some(v)
                                if v.starts_with('-')
                                    && v != "--"
                                    && !v[1..].starts_with(|c: char| c.is_ascii_digit()) =>
                            {
                                return Err(format!(
                                    "{a} needs a value, and `{v}` is another option."
                                ));
                            }
                            Some(v) if v.is_empty() => {
                                return Err(format!("{a} needs a value and got nothing."));
                            }
                            None => {
                                return Err(format!("{a} needs a value and got nothing."));
                            }
                            Some(_) => {}
                        }
                        i += 1;
                    }
                }
                None => {
                    let here = if cmd.is_empty() {
                        "a scan".to_string()
                    } else {
                        format!("`sweep {cmd}`")
                    };
                    let elsewhere = flag_belongs_to(a, cmd);
                    if !elsewhere.is_empty() {
                        let list: Vec<String> = elsewhere
                            .iter()
                            .map(|c| {
                                if *c == "a scan" {
                                    "a scan".to_string()
                                } else {
                                    format!("`sweep {c}`")
                                }
                            })
                            .collect();
                        return Err(format!(
                            "{a} applies to {}, not to {here}.",
                            list.join(" and ")
                        ));
                    }
                    return Err(match nearest_flag(a, allowed) {
                        Some(sugg) => {
                            format!("unknown option {a}. Did you mean {sugg}? Run `sweep help`.")
                        }
                        None => format!("unknown option {a}. Run `sweep help`."),
                    });
                }
            }
        }
        i += 1;
    }
    Ok(())
}

/// Arguments that are not flags or flag values, for `cmd`.
fn positionals<'a>(cmd: &str, args: &'a [String]) -> Vec<&'a str> {
    let allowed = COMMAND_FLAGS
        .iter()
        .find(|(c, _)| *c == cmd)
        .map_or(&[][..], |(_, flags)| *flags);
    let mut found = Vec::new();
    let mut i = usize::from(!cmd.is_empty());
    while i < args.len() {
        let a = args[i].as_str();
        if a == "--" {
            found.extend(args[i + 1..].iter().map(String::as_str));
            break;
        }
        match allowed.iter().find(|(name, _)| *name == a) {
            Some((_, true)) => i += 2,
            Some((_, false)) => i += 1,
            None => {
                if !a.starts_with('-') {
                    found.push(a);
                }
                i += 1;
            }
        }
    }
    found
}

/// Refuse a word the named command would otherwise drop without a word.
/// `sweep apply A B` moved A and never mentioned B, and `sweep forget xyz`
/// removed every sweep journal after a stray word.
fn check_positionals(cmd: &str, args: &[String]) -> Result<(), String> {
    let first = args.first().map(String::as_str).unwrap_or_default();
    if matches!(first, "version" | "--version" | "-V") {
        return match args[1..].iter().find(|arg| !arg.starts_with('-')) {
            Some(extra) => Err(format!(
                "version takes no arguments, and got `{}`.",
                etude_cli_support::shown_text(extra)
            )),
            None => Ok(()),
        };
    }
    if first == "help" {
        return Ok(());
    }
    let found = positionals(cmd, args);
    let quoted = |word: &str| format!("`{}`", etude_cli_support::shown_text(word));
    if matches!(cmd, "" | "apply" | "undo" | "review" | "status")
        && found.first().is_some_and(|folder| folder.is_empty())
    {
        return Err(
            "the folder name is empty. Give a folder, or leave it out to use the current folder. Nothing was changed."
                .to_string(),
        );
    }
    match cmd {
        "forget" | "verify" => match found.first() {
            Some(extra) if cmd == "forget" => Err(format!(
                "forget takes no arguments, and got {}. Nothing was removed.\n\
                 Run `sweep forget` on its own to remove sweep's undo records.",
                quoted(extra)
            )),
            Some(extra) => Err(format!(
                "{cmd} takes no arguments, and got {}.",
                quoted(extra)
            )),
            None => Ok(()),
        },
        "lesson" => match found.get(1) {
            Some(extra) => Err(format!(
                "lesson takes one step number, and {} is a second.",
                quoted(extra)
            )),
            None => Ok(()),
        },
        "undo" if undo_id(args).is_some() => match (found.first(), undo_id(args)) {
            (Some(extra), _) => Err(format!(
                "undo --id takes no folder, and got {}. Give the id or the folder, not both. Nothing was moved.",
                quoted(extra)
            )),
            (None, Some(id)) if !etude_core::journal::valid_journal_id(id) => Err(format!(
                "{} is not an undo record id. `sweep status FOLDER` lists the ids. Nothing was moved.",
                quoted(id)
            )),
            _ => Ok(()),
        },
        "apply" | "undo" | "review" => match found.get(1) {
            Some(extra) => Err(format!(
                "{cmd} takes one folder, and {} is a second. Nothing was moved.",
                quoted(extra)
            )),
            None => Ok(()),
        },
        "" => match found.get(1) {
            Some(extra) => Err(format!(
                "a scan takes one folder, and {} is a second. Nothing was changed.",
                quoted(extra)
            )),
            None => Ok(()),
        },
        _ => Ok(()),
    }
}

/// `--only` named a group the plan does not have. A plan with no groups at all
/// is a different answer: there is nothing to apply, and the notes say why.
fn no_such_group(only: &str, groups: &[plan::Group], place: &str) -> Option<String> {
    if groups.is_empty() || groups.iter().any(|group| group.name == only) {
        return None;
    }
    let names: Vec<String> = groups
        .iter()
        .map(|group| etude_cli_support::shown_text(&group.name))
        .collect();
    Some(format!(
        "no group named \"{}\" {place}. Groups: {}. Nothing was moved.",
        etude_cli_support::shown_text(only),
        names.join(", ")
    ))
}

/// The positional PATH for `cmd`, skipping any flag's value while it looks.
///
/// This was `apply_path`, hardcoded to "apply", and the bug it fixed was
/// present in every other command that takes a path:
///
///   sweep --depth 2 DIR          scanned the CURRENT directory and never
///                                looked at DIR. Exit 0, no warning.
///   sweep review --depth 2 DIR   took "2" as the path and failed to read it.
///
/// Both hand-rolled the search as "the first argument not starting with -",
/// which is the value of `--depth`. `apply` alone consulted COMMAND_FLAGS to
/// step over a flag's value, and had a test named
/// `apply_depth_value_is_not_mistaken_for_the_path` guarding it.
///
/// So this is the same lesson as COMMAND_FLAGS itself, whose comment says a
/// per-command list "left the next command exposed" five times running. One
/// finder, parameterised by command, means a command cannot pick the path
/// wrongly rather than merely being unlikely to.
fn path_arg(cmd: &str, args: &[String]) -> Result<Option<PathBuf>, String> {
    Ok(typed_path_arg(cmd, args)?.map(|p| PathBuf::from(expand_tilde(p))))
}

/// The path argument exactly as typed, before `~` is expanded, for a printed
/// step that should read the way the person wrote it.
fn typed_path_arg<'a>(cmd: &str, args: &'a [String]) -> Result<Option<&'a str>, String> {
    let mut path = None;
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "--" {
            if path.is_none() {
                path = args.get(i + 1).map(String::as_str);
            }
            break;
        }
        if let Some((_, allowed)) = COMMAND_FLAGS.iter().find(|(c, _)| *c == cmd)
            && let Some((_, takes_value)) = allowed.iter().find(|(name, _)| *name == a)
        {
            i += if *takes_value { 2 } else { 1 };
            continue;
        }
        if a.starts_with('-') {
            return Err(format!("unknown option {a}. Run `sweep help`."));
        }
        if path.is_none() {
            path = Some(a.as_str());
        }
        i += 1;
    }
    Ok(path)
}

/// The scanned folder as one shell word that runs from the current folder,
/// written the way the person typed it: `~/Desktop`, `Downloads`, or `.` when
/// they typed none. Falls back to the full quoted path when the typed form
/// cannot be pasted safely.
fn typed_folder_word(path: &Path, args: &[String]) -> String {
    use etude_cli_support::shell_word;
    let typed = match typed_path_arg("", args) {
        Ok(Some(typed)) => typed,
        Ok(None) => return ".".to_owned(),
        Err(_) => return etude_cli_support::quoted_path(path),
    };
    // Only `~/` is expanded, and only when HOME is set: the same rule as
    // `expand_tilde`, so the shell reads the word back as the same folder.
    let word = match typed.strip_prefix("~/") {
        Some("") if std::env::var_os("HOME").is_some() => Some("~/".to_owned()),
        Some(rest) if std::env::var_os("HOME").is_some() => {
            shell_word(rest).map(|rest| format!("~/{rest}"))
        }
        _ => shell_word(typed),
    };
    word.unwrap_or_else(|| etude_cli_support::quoted_path(path))
}

/// The id given to `undo --id`. A word after `--` is a folder, however it looks.
fn undo_id(args: &[String]) -> Option<&str> {
    let flags = args.split(|arg| arg == "--").next()?;
    let at = flags.iter().position(|arg| arg == "--id")?;
    flags.get(at + 1).map(String::as_str)
}

fn apply_path(args: &[String]) -> Result<PathBuf, String> {
    path_arg("apply", args)?
        .ok_or_else(|| "apply requires an explicit PATH. Run `sweep help`.".to_string())
}

fn expand_tilde(p: &str) -> String {
    match p.strip_prefix("~/") {
        Some(rest) => match std::env::var("HOME") {
            Ok(h) => format!("{h}/{rest}"),
            Err(_) => p.to_string(),
        },
        None => p.to_string(),
    }
}

/// Scan and build a plan, running content inspection when the user asked for
/// it AND consented. Returns the plan plus any inspection stats to disclose.
/// Every `--map EXT=Folder` in order. Parsing only; validation happens in
/// etude-core against the actual root, all-or-nothing.
fn map_flags(args: &[String]) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--map" {
            let v = args
                .get(i + 1)
                .ok_or_else(|| "--map needs EXT=Folder, like --map bpy=Blender".to_string())?;
            let (ext, folder) = v.split_once('=').ok_or_else(|| {
                format!(
                    "--map {}: expected EXT=Folder",
                    etude_cli_support::shown_text(v)
                )
            })?;
            out.push((ext.to_string(), folder.to_string()));
            i += 2;
            continue;
        }
        i += 1;
    }
    Ok(out)
}

struct ReceiptInspector(inspect::ContentInspector);

impl plan::Inspector for ReceiptInspector {
    fn inspect(&mut self, path: &Path, ext: &str) -> Option<etude_core::Category> {
        let before = self.0.stats;
        let found = plan::Inspector::inspect(&mut self.0, path, ext);
        if etude_read::scan::TEXT_EXTS.contains(&ext) {
            let after = self.0.stats;
            let read_completed = after.inspected + after.skipped_binary + after.skipped_slow
                > before.inspected + before.skipped_binary + before.skipped_slow;
            scan::record_read_outcome("consented_text_prefixes", read_completed);
        }
        found
    }
}

fn observation_contract_digest() -> String {
    let mut declaration = etude_cli_support::contract::declaration(
        etude_cli_support::contract::Tool::Sweep,
        env!("CARGO_PKG_VERSION"),
        etude_read::scan::TEXT_EXTS,
        Some(etude_read::buf::MAX_READ),
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
    plan::binding_digest(declaration.as_bytes())
}

fn current_binding_context(profile: &str) -> plan::BindingContext {
    plan::BindingContext::new(
        "sweep",
        env!("CARGO_PKG_VERSION"),
        profile,
        &observation_contract_digest(),
    )
}

#[derive(Debug, Clone)]
struct SchemeConfig {
    name: String,
    maps: Vec<plan::MapSpec>,
    /// `--scheme tags` only: which extension gets which tag.
    tag_maps: Vec<plan::TagSpec>,
    profile_sha256: String,
}

/// Why a scheme could not be set up. A saved profile that is absent and one
/// that cannot be used are different answers, and neither is a typed argument.
#[derive(Debug)]
enum SchemeError {
    Arguments(String),
    ProfileMissing(String),
    ProfileInvalid(String),
}

impl From<String> for SchemeError {
    fn from(message: String) -> Self {
        Self::Arguments(message)
    }
}

impl From<existing_profile::ProfileError> for SchemeError {
    fn from(error: existing_profile::ProfileError) -> Self {
        match error {
            existing_profile::ProfileError::Missing(message) => Self::ProfileMissing(message),
            existing_profile::ProfileError::Invalid(message) => Self::ProfileInvalid(message),
        }
    }
}

/// Print a scheme failure and pick its code.
fn scheme_refusal(error: SchemeError) -> ExitCode {
    use etude_cli_support::reason::{Code, Subject};
    let (message, code, subject) = match error {
        SchemeError::Arguments(message) => (message, Code::InvalidArguments, None),
        SchemeError::ProfileMissing(message) => (
            message,
            Code::ProfileMissing,
            Some(Subject::wait("profile", None)),
        ),
        SchemeError::ProfileInvalid(message) => (
            message,
            Code::ProfileInvalid,
            Some(Subject::wait("profile", None)),
        ),
    };
    eprintln!("sweep: {message}");
    refusal(code, subject)
}

fn resolve_scheme(args: &[String], root: &Path) -> Result<SchemeConfig, SchemeError> {
    parse_scheme(args)?;
    match value(args, "--scheme").as_deref().unwrap_or("folders") {
        "folders" => {
            let raw = map_flags(args)?;
            let maps = if raw.is_empty() {
                Vec::new()
            } else {
                plan::validate_maps(&raw, root)?
            };
            Ok(SchemeConfig {
                name: "folders".into(),
                maps,
                tag_maps: Vec::new(),
                profile_sha256: "none".into(),
            })
        }
        "existing" => {
            if has(args, "--map") {
                return Err(SchemeError::Arguments(
                    "--map cannot be combined with --scheme existing; edit the saved profile instead".into(),
                ));
            }
            let profile = existing_profile::ExistingProfile::load()?;
            // A missing route folder is not a problem: apply creates it for
            // the first file that moves, and the plan says so first.
            let maps = plan::validate_existing_maps(&profile.routes, root).map_err(|why| {
                SchemeError::ProfileInvalid(existing_profile::route_problem(&why))
            })?;
            Ok(SchemeConfig {
                name: "existing".into(),
                maps,
                tag_maps: Vec::new(),
                profile_sha256: profile.digest,
            })
        }
        "tags" => {
            if has(args, "--include-tagged") {
                return Err(SchemeError::Arguments(
                    "--include-tagged does nothing with --scheme tags: tags are added to tagged files too, and their existing tags stay"
                        .into(),
                ));
            }
            let tag_maps = plan::validate_tag_maps(&map_flags(args)?)?;
            Ok(SchemeConfig {
                name: "tags".into(),
                maps: Vec::new(),
                tag_maps,
                profile_sha256: "none".into(),
            })
        }
        _ => unreachable!("parse_scheme checked the scheme name"),
    }
}

fn planning_profile(args: &[String], inspected: bool, scheme: &SchemeConfig) -> String {
    // Written only when set, so a plan made under the default policy binds
    // exactly as before.
    let tagged = if has(args, "--include-tagged") {
        ";include_tagged=true"
    } else {
        ""
    };
    // Written only under `--scheme tags`, so every other plan binds as before.
    let tag_maps = if scheme.name == "tags" {
        format!(";tag_maps={:?}", scheme.tag_maps)
    } else {
        String::new()
    };
    let configuration = format!(
        "scheme={};depth={:?};sync={};grace={:?};maps={:?};content={inspected};profile_sha256={}{tagged}{tag_maps}",
        scheme.name,
        parse_depth(args),
        has(args, "--allow-sync"),
        since_flag(args).map(|value| value.or(ScanConfig::default().grace)),
        scheme.maps,
        scheme.profile_sha256
    );
    format!(
        "sweep-metadata-v1;scheme={};profile_sha256={};configuration={}",
        scheme.name,
        scheme.profile_sha256,
        plan::binding_digest(configuration.as_bytes())
    )
}

fn binding_field<'a>(profile: &'a str, field: &str) -> Option<&'a str> {
    profile.split(';').find_map(|part| {
        part.strip_prefix(field)
            .and_then(|value| value.strip_prefix('='))
    })
}

/// A plan made under `--scheme existing` holds only while its profile does.
/// The caller reports the refusal this returns.
fn verify_existing_profile(profile: &str) -> Result<(), ExitCode> {
    use etude_cli_support::reason::{Code, Subject};
    if binding_field(profile, "scheme") != Some("existing") {
        return Ok(());
    }
    let changed = |why: &str| {
        eprintln!("sweep: {why} Nothing was moved. Export a new plan.");
        refusal(Code::PlanStale, Some(Subject::wait("plan", None)))
    };
    let Some(expected) = binding_field(profile, "profile_sha256") else {
        return Err(changed("this plan does not record which profile it used."));
    };
    match existing_profile::ExistingProfile::load() {
        Ok(found) if found.digest == expected => Ok(()),
        Ok(_) | Err(existing_profile::ProfileError::Missing(_)) => {
            Err(changed("the profile changed since this plan was made."))
        }
        Err(existing_profile::ProfileError::Invalid(message)) => {
            eprintln!("sweep: {message}");
            Err(refusal(
                Code::ProfileInvalid,
                Some(Subject::wait("profile", None)),
            ))
        }
    }
}

fn bound_or_refuse(
    p: plan::Plan,
    context: plan::BindingContext,
) -> Result<plan::BoundPlan, ExitCode> {
    let root = p.root.clone();
    plan::BoundPlan::from_plan(p, context).map_err(|error| {
        // The one bind failure that is not a changed folder says so itself.
        let text = error.to_string();
        if text.ends_with("run the scan again") {
            eprintln!("sweep: {text}");
        } else {
            eprintln!(
                "{}",
                etude_cli_support::problems::folder_changed_just_now("sweep", &root)
            );
        }
        etude_cli_support::envelope::status("refused");
        refusal(
            etude_cli_support::reason::Code::PlanStale,
            Some(etude_cli_support::reason::Subject::wait("plan", None)),
        )
    })
}

fn apply_exported_plan(args: &[String], file: &str) -> ExitCode {
    if path_arg("apply", &args[1..]).is_ok_and(|path| path.is_some()) {
        return etude_cli_support::problems::plan_path_problem("sweep").report();
    }
    if let Some(flag) = [
        "--map",
        "--depth",
        "--since",
        "--allow-sync",
        "--include-tagged",
        "--inspect-content",
        "--export-plan",
        "--scheme",
    ]
    .iter()
    .find(|flag| has(args, flag))
    {
        return etude_cli_support::problems::plan_flag_problem("sweep", flag).report();
    }
    let Some(digest) = value(args, "--plan-digest") else {
        eprintln!("sweep: --plan requires the digest printed with the exported plan");
        return refusal(etude_cli_support::reason::Code::InvalidArguments, None);
    };
    let mut bound = match load_saved_plan(Path::new(file), &digest) {
        Ok(plan) => plan,
        Err(code) => return code,
    };
    let context = current_binding_context(&bound.context().profile);
    if let Err(error) = bound.validate(&context) {
        return etude_cli_support::problems::plan_check_problem("sweep", &bound.root, &error)
            .report();
    }
    if bound.plan.adds_tags && !cfg!(target_os = "macos") {
        eprintln!("sweep: this plan adds Finder tags, which needs macOS");
        return refusal(etude_cli_support::reason::Code::InvalidArguments, None);
    }
    if bound.plan.include_tagged {
        include_tagged_warning();
    }
    let only = value(args, "--only");
    if !has(args, "--yes") && only.is_none() {
        eprintln!("sweep: applying requires --yes or --only NAME");
        return refusal(etude_cli_support::reason::Code::InvalidArguments, None);
    }
    if let Some(msg) = only
        .as_deref()
        .and_then(|name| no_such_group(name, &bound.plan.groups, "in this plan"))
    {
        eprintln!("sweep: {msg}");
        return refusal(etude_cli_support::reason::Code::InvalidArguments, None);
    }
    for group in &mut bound.plan.groups {
        group.accepted = only.as_ref().is_none_or(|name| name == &group.name);
    }
    if let Err(error) = bound.finalize_choices() {
        return etude_cli_support::problems::plan_check_problem("sweep", &bound.root, &error)
            .report();
    }
    if bound.moves() == 0 {
        etude_cli_support::envelope::no_undo_needed();
        eprintln!("sweep: nothing to apply.");
        return ExitCode::from(1);
    }
    if has(args, "--no-journal")
        && bound.groups.iter().any(|group| {
            group.accepted
                && matches!(
                    group.signal,
                    plan::Signal::Mapped { .. }
                        | plan::Signal::Proposed { .. }
                        | plan::Signal::Tagged { .. }
                )
        })
    {
        eprintln!("sweep: mapped, proposed and tag plans require a journal");
        return refusal(
            etude_cli_support::reason::Code::UnsafeInput,
            Some(etude_cli_support::reason::Subject::wait("input", None)),
        );
    }
    let sl = if has(args, "--no-journal") {
        None
    } else {
        let Some(sl) = sealer(false) else {
            return ExitCode::from(2);
        };
        Some(sl)
    };
    run_apply(&bound, &context, sl, "apply")
}

/// Load a plan file for `apply --plan` or `propose`, and refuse one that is not
/// sweep's or whose profile no longer holds.
fn load_saved_plan(file: &Path, digest: &str) -> Result<plan::BoundPlan, ExitCode> {
    use etude_cli_support::problems;
    let bound = plan::BoundPlan::load(file, digest)
        .map_err(|error| problems::plan_load_problem("sweep", file, &error).report())?;
    let maker = bound.context().tool.clone();
    if maker != "sweep" {
        return Err(problems::plan_of_another_tool("sweep", &maker).report());
    }
    if !bound.context().profile.starts_with("sweep-metadata-v1;") {
        eprintln!("{}", problems::plan_stale_text("sweep", &bound.root));
        return Err(refusal(
            etude_cli_support::reason::Code::PlanStale,
            Some(etude_cli_support::reason::Subject::wait("plan", None)),
        ));
    }
    verify_existing_profile(&bound.context().profile)?;
    Ok(bound)
}

/// Build the plan the chosen scheme describes.
fn plan_for(
    scheme: &SchemeConfig,
    outcome: &scan::ScanOutcome,
    inspector: Option<&mut dyn plan::Inspector>,
) -> plan::Plan {
    match (scheme.name.as_str(), inspector) {
        ("tags", inspector) => plan::build_tags(outcome, &scheme.tag_maps, inspector),
        ("existing", None) => plan::build_existing(outcome, &scheme.maps),
        ("existing", Some(inspector)) => {
            plan::build_existing_with(outcome, inspector, &scheme.maps)
        }
        (_, inspector) => plan::build_with_maps(outcome, inspector, &scheme.maps),
    }
}

fn scan_and_plan(
    path: &Path,
    args: &[String],
) -> Result<(plan::Plan, Option<etude_read::Stats>, SchemeConfig), ExitCode> {
    let scheme = resolve_scheme(args, path).map_err(scheme_refusal)?;
    let depth = match parse_depth(args) {
        Ok(d) => d,
        Err(msg) => {
            eprintln!("sweep: {msg}");
            return Err(refusal(
                etude_cli_support::reason::Code::InvalidArguments,
                None,
            ));
        }
    };
    let cfg = ScanConfig {
        depth,
        allow_sync: has(args, "--allow-sync"),
        // A tag plan adds to a file's tags and moves nothing, so a tagged
        // file is not held back from it.
        include_tagged: has(args, "--include-tagged") || scheme.name == "tags",
        grace: match since_flag(args) {
            Ok(g) => g.or(ScanConfig::default().grace),
            Err(msg) => {
                eprintln!("sweep: {msg}");
                return Err(refusal(
                    etude_cli_support::reason::Code::InvalidArguments,
                    None,
                ));
            }
        },
        ..Default::default()
    };
    let outcome = match scan::scan(path, &cfg) {
        Ok(o) => o,
        Err(e) => {
            refuse_scan(path, &e);
            return Err(scan_exit_code(&e));
        }
    };

    // Agent-directed moves without a journal would be moves nobody can
    // reverse, initiated by the party most likely to be wrong. Refused.
    if scheme.name == "tags" && has(args, "--no-journal") {
        eprintln!(
            "sweep: adding tags with --no-journal is refused. Tags must stay restorable;\n\
             drop --no-journal."
        );
        return Err(refusal(
            etude_cli_support::reason::Code::UnsafeInput,
            Some(etude_cli_support::reason::Subject::wait("input", None)),
        ));
    }
    if !scheme.maps.is_empty() && has(args, "--no-journal") {
        eprintln!(
            "sweep: routed moves with --no-journal are refused. A mapped move must stay\n\
             reversible; drop --no-journal."
        );
        return Err(refusal(
            etude_cli_support::reason::Code::UnsafeInput,
            Some(etude_cli_support::reason::Subject::wait("input", None)),
        ));
    }

    if !has(args, "--inspect-content") {
        let proposed = plan_for(&scheme, &outcome, None);
        return Ok((proposed, None, scheme));
    }

    // Consent to reading is separate from consent to moving. --yes does not
    // cover it, deliberately.
    match inspect::consent_interactive() {
        Ok(true) => {}
        Ok(false) => {
            println!(
                "  Contents were not read. Continuing on filesystem and Finder organization metadata.\n"
            );
            let proposed = plan_for(&scheme, &outcome, None);
            return Ok((proposed, None, scheme));
        }
        Err(e) => {
            refuse("could not read your answer to the consent prompt", &e);
            return Err(failure(
                etude_cli_support::reason::Code::InternalError,
                None,
            ));
        }
    }

    let mut insp = ReceiptInspector(inspect::ContentInspector::new());
    let p = plan_for(&scheme, &outcome, Some(&mut insp));
    Ok((p, Some(insp.0.stats), scheme))
}

fn routing_hint(p: &plan::Plan, path: &Path, args: &[String]) -> Option<(String, String)> {
    if p.adds_tags || p.unclaimed.is_empty() {
        return None;
    }
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    for file in &p.unclaimed {
        if let Some(ext) = file.extension().and_then(|ext| ext.to_str())
            && !ext.is_empty()
            && !ext.chars().any(char::is_control)
        {
            *counts.entry(ext.to_ascii_lowercase()).or_default() += 1;
        }
    }
    let mut exts: Vec<String> = counts.keys().take(3).cloned().collect();
    let mut fallback = "Routed Files".to_owned();
    let mut suffix = 1;
    while std::fs::symlink_metadata(path.join(&fallback)).is_ok() {
        fallback = format!("Routed Files {suffix}");
        suffix += 1;
    }
    let candidate = counts.iter().find(|(ext, count)| {
        **count >= 3
            && !ext.starts_with('-')
            && plan::validate_maps(&[((*ext).clone(), fallback.clone())], path).is_ok()
    });
    let Some((ext, _)) = candidate else {
        let command = "sweep propose --help";
        let kinds = if exts.is_empty() {
            "files".to_owned()
        } else {
            format!(".{} files", exts.join(", ."))
        };
        let folder = etude_cli_support::shown(path);
        let text = format!(
            "{} had no group.\nTo choose a folder for each of the {kinds} in {}, read:\n  {command}",
            etude_cli_support::counted(p.unclaimed.len(), "file", "files"),
            typed_folder_word(path, args)
        );
        return Some((
            text,
            etude_core::json::obj(&[
                ("count", etude_core::json::num(p.unclaimed.len())),
                (
                    "extensions",
                    etude_core::json::arr(exts.iter().map(|ext| etude_core::json::str(ext))),
                ),
                ("folder", etude_core::json::str(&folder)),
                ("command", etude_core::json::str(command)),
            ]),
        ));
    };
    if !exts.contains(ext) {
        exts.pop();
        exts.push(ext.clone());
    }
    // Never suggest a folder found here: the alphabetically first one is a
    // guess, and naming it would be advice the tool made up. The new-folder
    // name is sweep's own, runs as printed, and reads as "your folder here".
    let folder = &fallback;
    let mut carried = String::new();
    for flag in ["--depth", "--since"] {
        if let Some(v) = value(args, flag) {
            carried.push_str(&format!(" {flag} {}", etude_cli_support::shell_word(&v)?));
        }
    }
    for flag in ["--allow-sync", "--include-tagged"] {
        if has(args, flag) {
            carried.push_str(&format!(" {flag}"));
        }
    }
    let map = etude_cli_support::shell_word(&format!("{ext}={folder}"))?;
    // The JSON command keeps the resolved path, so it runs without a shell
    // expanding `~`; the printed one reads the way the person typed it.
    let command = format!(
        "sweep{carried} --map {map} -- {}",
        etude_cli_support::quoted_path(path)
    );
    let typed_command = format!(
        "sweep{carried} --map {map} -- {}",
        typed_folder_word(path, args)
    );
    let text = format!(
        "{} had no group.\nTo route the .{ext} files, name a folder after {ext}= (sweep creates it if missing):\n  {typed_command}",
        etude_cli_support::counted(p.unclaimed.len(), "file", "files")
    );
    Some((
        text,
        etude_core::json::obj(&[
            ("count", etude_core::json::num(p.unclaimed.len())),
            (
                "extensions",
                etude_core::json::arr(exts.iter().map(|ext| etude_core::json::str(ext))),
            ),
            ("folder", etude_core::json::str(folder)),
            ("command", etude_core::json::str(&command)),
        ]),
    ))
}

fn run_scan(path: &Path, args: &[String]) -> ExitCode {
    let quiet = has(args, "--quiet");
    let explain = has(args, "--explain");

    let (plan, stats, scheme) = match scan_and_plan(path, args) {
        Ok(v) => v,
        Err(code) => return code,
    };
    print_include_tagged_warning(args);
    // Quiet suppresses text only; agents still get the structured hint.
    let hint = routing_hint(&plan, path, args);
    if !quiet
        && !has(args, "--json")
        && let Some((text, _)) = &hint
    {
        println!("{text}");
    }

    let mut export_binding = None;
    let mut export_items = None;
    let mut exported = None;
    if let Some(file) = value(args, "--export-plan") {
        let context = current_binding_context(&planning_profile(args, stats.is_some(), &scheme));
        let bound = match bound_or_refuse(plan.clone(), context) {
            Ok(bound) => bound,
            Err(code) => return code,
        };
        if let Err(error) = bound.export(Path::new(&file)) {
            return etude_cli_support::problems::plan_export_problem(
                "sweep",
                Path::new(&file),
                &bound.root,
                &error,
            )
            .report();
        }
        if !has(args, "--json") {
            println!(
                "Plan exported to {file}; contains selected paths and metadata. Digest: {}",
                bound.digest()
            );
        }
        exported = Some((file, bound.digest().to_string()));
        export_binding = Some(bound.binding_json());
        // The files a later `sweep propose` may name, in the plan's own
        // order, bound to this plan's digest.
        // A tags plan places no files, so it offers nothing to propose.
        if !plan.adds_tags {
            export_items = Some(etude_core::proposal::items_json(
                &etude_core::proposal::eligible_items(&bound),
            ));
        }
    }
    let plan_json = || {
        let mut json = plan.to_json();
        if let Some((_, hint_json)) = &hint {
            json.pop();
            json.push_str(&format!(",\"routing_hint\":{hint_json}}}"));
        }
        if let Some(binding) = &export_binding {
            json.pop();
            json.push_str(&format!(",\"binding\":{binding}}}"));
        }
        if let Some(items) = &export_items {
            json.pop();
            json.push_str(&format!(",\"items\":{items}}}"));
        }
        // Only when there is one, so a plan that can apply reads as it always did.
        let blocked = blocked_folders(&plan);
        if !blocked.is_empty() {
            json.pop();
            let names =
                etude_core::json::arr(blocked.iter().map(|name| etude_core::json::str(name)));
            json.push_str(&format!(",\"blocked_folders\":{names}}}"));
        }
        if root_is_locked(&plan) {
            json.pop();
            json.push_str(",\"root_locked\":true}");
        }
        json
    };
    if plan.only_too_recent() {
        etude_cli_support::envelope::reason(
            etude_cli_support::reason::Reason::nothing_to_do_later(
                etude_cli_support::reason::Subject::wait("input", None),
            ),
        );
    }
    if plan.skipped_unreadable > 0 {
        // The plan below is real but partial. Its reason must say so: a
        // caller reading only `reason` would otherwise see `done`.
        etude_cli_support::envelope::status("incomplete");
        set_reason(
            etude_cli_support::reason::Code::PermissionDenied,
            Some(etude_cli_support::reason::Subject::wait("input", None)),
        );
    }
    if plan.groups.is_empty() && has(args, "--json") {
        println!("{}", plan_json());
        return ExitCode::from(1);
    }
    if plan.groups.is_empty() && plan.adds_tags {
        tag_output::nothing_to_tag(&plan);
        return ExitCode::from(1);
    }
    if plan.groups.is_empty() {
        println!(
            "\nScanned {}  ·  filesystem and Finder organization metadata  ·  no contents read to decide  ·  sweep {}",
            etude_cli_support::counted(plan.scanned, "item", "items"),
            env!("CARGO_PKG_VERSION")
        );
        print_refused_by_policy_note(&plan);
        print_unreadable_warning(&plan);
        print_left_alone_notes(&plan, false);
        print_agent_named_note(&plan);
        // "Nothing here needs organising" is only true when nothing was held
        // back. Files inside the grace window or still downloading DO need
        // organising -- just not yet -- and saying otherwise sends someone
        // away believing their folder was looked at and found tidy.
        // skipped_in_flight belongs here too. A folder holding only
        // movie.mp4.download/ disclosed the folder and then concluded
        // "Nothing here needs organising", which denies what it just said.
        // A download in progress is something to organise later, whether it
        // arrived as a file or as a directory.
        let held = plan.too_recent()
            + plan.in_flight()
            + plan.skipped_in_flight
            + plan.skipped_tagged
            + plan.locked_skipped;
        if held > 0 {
            // The left-alone notes above already gave these counts.
            // --since 0 only helps if the grace window is what held things
            // back. An in-flight download is refused regardless of the
            // window, so telling someone to pass --since 0 for a folder of
            // .part files sends them to run a command that changes nothing
            // and prints the same refusal. The directory-form downloads were
            // already disclosed by the left-alone notes above; they are only
            // counted here so the conclusion does not contradict them.
            print_hold_advice(&plan);
            println!("\nNothing has been moved.  Nothing left this machine.");
        } else if !plan.unclaimed.is_empty() {
            println!("\nNothing has been moved. Nothing left this machine.");
        } else {
            println!(
                "\nNothing here needs organising.\n\n\
                 Nothing has been moved.  Nothing left this machine."
            );
        }
        return ExitCode::from(1);
    }

    if has(args, "--json") {
        // The agent contract: same data the human rendering draws from, so the
        // tool cannot tell a person one thing and an agent another.
        println!("{}", plan_json());
        return ExitCode::SUCCESS;
    }

    if plan.adds_tags {
        tag_output::render(&plan, quiet, explain, stats.is_some());
    } else {
        let next = next_commands(args, quiet, exported.as_ref());
        render(&plan, quiet, explain, stats.is_some(), &next);
    }
    if let Some(st) = stats {
        inspect::report(&st);
    }
    ExitCode::SUCCESS
}

/// The one place this fact is worded, so both the empty-groups branch and
/// `render()` say the same thing. Distinct from `skipped_system`: sweep did
/// not choose to leave this out. It tried to read it and could not. The
/// scan is incomplete. "Scanned N items" above must not be read as a
/// total. It names both real-world causes from the filed issue (a permission
/// bit, or a path too long for the OS) rather than assuming EACCES. The
/// same `read_dir` failure covers either. Unlike the other skip counts,
/// this one is worth naming as a WARNING, not a footnote.
fn print_unreadable_warning(p: &plan::Plan) {
    if p.skipped_unreadable > 0 {
        println!(
            "\n  WARNING: {} {} could not be read (permission denied, a path too long, \
             or another read failure). Contents unknown. NOT included in the count above",
            p.skipped_unreadable,
            if p.skipped_unreadable == 1 {
                "item"
            } else {
                "items"
            }
        );
    }
}

/// A deliberate policy refusal: sweep could have entered these and chose
/// not to (a credential/noise directory by name, or an absolute system
/// location). Printed from both human-output sites for the same reason as
/// `print_unreadable_warning`: an agent or a person reading either output
/// mode should get the same disclosure. Worded without "system" alone,
/// since `NEVER_ENTER` also covers plain noise directories like
/// `node_modules`, not just credential or OS locations.
fn print_refused_by_policy_note(p: &plan::Plan) {
    if p.skipped_system > 0 {
        println!(
            "\n  refused {} {}, by policy (a protected, credential, or noise directory)",
            p.skipped_system,
            if p.skipped_system == 1 {
                "location"
            } else {
                "locations"
            }
        );
    }
}

/// Say what the plan deliberately left where it was, and why.
///
/// Scan, apply, and review all draw from the same plan. Keeping their human
/// disclosure here prevents a command from reporting only the work it did
/// while silently omitting files it held back. Every count shares a line with
/// its reason: separating them caused a real user to attach a large count to
/// the wrong explanation.
fn print_left_alone_notes(p: &plan::Plan, explain: bool) {
    print_aliases_skipped(p);
    print_locked_skipped(p);
    let counts = p.sensitive_counts();
    let personal: usize = counts.values().sum();
    let recent = p.too_recent();
    let downloading = p.in_flight();
    // A tag plan reports files no --map names itself.
    let unclear = if p.adds_tags { 0 } else { p.no_clear_group() };
    let in_place = p.already_in_destination();
    let projects = p.skipped_project;
    let incomplete = p.skipped_in_flight;
    let packages = p.skipped_package;
    let tagged = p.skipped_tagged;

    let doc_held = p.project_documents() + p.near_document().map(|(n, _)| n).unwrap_or(0);
    if personal
        + recent
        + downloading
        + unclear
        + in_place
        + projects
        + incomplete
        + packages
        + tagged
        + doc_held
        == 0
    {
        return;
    }

    println!();
    if personal == 1 {
        println!("  1 file looks like a personal record and was not touched");
    } else if personal > 1 {
        println!("  {personal} files look like personal records and were not touched");
    }
    if explain {
        for (cat, n) in &counts {
            println!("      {n:>3}  {}", cat.describe());
        }
    }
    if recent == 1 {
        println!("  1 file changed too recently to judge and was left alone");
    } else if recent > 1 {
        println!("  {recent} files changed too recently to judge and were left alone");
    }
    if downloading == 1 {
        println!("  1 download is still in progress and was left alone");
    } else if downloading > 1 {
        println!("  {downloading} downloads are still in progress and were left alone");
    }
    let documents = p.project_documents();
    if documents == 1 {
        println!("  1 file is a project document and was not touched");
    } else if documents > 1 {
        println!("  {documents} files are project documents and were not touched");
    }
    if let Some((n, marker)) = p.near_document() {
        if n == 1 {
            println!("  1 file stays because {marker} may reference it");
        } else {
            println!("  {n} files stay because {marker} may reference them");
        }
    }
    if unclear == 1 {
        println!("  1 file matched no group and was left where it is");
    } else if unclear > 1 {
        println!("  {unclear} files matched no group and were left where they are");
    }
    if unclear > 0 {
        for (folder, n) in p.below_floor() {
            println!("  {}", below_floor_line(&p.root, folder, n));
        }
    }
    if in_place == 1 {
        println!("  1 file is already in its destination and was left in place");
    } else if in_place > 1 {
        println!("  {in_place} files are already in their destination and were left in place");
    }
    if projects == 1 {
        println!("  1 folder was left alone because it holds a project file");
    } else if projects > 1 {
        println!("  {projects} folders were left alone because they hold project files");
    }
    // Each on its own line with its own reason. A movie.mp4.download/ is not
    // a folder that holds a project file, and a Pages document is not a
    // project bundle -- putting either count under the sentence above would
    // attach a number to a reason that is not its own.
    if incomplete == 1 {
        println!("  1 folder is a download still in progress and was left alone");
    } else if incomplete > 1 {
        println!("  {incomplete} folders are downloads still in progress and were left alone");
    }
    if packages == 1 {
        println!("  1 folder was left alone because macOS treats it as a single item");
    } else if packages > 1 {
        println!("  {packages} folders were left alone because macOS treats each as a single item");
    }
    // A count only: the tag names are never read, so they cannot be shown.
    if tagged == 1 {
        println!("  1 Finder-tagged item was left alone (tag names not read)");
    } else if tagged > 1 {
        println!("  {tagged} Finder-tagged items were left alone (tag names not read)");
    }
}

/// `--include-tagged` changes the default custody rule, so it is announced
/// on stderr every time it is used, including under `--json` and `--quiet`.
fn print_include_tagged_warning(args: &[String]) {
    if has(args, "--include-tagged") {
        include_tagged_warning();
    }
}

fn include_tagged_warning() {
    eprintln!(
        "sweep: WARNING: --include-tagged is on. Finder-tagged items WILL be organised.\n       \
         Their tags stay on them. A tagged item that would have to cross volumes is\n       \
         left in place, because the copy would drop its tag."
    );
}

/// Finder aliases have separate accounting from symlinks.
fn print_aliases_skipped(p: &plan::Plan) {
    if p.aliases_skipped > 0 {
        println!(
            "  {} Finder {} skipped by flag",
            p.aliases_skipped,
            if p.aliases_skipped == 1 {
                "alias"
            } else {
                "aliases"
            }
        );
    }
}

/// Locked items are held by the scan, never moved or tagged.
fn print_locked_skipped(p: &plan::Plan) {
    if p.locked_skipped > 0 {
        println!(
            "  {}. {}",
            etude_cli_support::problems::locked_note(p.locked_skipped),
            etude_cli_support::problems::locked_steps(&p.root, p.locked_skipped, p.locked_system)
        );
    }
}

/// What to do about the items a scan held back, one sentence per cause.
fn print_hold_advice(p: &plan::Plan) {
    let downloading = p.in_flight() + p.skipped_in_flight > 0;
    match (recent_hold(p), downloading) {
        (RecentHold::WaitingHelps, _) => println!(
            "\nNothing else here needs organising. Run again later, or pass\n\
             --since 0 to include everything."
        ),
        (RecentHold::SinceZeroHelps, false) => println!(
            "\nNothing else here needs organising. --since 0 would include the\n\
             recent ones."
        ),
        (RecentHold::SinceZeroHelps, true) => println!(
            "\nNothing else here needs organising. --since 0 would include the\n\
             recent ones; the downloads have to finish first either way."
        ),
        (RecentHold::WouldNotGroup, false) => println!(
            "\nNothing else here needs organising. {}",
            would_not_group(p.too_recent())
        ),
        (RecentHold::WouldNotGroup, true) => println!(
            "\nNothing else here needs organising. {} The downloads finish on\n\
             their own; run again once they have.",
            would_not_group(p.too_recent())
        ),
        (RecentHold::None, true) => println!(
            "\nNothing else here needs organising. These finish downloading on\n\
             their own; run again once they have."
        ),
        (RecentHold::None, false) => println!("\nNothing else here needs organising."),
    }
    if p.skipped_tagged > 0 {
        println!(
            "Tagged items stay where they are unless --include-tagged is passed deliberately."
        );
    }
}

/// What the grace window is holding, and what would change the answer. The
/// words promise waiting only where the JSON reason says retry `later`.
enum RecentHold {
    None,
    WaitingHelps,
    SinceZeroHelps,
    WouldNotGroup,
}

fn recent_hold(p: &plan::Plan) -> RecentHold {
    if p.too_recent() == 0 {
        RecentHold::None
    } else if p.only_too_recent() {
        RecentHold::WaitingHelps
    } else if p.recent_would_group {
        RecentHold::SinceZeroHelps
    } else {
        RecentHold::WouldNotGroup
    }
}

fn would_not_group(count: usize) -> &'static str {
    if count == 1 {
        "That file is too recent to judge, and on its own it would not form a group."
    } else {
        "Those files are too recent to judge, and on their own they would not form a group."
    }
}

/// The way forward when an apply found nothing it may move yet.
fn hold_hint(p: &plan::Plan) -> String {
    let downloading = p.in_flight() + p.skipped_in_flight > 0;
    match (recent_hold(p), downloading) {
        (RecentHold::WaitingHelps, _) => {
            " Run again later, or pass --since 0 to include everything.".into()
        }
        (RecentHold::SinceZeroHelps, false) => " --since 0 would include the recent files.".into(),
        (RecentHold::SinceZeroHelps, true) => {
            " --since 0 would include the recent files; the downloads have to finish first either way."
                .into()
        }
        (RecentHold::WouldNotGroup, false) => format!(" {}", would_not_group(p.too_recent())),
        (RecentHold::WouldNotGroup, true) => format!(
            " {} Run again once the downloads have finished.",
            would_not_group(p.too_recent())
        ),
        (RecentHold::None, true) => " Run again once the downloads have finished.".into(),
        (RecentHold::None, false) => String::new(),
    }
}

/// Say when a folder name came from an agent rather than from the files, and
/// which folders a saved route will create.
///
/// The invariant this qualifies is the oldest in the repo: destination names
/// are derived from the filesystem, never coined. --map with a folder that
/// does not exist yet is the one sanctioned agent exception, and an exception
/// the output does not announce would just be the invariant quietly gone. A
/// saved route names the person's own folder; when it is missing, the plan
/// says sweep creates it before anything moves.
fn print_agent_named_note(p: &plan::Plan) {
    if let Some(note) = agent_named_note(p) {
        println!("{note}");
    }
}

fn agent_named_note(p: &plan::Plan) -> Option<String> {
    let coined: Vec<String> = p
        .groups
        .iter()
        .filter_map(|g| match &g.signal {
            plan::Signal::Mapped {
                created: true,
                saved: false,
                ..
            } => Some(etude_core::shown_text(&g.name)),
            _ => None,
        })
        .collect();
    let mut note = match coined.len() {
        0 => String::new(),
        1 => format!(
            "\n  the folder name {} was chosen by your agent, not derived from your files",
            coined[0]
        ),
        _ => format!(
            "\n  {} folder names were chosen by your agent, not derived from your files: {}",
            coined.len(),
            coined.join(", ")
        ),
    };
    let saved: Vec<String> = p
        .groups
        .iter()
        .filter(|g| {
            matches!(
                g.signal,
                plan::Signal::Mapped {
                    created: true,
                    saved: true,
                    ..
                }
            )
        })
        .map(|g| shown_folder_name(&g.name))
        .collect();
    match saved.len() {
        0 => {}
        1 => note.push_str(&format!(
            "\n  sweep creates the folder {} for your saved route; undo removes it if it is empty",
            saved[0]
        )),
        _ => note.push_str(&format!(
            "\n  sweep creates {} folders for your saved routes: {}; undo removes each one that is empty",
            saved.len(),
            saved.join(", ")
        )),
    }
    (!note.is_empty()).then_some(note)
}

/// Why files of a built-in kind stayed: too few for a new folder, or the name
/// is held here by a link or a file that sweep never moves anything into.
fn below_floor_line(root: &Path, folder: &str, n: usize) -> String {
    let files = etude_cli_support::counted(n, "file", "files");
    let held = match std::fs::symlink_metadata(root.join(folder)) {
        Ok(meta) if meta.file_type().is_symlink() => Some("a link"),
        Ok(meta) if !meta.is_dir() => Some("a file"),
        _ => None,
    };
    if let Some(what) = held {
        return format!(
            "{files} would go to {folder}, but {folder} here is {what}, not a folder, so sweep moves nothing into it. Use a plain folder named {folder}"
        );
    }
    let article = if folder.starts_with(['A', 'E', 'I', 'O', 'U']) {
        "an"
    } else {
        "a"
    };
    format!(
        "{files} would go to {folder}: sweep makes a new folder only for 3 or more files, and moves any number into {article} {folder} folder that is already here"
    )
}

/// A folder name for a plan line. Control characters show as `?`, and a name
/// that starts or ends with a space is put in single quotes, so `Docs ` does
/// not read as `Docs`.
fn shown_folder_name(name: &str) -> String {
    let shown = etude_core::shown_text(name);
    if shown.trim() != shown {
        format!("'{shown}'")
    } else {
        shown
    }
}

/// The commands to run after this scan, exactly as they must be typed. Apply
/// plans again from its own flags, so a footer that dropped `--depth 2` offered
/// a different set of files from the one on screen. Under --quiet the path stays
/// a placeholder, because --quiet promises not to print names.
fn next_commands(args: &[String], quiet: bool, exported: Option<&(String, String)>) -> Vec<String> {
    use etude_cli_support::shell_word;
    let word = |text: &str, placeholder: &str| {
        if quiet {
            placeholder.to_owned()
        } else {
            shell_word(text).unwrap_or_else(|| placeholder.to_owned())
        }
    };
    if let Some((file, digest)) = exported {
        return vec![format!(
            "Apply this plan: sweep apply --plan {} --plan-digest {digest} --yes",
            word(file, "<file>")
        )];
    }
    let path = match path_arg("", args) {
        Ok(Some(path)) => word(&path.to_string_lossy(), "<path>"),
        _ => ".".to_owned(),
    };
    let mut carried = String::new();
    let mut i = 0;
    while i < args.len() && args[i] != "--" {
        match args[i].as_str() {
            "--depth" | "--since" | "--scheme" | "--map" => {
                let value = args.get(i + 1).map_or("", String::as_str);
                let value = shell_word(value).unwrap_or_else(|| "<value>".to_owned());
                carried.push_str(&format!(" {} {value}", args[i]));
                i += 1;
            }
            "--allow-sync" | "--include-tagged" => carried.push_str(&format!(" {}", args[i])),
            _ => {}
        }
        i += 1;
    }
    let mut next = Vec::new();
    // Review takes no --map, so it cannot offer the plan a mapped scan showed.
    if !has(args, "--map") {
        next.push(format!("Review: sweep review {path}{carried}"));
    }
    next.push(format!("Apply:  sweep apply {path}{carried} --yes"));
    next
}

/// Apply refuses a moving plan in a locked folder. A tag plan moves nothing.
fn root_is_locked(p: &plan::Plan) -> bool {
    !p.adds_tags && !p.groups.is_empty() && etude_core::scan::is_locked(&p.root)
}

/// Groups whose folder cannot be made, because something that is not a folder
/// already holds the name. Apply refuses these, so the scan says so first.
fn blocked_folders(p: &plan::Plan) -> Vec<&str> {
    if p.adds_tags {
        return Vec::new();
    }
    p.groups
        .iter()
        .filter(|g| {
            p.root
                .join(&g.name)
                .symlink_metadata()
                .is_ok_and(|found| !found.is_dir())
        })
        .map(|g| g.name.as_str())
        .collect()
}

fn render(p: &plan::Plan, quiet: bool, explain: bool, read_contents: bool, next: &[String]) {
    // This line must never claim more restraint than actually happened.
    let basis = if read_contents {
        "filesystem and Finder organization metadata, and the contents of some text files"
    } else {
        "filesystem and Finder organization metadata  ·  no contents read to decide"
    };
    println!(
        "\nScanned {}  ·  {basis}  ·  sweep {}\n",
        etude_cli_support::counted(p.scanned, "item", "items"),
        env!("CARGO_PKG_VERSION")
    );

    let width = p
        .groups
        .iter()
        .map(|g| g.name.chars().count())
        .max()
        .unwrap_or(10)
        .max(12);
    for g in &p.groups {
        println!(
            "  {:<width$}  {:>3} {:<5}   {}",
            g.name,
            g.members.len(),
            if g.members.len() == 1 {
                "file"
            } else {
                "files"
            },
            g.signal.describe(),
            width = width
        );
    }

    print_left_alone_notes(p, explain);
    print_agent_named_note(p);

    if p.skipped_hidden + p.skipped_symlink > 0 {
        println!(
            "\n  skipped {} hidden {} and {} {}",
            p.skipped_hidden,
            if p.skipped_hidden == 1 {
                "item"
            } else {
                "items"
            },
            p.skipped_symlink,
            if p.skipped_symlink == 1 {
                "symlink"
            } else {
                "symlinks"
            }
        );
    }
    print_refused_by_policy_note(p);
    print_unreadable_warning(p);
    if p.root_is_synced {
        println!("\n  warning: this folder is inside a cloud-synced tree");
    }

    if explain && !quiet {
        println!("\n--- signal trace ---");
        for g in &p.groups {
            println!("\n{}: {}", g.name, g.signal.describe());
            for m in &g.members {
                println!("    {}", m.display());
            }
        }
    }

    println!("\nNothing has been moved.  Nothing left this machine.");
    if !quiet {
        println!("Note: this listing is in your terminal scrollback.");
    }
    // A scan must not offer an apply it already knows will refuse.
    if root_is_locked(p) {
        println!(
            "\nNot ready to apply: {} is locked, so nothing can move into or out of it.\n{} Then scan again.",
            etude_cli_support::shown(&p.root),
            etude_cli_support::problems::UNLOCK_FOLDER
        );
        return;
    }
    let blocked = blocked_folders(p);
    if !blocked.is_empty() {
        for name in &blocked {
            let link = p
                .root
                .join(name)
                .symlink_metadata()
                .is_ok_and(|found| found.is_symlink());
            println!(
                "\nNot ready to apply: a {} named \"{}\" is where sweep would make that folder.",
                if link { "link" } else { "file" },
                etude_cli_support::shown_text(name)
            );
        }
        println!(
            "Rename {}, then scan again.",
            if blocked.len() == 1 { "it" } else { "them" }
        );
        return;
    }
    for command in next {
        println!("{command}");
    }
    // The scan read no contents; an apply does. Say so before the yes.
    println!(
        "Applying reads the first and last 4 KiB of each file it moves, so undo can tell if one changed."
    );
}

/// Binds the keychain-held key to the journal's `Sealer` interface.
struct KeychainSeal {
    key: [u8; 32],
}

impl etude_core::journal::Sealer for KeychainSeal {
    fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>, &'static str> {
        etude_keep::seal(&self.key, plaintext).map_err(|_| "could not seal the journal")
    }
    fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, &'static str> {
        etude_keep::open(&self.key, sealed).map_err(|_| "wrong key or the journal was altered")
    }
}

/// Drop undo records past their TTL, and say how many.
///
/// Keeping an index of the user's filenames forever keeps the exposure
/// forever, so a record past 30 days is dropped by the next run that opens
/// records: an apply or an undo with a journal. Only those runs call this,
/// with the key they already hold for their own work; no run reads the key
/// just to prune, so a scan, `status` or `verify` never touches the keychain
/// for it and prunes nothing.
///
/// Stash records are kept: their files may still be in a holding folder, and
/// only `stash pop` can tell that nothing is left. A record must open with this
/// key before it is dropped. One that does not is the record `sweep undo` tells
/// the person to keep, because the right key opens it again.
fn prune_old_records(seal: &dyn etude_core::journal::Sealer) {
    let expired = etude_core::journal::prune_expired(seal);
    etude_cli_support::envelope::effect("expired_journals_removed", expired);
    if expired > 0 {
        eprintln!(
            "sweep: dropped {} older than {} days",
            etude_cli_support::counted(expired, "undo record", "undo records"),
            etude_core::journal::TTL_DAYS
        );
    }
}

/// Fetch the key, creating it on first use.
///
/// On failure sweep refuses rather than falling back to a plaintext journal.
/// Silently degrading is the failure mode a privacy tool must not have.
fn sealer(takes_no_journal: bool) -> Option<KeychainSeal> {
    let result = etude_keep::key();
    etude_core::scan::record_read_outcome("key_material", result.is_ok());
    match result {
        Ok(key) => Some(KeychainSeal { key }),
        Err(e) => {
            set_journal_key_reason();
            eprintln!(
                "{}",
                etude_cli_support::journal_key_problem(
                    "sweep",
                    &e,
                    matches!(e, etude_keep::KeepError::SuppliedKey),
                    takes_no_journal,
                )
            );
            None
        }
    }
}

/// One condition, one answer: every command refuses an unusable key the same way.
fn set_journal_key_reason() {
    etude_cli_support::envelope::reason(etude_cli_support::reason::Reason::of(
        etude_cli_support::reason::Code::JournalKeyUnavailable,
    ));
    etude_cli_support::envelope::status("refused");
}

/// `sweep review PATH`: scan, decide interactively, apply in one pass.
///
/// No plan is persisted between commands. See the module docs in review.rs:
/// a stored plan is a second plaintext index of the user's filenames, and
/// deleting the asset beats protecting it.
fn cmd_review(args: &[String]) -> ExitCode {
    let path = match path_arg("review", &args[1..]) {
        Ok(Some(p)) => p,
        Ok(None) => std::env::current_dir().unwrap_or_default(),
        Err(msg) => {
            eprintln!("sweep: {msg}");
            return refusal(etude_cli_support::reason::Code::InvalidArguments, None);
        }
    };

    let scheme = match resolve_scheme(args, &path) {
        Ok(scheme) => scheme,
        Err(error) => return scheme_refusal(error),
    };
    if scheme.name == "tags" {
        eprintln!("sweep: review moves files; tags are added from an exported plan instead.");
        return refusal(etude_cli_support::reason::Code::InvalidArguments, None);
    }

    let depth = match parse_depth(args) {
        Ok(d) => d,
        Err(msg) => {
            eprintln!("sweep: {msg}");
            return refusal(etude_cli_support::reason::Code::InvalidArguments, None);
        }
    };
    let cfg = ScanConfig {
        depth,
        allow_sync: has(args, "--allow-sync"),
        include_tagged: has(args, "--include-tagged"),
        grace: match since_flag(args) {
            Ok(g) => g.or(ScanConfig::default().grace),
            Err(msg) => {
                eprintln!("sweep: {msg}");
                return refusal(etude_cli_support::reason::Code::InvalidArguments, None);
            }
        },
        ..Default::default()
    };
    let outcome = match scan::scan(&path, &cfg) {
        Ok(o) => o,
        Err(e) => {
            refuse_scan(&path, &e);
            return scan_exit_code(&e);
        }
    };
    let mut p = if scheme.name == "existing" {
        plan::build_existing(&outcome, &scheme.maps)
    } else {
        plan::build_mapped(&outcome, &scheme.maps)
    };
    // Only the refusal: a note about the folder would read as a review starting.
    if !p.groups.is_empty() && review::refuse_without_terminal() {
        return refusal(etude_cli_support::reason::Code::InvalidArguments, None);
    }
    print_include_tagged_warning(args);
    print_left_alone_notes(&p, false);
    print_agent_named_note(&p);
    if p.groups.is_empty() {
        if p.too_recent() + p.in_flight() + p.skipped_in_flight + p.skipped_tagged > 0 {
            print_hold_advice(&p);
        } else {
            println!("\nNothing here needs organising.");
        }
        return ExitCode::from(1);
    }

    match review::run(&mut p) {
        Ok(review::Outcome::Cancelled) => ExitCode::SUCCESS,
        Err(e) => {
            refuse("could not finish the review", &e);
            failure(etude_cli_support::reason::Code::InternalError, None)
        }
        Ok(review::Outcome::Apply) => {
            let use_journal = !has(args, "--no-journal");
            let sl = if use_journal {
                match sealer(false) {
                    Some(s) => Some(s),
                    None => return ExitCode::from(2),
                }
            } else {
                None
            };
            let context = current_binding_context(&planning_profile(args, false, &scheme));
            let bound = match bound_or_refuse(p, context.clone()) {
                Ok(plan) => plan,
                Err(code) => return code,
            };
            run_apply(&bound, &context, sl, "review")
        }
    }
}

/// Print a refusal the way rustc does: the operation that failed on the
/// first line, the reason underneath.
///
/// The audit that produced this found nine sites doing `eprintln!("sweep:
/// {e}")` -- forwarding an inner error that says what went wrong but never
/// what sweep was attempting when it went wrong. "io error: Permission
/// denied (os error 13)" is a true sentence that leaves the reader guessing
/// whether sweep was reading their folder, writing a journal, or moving a
/// file.
///
/// The context was never missing from the program, only from the message:
/// every one of those call sites knows exactly what it was doing. So this
/// takes the operation as a plain phrase rather than threading context down
/// through the error types, which would have changed shared types that
/// `stash` also renders.
///
/// Deliberately does NOT take a path. The reason a caller knows the
/// operation but must not name its target is the same reason ScanError
/// redacts: "errors never contain paths unless --explain". See
/// THREAT-MODEL § T3.
fn refuse(doing: &str, why: &impl std::fmt::Display) {
    eprintln!("sweep: {doing}");
    eprintln!("       {why}");
}

/// A scan error, naming the folder the person gave.
fn refuse_scan(path: &Path, e: &etude_core::scan::ScanError) {
    eprintln!(
        "{}",
        etude_cli_support::problems::scan_problem("sweep", path, e)
    );
}

/// What an apply or undo failure was, in the person's words. `Journal` is told
/// apart from the moves because it is raised after a move or restore has already
/// succeeded: the files moved, the record of them did not. The caller's own
/// follow-up line says what that means for the files.
fn refuse_apply(
    e: &etude_core::apply::ApplyError,
    root: &Path,
    doing: &str,
    items: &[PathBuf],
    work: etude_cli_support::problems::LockedWork,
) {
    use etude_cli_support::problems::{disk_full, disk_read_only};
    use etude_cli_support::{plain_io_error, shown, shown_text};
    use etude_core::apply::ApplyError as E;
    let name_in = |path: &Path| {
        let parent = path.parent().map(shown).unwrap_or_default();
        let name = etude_core::redact::path(Path::new(path.file_name().unwrap_or_default()));
        (name, parent)
    };
    let folder = shown(root);
    match e {
        E::Io(source) if etude_core::apply::is_copy_mismatch(source) => eprintln!(
            "sweep: the copy to the other disk did not match the original, so the original was kept."
        ),
        E::Io(source) if disk_full(source) => {
            eprintln!("sweep: {doing} in {folder}: the disk is full.")
        }
        E::Io(source) if disk_read_only(source) => {
            eprintln!("sweep: {doing} in {folder}: the disk is read-only.")
        }
        E::Io(source) if etude_core::apply::is_folder_appeared(source) => eprintln!(
            "sweep: {doing} in {folder}: a folder it needed to make appeared after the plan was made. Scan and apply again."
        ),
        E::Io(source) if source.kind() == std::io::ErrorKind::AlreadyExists => eprintln!(
            "sweep: {doing} in {folder}: a file appeared at the destination after the plan was made. Scan and apply again."
        ),
        E::Io(source) => {
            eprintln!("sweep: {doing} in {folder}: {}.", plain_io_error(source));
            let advice = etude_cli_support::problems::denied_advice(source, items, root, work);
            if let Some(advice) = advice {
                eprintln!("       {advice}");
            }
        }
        E::Journal(error) => {
            eprintln!("sweep: {}", etude_cli_support::journal_write_problem(error));
            eprintln!(
                "       Free some space or fix that folder, or set ETUDE_STATE_DIR to another one."
            );
        }
        E::DestinationBlocked(path) => eprintln!(
            "sweep: a file named \"{}\" is where sweep would make that folder, in {}. Rename it, then apply again.",
            path.file_name()
                .map(|name| shown_text(&name.to_string_lossy()))
                .unwrap_or_default(),
            path.parent().map(shown).unwrap_or_default(),
        ),
        E::DestinationExists(path) => {
            let (name, parent) = name_in(path);
            eprintln!(
                "sweep: a file named {name} is already in {parent}. Rename or move one of them, or apply another group with --only NAME."
            );
        }
        E::DestinationCollision(path) => {
            let (name, parent) = name_in(path);
            eprintln!(
                "sweep: two files would land in {parent} with the same name, {name}. Rename one, or apply another group with --only NAME. `sweep {folder} --explain` lists each group's files."
            );
        }
        E::CannotCompareNames(path) => {
            let (name, parent) = name_in(path);
            eprintln!(
                "sweep: refused: the name {name} in {parent} cannot be compared with the other names, so a clash cannot be ruled out. Rename that file, then apply again."
            );
        }
        E::DestinationIsSynced(path) => eprintln!(
            "sweep: refused: {} looks cloud-synced (its path contains \"{}\"). If it is not, or you accept that, pass --allow-sync.",
            shown(path),
            etude_core::scan::sync_marker(path).unwrap_or("a sync folder's name"),
        ),
        E::DestinationLocked(path) => eprintln!(
            "{}",
            etude_cli_support::problems::locked_folder_message(
                "sweep",
                path,
                &format!("sweep {}", etude_cli_support::quoted_path(root))
            )
        ),
        E::DestinationDirectoryChanged(path) => {
            eprintln!(
                "sweep: {doing}: {} was moved or replaced while sweep was working.",
                shown(path)
            )
        }
        E::StalePlan(_) | E::Injected(_) => eprintln!("sweep: {doing} in {folder}: {e}"),
    }
}

fn scan_exit_code(e: &etude_core::scan::ScanError) -> ExitCode {
    use etude_cli_support::reason::{Code, Subject};
    use etude_core::scan::ScanError as E;
    match e {
        E::RefusedProjectRoot { .. } => refusal(Code::ProjectFolder, None),
        // An incoming folder can be scanned safely once it has finished arriving.
        E::RefusedInFlightRoot(_) => refusal(
            Code::DownloadInFlight,
            Some(Subject::wait("download_in_flight", None)),
        ),
        E::RefusedRunningAsRoot
        | E::RefusedSystemLocation(_)
        | E::RefusedSyncRoot(_)
        | E::TooManyEntries { .. } => {
            refusal(Code::UnsafeInput, Some(Subject::wait("input", None)))
        }
        E::NotADirectory(_) => failure(Code::MissingInput, Some(Subject::wait("input", None))),
        E::RootDenied { .. } => failure(Code::PermissionDenied, Some(Subject::wait("input", None))),
        E::Io(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            failure(Code::PermissionDenied, Some(Subject::wait("input", None)))
        }
        // A folder that is not there, or a path that runs through a regular
        // file, is the same condition `NotADirectory` reports: the named input
        // does not exist. The caller can fix that by naming one that does.
        E::Io(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            failure(Code::MissingInput, Some(Subject::wait("input", None)))
        }
        E::Io(_) => failure(Code::InternalError, None),
    }
}

/// Destination* variants are safety refusals (2); Io/Journal/Injected are failures (3).
fn apply_exit_code(e: &etude_core::apply::ApplyError, moved: usize) -> ExitCode {
    use etude_cli_support::problems::{disk_full, disk_read_only};
    use etude_cli_support::reason::{Code, Subject};
    use etude_core::apply::ApplyError::*;
    match e {
        StalePlan(error)
            if error
                .to_string()
                .contains("destination must stay inside the plan root") =>
        {
            refusal(
                Code::DestinationOutsideRoot,
                Some(Subject::wait("destination", None)),
            )
        }
        StalePlan(_) | DestinationDirectoryChanged(_) | DestinationLocked(_) => {
            refusal(Code::PlanStale, Some(Subject::wait("plan", None)))
        }
        DestinationExists(_) | DestinationCollision(_) => refusal(
            Code::DestinationExists,
            Some(Subject::wait("destination", None)),
        ),
        DestinationIsSynced(_) | CannotCompareNames(_) => {
            refusal(Code::UnsafeInput, Some(Subject::wait("input", None)))
        }
        Io(error) if disk_full(error) => failure(
            Code::InsufficientSpace,
            Some(Subject::wait("destination", None)),
        ),
        Io(error)
            if error.kind() == std::io::ErrorKind::PermissionDenied || disk_read_only(error) =>
        {
            failure(Code::PermissionDenied, Some(Subject::wait("input", None)))
        }
        // Occupied, and nothing moved: change the name and the same apply works.
        // Once files have moved it is a partial result like any other.
        DestinationBlocked(_) if moved == 0 => refusal(
            Code::DestinationExists,
            Some(Subject::wait("destination", None)),
        ),
        // The folder appeared after the plan was made, so the plan is out of date.
        // This is how the loser of two applies on one folder can end.
        Io(error) if moved == 0 && error.kind() == std::io::ErrorKind::AlreadyExists => {
            refusal(Code::PlanStale, Some(Subject::wait("plan", None)))
        }
        Journal(_) => failure(
            Code::JournalWriteFailed,
            Some(Subject::wait("journal", None)),
        ),
        DestinationBlocked(_) | Io(_) | Injected(_) => failure(
            Code::OperationIncomplete,
            Some(Subject::wait("journal", None)),
        ),
    }
}

/// No done entries means undo already ran. Exit 1. Don't call undo again.
/// Is there nothing left for undo to reverse?
///
/// A journal whose tail was cut short can NEVER answer this from its entries
/// alone. Apply moves a file and only then records it, so a lost record means
/// a move that happened and is not written down: every entry can read Planned
/// while files sit at their destinations. Short-circuiting on that says
/// "already restored" and strands them, which is the silent stranding this
/// whole area exists to prevent -- and it is what the stress scenario was
/// reporting as "Nothing to undo" on a run that had moved 130 files.
///
/// When the tail is damaged the answer is no, so undo runs and its
/// successor-entry recovery checks the filesystem instead of the journal.
fn journal_is_fully_undone(j: &etude_core::Journal, sl: &dyn etude_core::journal::Sealer) -> bool {
    !etude_core::apply::has_undo_work(j, sl)
}

/// Shared tail of `apply` and `review`.
fn run_apply(
    p: &plan::BoundPlan,
    context: &plan::BindingContext,
    sl: Option<KeychainSeal>,
    operation: &'static str,
) -> ExitCode {
    run_apply_with_routes(p, context, sl, operation, None)
}

fn routes_complete(r: &etude_core::apply::ApplyReport, planned: usize) -> bool {
    planned > 0
        && r.moved == planned
        && r.held_tagged
            + r.held_locked
            + r.held_cross_volume
            + r.tagged_not_returned
            + r.hold_record_failed
            == 0
}

#[cfg(test)]
mod saved_route_tests {
    use super::*;
    #[test]
    fn partial_apply_discards_prepared_routes_without_panicking() {
        let out = std::process::Command::new("mktemp")
            .arg("-d")
            .output()
            .unwrap();
        assert!(out.status.success());
        let base = PathBuf::from(String::from_utf8(out.stdout).unwrap().trim());
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(base.clone());
        std::fs::create_dir(base.join("Box")).unwrap();
        let path = base.join("state/profile.toml");
        let pending = existing_profile::PendingRoutes::prepare_at(
            &base,
            &[plan::MapSpec {
                ext: "bpy".into(),
                folder: "Box".into(),
                created: false,
            }],
            path.clone(),
        )
        .unwrap();
        assert_eq!(
            finish_saved_routes(Some(pending), false, ExitCode::SUCCESS),
            ExitCode::from(3)
        );
        assert!(!path.exists());
        assert_eq!(std::fs::read_dir(base.join("state")).unwrap().count(), 0);
    }

    #[test]
    fn a_failed_folder_flush_after_the_rename_still_counts_as_saved() {
        let out = std::process::Command::new("mktemp")
            .arg("-d")
            .output()
            .unwrap();
        let base = PathBuf::from(String::from_utf8(out.stdout).unwrap().trim());
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(base.clone());
        std::fs::create_dir(base.join("Box")).unwrap();
        let path = base.join("state/profile.toml");
        let mut pending = existing_profile::PendingRoutes::prepare_at(
            &base,
            &[plan::MapSpec {
                ext: "bpy".into(),
                folder: "Box".into(),
                created: false,
            }],
            path.clone(),
        )
        .unwrap();
        pending.fail_folder_sync = true;
        assert_eq!(
            finish_saved_routes(Some(pending), true, ExitCode::SUCCESS),
            ExitCode::SUCCESS
        );
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("folder = \"Box\"")
        );
    }

    #[test]
    fn partial_or_held_results_cannot_save_routes() {
        use etude_core::apply::ApplyReport;
        assert!(!routes_complete(&ApplyReport::default(), 0));
        assert!(!routes_complete(
            &ApplyReport {
                moved: 2,
                ..Default::default()
            },
            3
        ));
        for report in [
            ApplyReport {
                moved: 3,
                held_tagged: 1,
                ..Default::default()
            },
            ApplyReport {
                moved: 3,
                held_locked: 1,
                ..Default::default()
            },
            ApplyReport {
                moved: 3,
                held_cross_volume: 1,
                ..Default::default()
            },
            ApplyReport {
                moved: 3,
                tagged_not_returned: 1,
                ..Default::default()
            },
            ApplyReport {
                moved: 3,
                hold_record_failed: 1,
                ..Default::default()
            },
        ] {
            assert!(!routes_complete(&report, 3));
        }
        assert!(routes_complete(
            &ApplyReport {
                moved: 3,
                ..Default::default()
            },
            3
        ));
    }
}

fn run_apply_with_routes(
    p: &plan::BoundPlan,
    context: &plan::BindingContext,
    sl: Option<KeychainSeal>,
    operation: &'static str,
    pending: Option<existing_profile::PendingRoutes>,
) -> ExitCode {
    if let Err(code) = verify_existing_profile(&context.profile) {
        return code;
    }
    // Before the apply: its own record does not exist yet, so this cannot
    // remove it, and older records go before any history is consulted, as they
    // always have.
    if let Some(seal) = sl.as_ref() {
        prune_old_records(seal);
    }
    let sealer = sl.as_ref().map(|s| s as &dyn etude_core::journal::Sealer);
    let mut progress =
        etude_cli_support::progress::Reporter::for_operation("sweep apply", operation);
    let result = etude_core::apply::apply_bound_with_structured_progress_outcome(
        p,
        context,
        sealer,
        None,
        |event| progress.update(event),
    );
    progress.settle();
    let complete = result.as_ref().is_ok_and(|r| routes_complete(r, p.moves()));
    let mut code = report_apply(result, p, operation, sl.is_some());
    code = finish_saved_routes(pending, complete, code);
    progress.finish(code);
    code
}

fn routes_json(routes: &[(String, String)]) -> String {
    etude_core::json::arr(routes.iter().map(|(ext, folder)| {
        etude_core::json::obj(&[
            ("extension", etude_core::json::str(ext)),
            ("folder", etude_core::json::str(folder)),
        ])
    }))
}

/// `details.routes_saved`: what --save-routes did, for agents.
fn routes_saved_detail(
    state: &str,
    reason: Option<&str>,
    added: &[(String, String)],
    requested: &[(String, String)],
    profile: &Path,
    written: bool,
    folder_synced: bool,
) {
    etude_cli_support::envelope::detail_field(
        "routes_saved",
        etude_core::json::obj(&[
            ("state", etude_core::json::str(state)),
            (
                "reason",
                reason.map_or_else(|| "null".into(), etude_core::json::str),
            ),
            ("added", routes_json(added)),
            ("requested", routes_json(requested)),
            ("profile", etude_core::json::path(profile)),
            ("written", etude_core::json::bool(written)),
            ("folder_synced", etude_core::json::bool(folder_synced)),
        ]),
    );
}

fn profile_subject(path: &Path) -> Option<etude_cli_support::reason::Subject> {
    Some(etude_cli_support::reason::Subject::path(
        "profile",
        &path.display().to_string(),
    ))
}

/// --save-routes was refused before anything moved.
fn save_routes_refusal(error: &existing_profile::PrepareError, maps: &[plan::MapSpec]) -> ExitCode {
    use etude_cli_support::reason::{Code, Subject};
    use existing_profile::PrepareKind as K;
    eprintln!(
        "sweep: {}. Nothing was moved, and no route was saved. {}.",
        error.what, error.next
    );
    let requested: Vec<(String, String)> = maps
        .iter()
        .map(|map| (map.ext.clone(), map.folder.clone()))
        .collect();
    let profile = existing_profile::profile_location();
    routes_saved_detail(
        "not_saved",
        Some("refused_before_moves"),
        &[],
        &requested,
        &profile,
        false,
        false,
    );
    etude_cli_support::envelope::no_undo_needed();
    etude_cli_support::envelope::status("refused");
    let path = error.path.as_deref().map(|path| path.display().to_string());
    let subject = |kind| match &path {
        Some(path) => Subject::path(kind, path),
        None => Subject::wait(kind, None),
    };
    match error.kind {
        K::Arguments => refusal(Code::InvalidArguments, None),
        K::MissingFolder => failure(Code::MissingInput, Some(subject("input"))),
        K::ProfileInvalid => refusal(Code::ProfileInvalid, profile_subject(&profile)),
        K::Denied => failure(Code::PermissionDenied, Some(subject("input"))),
        K::NoSpace => failure(Code::InsufficientSpace, Some(subject("destination"))),
        K::WriteFailed => failure(Code::ProfileWriteFailed, profile_subject(&profile)),
    }
}

fn finish_saved_routes(
    pending: Option<existing_profile::PendingRoutes>,
    complete: bool,
    mut code: ExitCode,
) -> ExitCode {
    let Some(pending) = pending else {
        return code;
    };
    let profile = pending.profile().to_path_buf();
    let shown_profile = etude_cli_support::shown(&profile);
    let added = pending.added.clone();
    let requested = pending.requested.clone();
    if !complete {
        drop(pending);
        eprintln!(
            "sweep: routes were not saved because apply did not move every planned file. Resolve the holds or failure above, then try again."
        );
        routes_saved_detail(
            "not_saved",
            Some("apply_incomplete"),
            &[],
            &requested,
            &profile,
            false,
            false,
        );
        if code == ExitCode::SUCCESS {
            etude_cli_support::envelope::status("incomplete");
            code = failure(
                etude_cli_support::reason::Code::OperationIncomplete,
                Some(etude_cli_support::reason::Subject::wait("journal", None)),
            );
        }
        return code;
    }
    match pending.save() {
        Ok(saved) => {
            if saved.written {
                for (ext, folder) in &added {
                    etude_cli_support::envelope::print(format!(
                        "Saved route .{ext} → {}. Undo moves the files back but keeps this route.",
                        etude_cli_support::shown_text(folder)
                    ));
                }
            } else {
                for (ext, folder) in &requested {
                    etude_cli_support::envelope::print(format!(
                        "Route .{ext} → {} was already saved; the profile was not changed.",
                        etude_cli_support::shown_text(folder)
                    ));
                }
            }
            etude_cli_support::envelope::print(format!("Routes profile: {shown_profile}"));
            if let Some(problem) = &saved.sync_problem {
                eprintln!(
                    "sweep: the route is saved, but the disk did not confirm the profile's folder was written: {problem}. If this Mac loses power soon, check {shown_profile}."
                );
            }
            routes_saved_detail(
                "saved",
                None,
                &added,
                &requested,
                &profile,
                saved.written,
                saved.sync_problem.is_none(),
            );
        }
        Err(error) => {
            let (why, reason) = match &error {
                existing_profile::SaveError::Changed => (
                    format!("{shown_profile} changed while the files were moving"),
                    "profile_changed",
                ),
                existing_profile::SaveError::WriteFailed(why) => (
                    format!("{shown_profile} could not be replaced: {why}"),
                    "write_failed",
                ),
            };
            let lines: Vec<String> = added
                .iter()
                .map(|(ext, folder)| existing_profile::route_lines(ext, folder))
                .collect();
            eprintln!(
                "sweep: the files moved as reported above, but the {} not saved: {why}. To save {}, add these lines to the end of {shown_profile}:\n\n{}\n",
                if added.len() == 1 {
                    "route was"
                } else {
                    "routes were"
                },
                if added.len() == 1 { "it" } else { "them" },
                lines.join("\n\n")
            );
            routes_saved_detail(
                "not_saved",
                Some(reason),
                &[],
                &requested,
                &profile,
                false,
                false,
            );
            etude_cli_support::envelope::status("incomplete");
            code = failure(
                etude_cli_support::reason::Code::ProfileWriteFailed,
                profile_subject(&profile),
            );
        }
    }
    code
}

fn report_apply(
    result: Result<etude_core::apply::ApplyReport, etude_core::apply::ApplyFailure>,
    p: &plan::BoundPlan,
    operation: &'static str,
    journal_enabled: bool,
) -> ExitCode {
    match result {
        Ok(r) => {
            if let Some(id) = r.journal_path.as_deref().and_then(undo_record_id) {
                etude_cli_support::envelope::recovery_operation(id);
            }
            finish_apply(
                &r,
                &p.binding_json(),
                p.adds_tags,
                &undo_step(&p.root),
                &p.root,
            )
        }
        Err(failure) => {
            let moved = failure.moved;
            let e = failure.error;
            etude_cli_support::envelope::effect("items_moved", moved);
            // A journal that became durable before the failure is the handle
            // for `status --id`; name it in the result. It is the only handle:
            // the words below decide whether any undo step is printed.
            if let Some(id) = failure.durable_journal_id.as_deref() {
                etude_cli_support::envelope::recovery_operation(id);
            }
            if matches!(e, etude_core::apply::ApplyError::StalePlan(_)) && moved == 0 {
                etude_cli_support::envelope::status("refused");
                eprintln!("{}", stale_apply_text(&e, p, operation));
                return apply_exit_code(&e, moved);
            }
            let doing = if p.adds_tags {
                "could not tag every file"
            } else {
                "could not move the files"
            };
            let items: Vec<PathBuf> = p
                .groups
                .iter()
                .flat_map(|group| group.members.iter().cloned())
                .collect();
            let work = if p.adds_tags {
                etude_cli_support::problems::LockedWork::TagApply
            } else {
                etude_cli_support::problems::LockedWork::Move
            };
            refuse_apply(&e, &p.root, doing, &items, work);
            // Decided from the count of moves, never from the kind of failure.
            if moved == 0 {
                etude_cli_support::envelope::status("refused");
                if p.adds_tags {
                    eprintln!("No tags were added, so there is nothing to undo.");
                } else if let Some(text) = etude_cli_support::problems::report_copy_left(
                    failure.copy_left.as_deref(),
                    false,
                    false,
                ) {
                    eprintln!("{text}");
                } else {
                    eprintln!("Nothing was moved, so there is nothing to undo.");
                }
                if matches!(&e, etude_core::apply::ApplyError::Io(source) if etude_cli_support::problems::disk_full(source))
                {
                    eprintln!("Free some space, then apply again.");
                }
                return apply_exit_code(&e, moved);
            }
            etude_cli_support::envelope::status("incomplete");
            let what = if p.adds_tags {
                format!(
                    "{} tagged before this stopped.",
                    etude_cli_support::counted(moved, "file was", "files were")
                )
            } else {
                format!(
                    "{} moved before this stopped.",
                    etude_cli_support::counted(moved, "file was", "files were")
                )
            };
            if journal_enabled {
                eprintln!(
                    "{what} To put {} back: {}",
                    if moved == 1 { "it" } else { "them" },
                    undo_step(&p.root)
                );
            } else {
                eprintln!(
                    "{what} --no-journal removed undo, so {} cannot be put back by sweep.",
                    if moved == 1 { "it" } else { "they" }
                );
            }
            if let Some(text) =
                created_folders_text(&failure.created_folders, &p.root, journal_enabled)
            {
                eprintln!("{text}");
            }
            if let Some(text) = etude_cli_support::problems::report_copy_left(
                failure.copy_left.as_deref(),
                true,
                false,
            ) {
                eprintln!("{text}");
            }
            apply_exit_code(&e, moved)
        }
    }
}

/// Name the folders a stopped apply created and left behind, and say what
/// becomes of them, so a person who sees a new folder knows where it came from.
fn created_folders_text(folders: &[PathBuf], root: &Path, journal: bool) -> Option<String> {
    let names: Vec<String> = folders
        .iter()
        .map(|folder| {
            shown_folder_name(
                &folder
                    .strip_prefix(root)
                    .unwrap_or(folder)
                    .display()
                    .to_string(),
            )
        })
        .collect();
    let (what, it) = match names.len() {
        0 => return None,
        1 => (format!("the folder {}", names[0]), "it"),
        _ => (format!("the folders {}", names.join(", ")), "each one"),
    };
    Some(if journal {
        format!("This apply created {what}; undo removes {it} once it is empty.")
    } else {
        format!("This apply created {what}; with --no-journal, undo cannot remove {it}.")
    })
}

/// The id in an undo record's file name.
pub(crate) fn undo_record_id(path: &Path) -> Option<&str> {
    path.file_name()?
        .to_str()?
        .strip_prefix("sweep-")?
        .strip_suffix(".journal")
}

/// Why a note about a refused attempt could not be saved.
fn undo_record_write_reason(error: &etude_core::apply::ApplyError) -> String {
    match error {
        etude_core::apply::ApplyError::Journal(error) => {
            etude_cli_support::plain_journal_error(error)
        }
        etude_core::apply::ApplyError::Io(error) => etude_cli_support::plain_io_error(error),
        other => other.to_string(),
    }
}

/// Words for a plan that stopped being true before anything moved.
fn stale_apply_text(
    e: &etude_core::apply::ApplyError,
    p: &plan::BoundPlan,
    operation: &str,
) -> String {
    use etude_cli_support::problems;
    if e.to_string()
        .contains("destination must stay inside the plan root")
    {
        return format!(
            "sweep: a folder in this plan would land outside {}. Nothing was moved. Use a plain folder name.",
            etude_cli_support::shown(&p.root)
        );
    }
    // Only a plan saved to a file can be old; these two planned just now.
    if p.is_live() || operation == "review" {
        problems::folder_changed_just_now("sweep", &p.root)
    } else {
        problems::plan_stale_text("sweep", &p.root)
    }
}

/// The undo command for this folder. It names the folder, so running it cannot
/// reverse an apply of some other folder.
fn undo_step(root: &Path) -> String {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    format!("sweep undo {}", etude_cli_support::quoted_path(&root))
}

/// The undo command for one record. It cannot reverse any other apply.
fn undo_id_step(id: &str) -> String {
    format!("sweep undo --id {id}")
}

/// Report an apply that ran to the end, and pick its exit.
///
/// Held items are counts in the normal result. Two outcomes are not normal: a
/// tagged item that could not be returned (it is where the tag was found, not
/// where the plan said), and a hold the journal could not record. Both finish
/// as `incomplete` with exit 3 and the counts printed, never a bare error. A
/// run where every item was held moved nothing and is reported the way a run
/// with nothing to do is.
fn finish_apply(
    r: &etude_core::apply::ApplyReport,
    binding: &str,
    adds_tags: bool,
    undo: &str,
    root: &Path,
) -> ExitCode {
    if adds_tags {
        return tag_output::finish_apply(r, binding, undo, root);
    }
    etude_cli_support::envelope::effect("items_moved", r.moved);
    etude_cli_support::envelope::detail(etude_core::json::obj(&[
        ("moved", etude_core::json::num(r.moved)),
        ("held_tagged", etude_core::json::num(r.held_tagged)),
        ("held_locked", etude_core::json::num(r.held_locked)),
        (
            "held_cross_volume",
            etude_core::json::num(r.held_cross_volume),
        ),
        (
            "tagged_not_returned",
            etude_core::json::num(r.tagged_not_returned),
        ),
        (
            "hold_record_failed",
            etude_core::json::num(r.hold_record_failed),
        ),
        ("binding", binding.to_string()),
        (
            "journal",
            r.journal_path
                .as_deref()
                .map(etude_core::json::path)
                .unwrap_or_else(|| "null".into()),
        ),
    ]));
    println!(
        "\nMoved {}.",
        etude_cli_support::counted(r.moved, "file", "files")
    );
    print_apply_holds(r, root);
    if r.moved > 0 {
        match &r.journal_path {
            Some(jp) => {
                println!("Undo with: {undo}");
                if let Some(id) = undo_record_id(jp) {
                    println!("Undo record id: {id}");
                }
                println!("Encrypted journal: {}", jp.display());
            }
            None => println!("No journal was written. This cannot be undone."),
        }
    }
    println!("\nNothing left this machine.");
    if r.tagged_not_returned > 0 || r.hold_record_failed > 0 {
        etude_cli_support::envelope::status("incomplete");
        return failure(
            etude_cli_support::reason::Code::OperationIncomplete,
            Some(etude_cli_support::reason::Subject::wait("journal", None)),
        );
    }
    if r.moved == 0 && r.held_tagged + r.held_locked + r.held_cross_volume > 0 {
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

fn locked_after_scan_note(count: usize, root: &Path) -> String {
    format!(
        "{} locked after the scan and {} in place. {}",
        etude_cli_support::counted(count, "item was", "items were"),
        if count == 1 { "stays" } else { "stay" },
        etude_cli_support::problems::unlock_step(root)
    )
}

/// Counts of what apply left alone, with the reason for each. No tag names and
/// no paths.
fn print_apply_holds(r: &etude_core::apply::ApplyReport, root: &Path) {
    let items = |n: usize| if n == 1 { "item" } else { "items" };
    if r.held_tagged > 0 {
        println!(
            "{} Finder-tagged {} left in place (tagged after the scan, or during the move and moved back).",
            r.held_tagged,
            items(r.held_tagged)
        );
    }
    if r.held_cross_volume > 0 {
        println!(
            "{} Finder-tagged {} left in place: moving to another volume would drop the tag.",
            r.held_cross_volume,
            items(r.held_cross_volume)
        );
    }
    if r.held_locked > 0 {
        println!("{}", locked_after_scan_note(r.held_locked, root));
    }
    if r.tagged_not_returned > 0 {
        println!(
            "{} Finder-tagged {} could not be returned because the original place was taken or the return failed. {} at the destination; `sweep undo` reports {}.",
            r.tagged_not_returned,
            items(r.tagged_not_returned),
            if r.tagged_not_returned == 1 {
                "It stays"
            } else {
                "They stay"
            },
            if r.tagged_not_returned == 1 {
                "it"
            } else {
                "them"
            },
        );
    }
    if r.hold_record_failed > 0 {
        eprintln!(
            "sweep: the journal could not record a held item and apply stopped. The files are where the counts above say; `sweep undo` reconciles from the filesystem."
        );
    }
}

/// `sweep apply PATH --yes | --only NAME`
///
/// Re-scans rather than trusting a stored plan. The filesystem may have changed
/// since the plan was printed, and a stale plan is the write-freshness failure:
/// the record says one thing and the tree says another.
fn cmd_apply(args: &[String]) -> ExitCode {
    if has(args, "--save-routes")
        && (!has(args, "--map")
            || has(args, "--no-journal")
            || value(args, "--scheme").as_deref() == Some("tags")
            || has(args, "--plan"))
    {
        eprintln!(
            "sweep: --save-routes requires --map on a live apply with a journal; it cannot use --scheme tags, --no-journal or --plan. Nothing was moved. Drop the conflicting option."
        );
        return refusal(etude_cli_support::reason::Code::InvalidArguments, None);
    }

    if let Some(file) = value(args, "--plan") {
        return apply_exported_plan(args, &file);
    }
    if has(args, "--plan-digest") {
        eprintln!("sweep: --plan-digest requires --plan FILE");
        return refusal(etude_cli_support::reason::Code::InvalidArguments, None);
    }
    let path = match apply_path(&args[1..]) {
        Ok(path) => path,
        Err(msg) => {
            eprintln!("sweep: {msg}");
            return refusal(etude_cli_support::reason::Code::InvalidArguments, None);
        }
    };
    let scheme = match resolve_scheme(args, &path) {
        Ok(scheme) => scheme,
        Err(error) => return scheme_refusal(error),
    };

    let pending = if has(args, "--save-routes") {
        match existing_profile::PendingRoutes::prepare(&path, &scheme.maps) {
            Ok(pending) => Some(pending),
            Err(error) => return save_routes_refusal(&error, &scheme.maps),
        }
    } else {
        None
    };

    if scheme.name == "tags" {
        eprintln!(
            "sweep: tags are added only from an exported plan you have seen.\n\
             Export it:  sweep PATH --scheme tags --map EXT=TAG --export-plan FILE\n\
             Then apply: sweep apply --plan FILE --plan-digest SHA --yes"
        );
        return refusal(etude_cli_support::reason::Code::InvalidArguments, None);
    }
    let accept_all = has(args, "--yes");
    let only = value(args, "--only");
    let use_journal = !has(args, "--no-journal");

    if !accept_all && only.is_none() {
        eprintln!(
            "sweep: refusing to apply without an explicit choice.\n\
             Pass --yes to accept every group, or --only NAME for one."
        );
        return refusal(etude_cli_support::reason::Code::InvalidArguments, None);
    }

    let depth = match parse_depth(args) {
        Ok(d) => d,
        Err(msg) => {
            eprintln!("sweep: {msg}");
            return refusal(etude_cli_support::reason::Code::InvalidArguments, None);
        }
    };
    let cfg = ScanConfig {
        depth,
        allow_sync: has(args, "--allow-sync"),
        include_tagged: has(args, "--include-tagged"),
        grace: match since_flag(args) {
            Ok(g) => g.or(ScanConfig::default().grace),
            Err(msg) => {
                eprintln!("sweep: {msg}");
                return refusal(etude_cli_support::reason::Code::InvalidArguments, None);
            }
        },
        ..Default::default()
    };
    let outcome = match scan::scan_for_live_apply(&path, &cfg) {
        Ok(o) => o,
        Err(e) => {
            refuse_scan(&path, &e);
            return scan_exit_code(&e);
        }
    };
    if !scheme.maps.is_empty() && !use_journal {
        eprintln!(
            "sweep: routed moves with --no-journal are refused. A mapped move must stay\n\
             reversible; drop --no-journal."
        );
        return refusal(
            etude_cli_support::reason::Code::UnsafeInput,
            Some(etude_cli_support::reason::Subject::wait("input", None)),
        );
    }
    let mut p = if scheme.name == "existing" {
        plan::build_existing(&outcome, &scheme.maps)
    } else {
        plan::build_mapped(&outcome, &scheme.maps)
    };
    if let Some(msg) = only
        .as_deref()
        .and_then(|name| no_such_group(name, &p.groups, "here"))
    {
        eprintln!("sweep: {msg}");
        return refusal(etude_cli_support::reason::Code::InvalidArguments, None);
    }
    for g in &mut p.groups {
        g.accepted = match &only {
            Some(n) => &g.name == n,
            None => true,
        };
    }
    if pending.is_some()
        && scheme.maps.iter().any(|map| {
            !p.groups.iter().any(|group| {
                group.accepted && group.name == map.folder && !group.members.is_empty()
            })
        })
    {
        eprintln!(
            "sweep: routes were not saved: every --map route needs a selected group with at least one eligible file. Nothing was moved. Drop --save-routes to apply the selected groups only."
        );
        if let Some(pending) = &pending {
            routes_saved_detail(
                "not_saved",
                Some("refused_before_moves"),
                &[],
                &pending.requested,
                pending.profile(),
                false,
                false,
            );
        }
        return refusal(etude_cli_support::reason::Code::InvalidArguments, None);
    }
    print_include_tagged_warning(args);
    print_unreadable_warning(&p);
    print_left_alone_notes(&p, false);
    print_agent_named_note(&p);
    if p.moves() == 0 {
        etude_cli_support::envelope::no_undo_needed();
        eprintln!("sweep: nothing to apply.{}", hold_hint(&p));
        if p.only_too_recent() {
            etude_cli_support::envelope::reason(
                etude_cli_support::reason::Reason::nothing_to_do_later(
                    etude_cli_support::reason::Subject::wait("input", None),
                ),
            );
        }
        etude_cli_support::progress::finish_without_work("sweep apply", "apply", ExitCode::from(1));
        return ExitCode::from(1);
    }

    if !use_journal {
        println!("\n  --no-journal: nothing will be recorded, so `sweep undo` will not work.\n");
    }

    let sl = if use_journal {
        match sealer(scheme.maps.is_empty()) {
            Some(s) => Some(s),
            None => return ExitCode::from(2),
        }
    } else {
        None
    };

    let context = current_binding_context(&planning_profile(args, false, &scheme));
    let bound = match plan::BoundPlan::from_live_plan(p.clone(), context.clone()) {
        Ok(plan) => plan,
        Err(error) => {
            if let Some(seal) = sl.as_ref()
                && let Err(journal_error) =
                    etude_core::apply::record_refused_plan(&p, &context, seal)
            {
                eprintln!(
                    "sweep: {} changed just now, so nothing was moved, but the note about this attempt could not be saved: {}.",
                    etude_cli_support::shown(&p.root),
                    undo_record_write_reason(&journal_error)
                );
                etude_cli_support::envelope::status("refused");
                return failure(
                    etude_cli_support::reason::Code::JournalWriteFailed,
                    Some(etude_cli_support::reason::Subject::wait("journal", None)),
                );
            }
            let _ = error;
            eprintln!(
                "{}",
                etude_cli_support::problems::folder_changed_just_now("sweep", &p.root)
            );
            return refusal(
                etude_cli_support::reason::Code::PlanStale,
                Some(etude_cli_support::reason::Subject::wait("plan", None)),
            );
        }
    };
    run_apply_with_routes(&bound, &context, sl, "apply", pending)
}

/// `sweep propose --plan F --plan-digest SHA --proposal P --export-plan OUT`
///
/// Checks per-file decisions against an exported plan and writes a new bound
/// plan. Moves nothing. The library enforces every rule; this only reads the
/// files, maps a refusal to its code and prints.
fn cmd_propose(args: &[String]) -> ExitCode {
    use etude_cli_support::reason::{Code, Subject};
    use etude_core::proposal::Refusal;
    let (Some(file), Some(digest), Some(proposal), Some(out)) = (
        value(args, "--plan"),
        value(args, "--plan-digest"),
        value(args, "--proposal"),
        value(args, "--export-plan"),
    ) else {
        eprintln!("sweep: propose needs --plan, --plan-digest, --proposal and --export-plan");
        return refusal(Code::InvalidArguments, None);
    };
    if path_arg("propose", &args[1..]).is_ok_and(|path| path.is_some()) {
        return etude_cli_support::problems::plan_path_problem("sweep").report();
    }
    let bound = match load_saved_plan(Path::new(&file), &digest) {
        Ok(plan) => plan,
        Err(code) => return code,
    };
    let context = current_binding_context(&bound.context().profile);
    if let Err(error) = bound.validate(&context) {
        return etude_cli_support::problems::plan_check_problem("sweep", &bound.root, &error)
            .report();
    }
    let bytes = match read_proposal(Path::new(&proposal)) {
        Ok(bytes) => bytes,
        Err(error) => {
            let name = etude_cli_support::shown(Path::new(&proposal));
            return match error.kind() {
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory => {
                    eprintln!("sweep: no proposal file at {name}.");
                    failure(Code::MissingInput, Some(Subject::wait("input", None)))
                }
                std::io::ErrorKind::PermissionDenied => {
                    eprintln!(
                        "sweep: cannot read {name}: permission denied. Check it with `ls -l`."
                    );
                    failure(Code::PermissionDenied, Some(Subject::wait("input", None)))
                }
                std::io::ErrorKind::InvalidInput => {
                    eprintln!(
                        "sweep: {name} is not a regular file. --proposal needs the proposal file."
                    );
                    refusal(Code::InvalidArguments, None)
                }
                _ if etude_cli_support::problems::is_link_loop(&error) => {
                    eprintln!(
                        "sweep: {name} is a symbolic link. --proposal needs the file itself."
                    );
                    refusal(Code::InvalidArguments, None)
                }
                _ => {
                    eprintln!(
                        "sweep: cannot read {name}: {}.",
                        etude_cli_support::plain_io_error(&error)
                    );
                    refusal(Code::InvalidArguments, None)
                }
            };
        }
    };
    let proposed = match etude_core::proposal::propose(&bound, &bytes) {
        Ok(proposed) => proposed,
        Err(why) => {
            eprintln!("sweep: proposal refused: {why}. Nothing was written.");
            etude_cli_support::envelope::status("refused");
            let (code, subject) = match why {
                Refusal::InvalidArguments(_) => (Code::InvalidArguments, None),
                Refusal::PlanStale(_) => (Code::PlanStale, Some(Subject::wait("plan", None))),
                Refusal::UnknownItem(_) => (Code::UnknownItem, Some(Subject::wait("item", None))),
                Refusal::ItemAssignedTwice(_) => {
                    (Code::ItemAssignedTwice, Some(Subject::wait("item", None)))
                }
                Refusal::DestinationOutsideRoot(_) => (
                    Code::DestinationOutsideRoot,
                    Some(Subject::wait("destination", None)),
                ),
                Refusal::FolderMissing(_) => {
                    (Code::MissingInput, Some(Subject::wait("input", None)))
                }
                Refusal::DestinationExists(_) => (
                    Code::DestinationExists,
                    Some(Subject::wait("destination", None)),
                ),
            };
            return refusal(code, subject);
        }
    };
    if proposed.moves == 0 {
        eprintln!("sweep: the proposal moves nothing; no plan was written.");
        return ExitCode::from(1);
    }
    if let Err(error) = proposed.plan.export(Path::new(&out)) {
        return etude_cli_support::problems::plan_export_problem(
            "sweep",
            Path::new(&out),
            &proposed.plan.root,
            &error,
        )
        .report();
    }
    etude_cli_support::envelope::effect("items_proposed", proposed.moves);
    etude_cli_support::envelope::detail(etude_core::json::obj(&[
        ("kind", etude_core::json::str("proposal")),
        ("moves", etude_core::json::num(proposed.moves)),
        ("skipped", etude_core::json::num(proposed.skipped)),
        ("binding", proposed.plan.binding_json()),
    ]));
    println!(
        "Proposal accepted: {} to move, {} skipped. Nothing has been moved.",
        etude_cli_support::counted(proposed.moves, "file", "files"),
        proposed.skipped
    );
    println!("Plan written to {out}. Digest: {}", proposed.plan.digest());
    println!("Apply with: sweep apply --plan {out} --plan-digest DIGEST --yes");
    ExitCode::SUCCESS
}

/// Read a proposal file, at most `MAX_PROPOSAL_BYTES`, regular files only.
fn read_proposal(path: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // O_NOFOLLOW | O_NONBLOCK on macOS: never follow a link, never block
        // on a pipe.
        #[cfg(target_os = "macos")]
        options.custom_flags(0x100 | 0x4);
        #[cfg(target_os = "linux")]
        options.custom_flags(0x20000 | 0x800);
    }
    let file = scan::observe_read("proposal_file_open", options.open(path))?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "the proposal must be a regular file",
        ));
    }
    let mut bytes = Vec::new();
    scan::observe_read(
        "proposal_file_bytes",
        file.take(etude_core::proposal::MAX_PROPOSAL_BYTES + 1)
            .read_to_end(&mut bytes),
    )?;
    Ok(bytes)
}

/// The newest sweep journal that still has entries to reverse.
fn newest_undoable(
    sl: &dyn etude_core::journal::Sealer,
) -> Result<Option<etude_core::Journal>, RecordError> {
    sweep_journal_matching(sl, |_| true)
}

/// The newest other folder with an apply left to reverse. A record that will not
/// open is passed over here: naming the folder then gives its own honest answer.
fn other_folder_to_undo(sl: &dyn etude_core::journal::Sealer, root: &Path) -> Option<PathBuf> {
    etude_core::journal::candidates_by_recency("sweep")
        .ok()?
        .into_iter()
        .filter_map(|id| id.load("sweep", sl).ok())
        .find(|j| j.root != root && j.root.is_dir() && !journal_is_fully_undone(j, sl))
        .map(|j| j.root)
}

/// An undo record that could not be opened. No id means the folder of records
/// itself could not be used.
struct RecordError {
    id: Option<String>,
    error: etude_core::journal::JournalError,
}

/// Print why undo stopped before moving anything, and pick its code.
fn refuse_unreadable_record(
    record: &RecordError,
    sl: &dyn etude_core::journal::Sealer,
    asked: UndoAsked,
) -> ExitCode {
    use etude_cli_support::problems;
    match &record.id {
        Some(id) => {
            eprintln!(
                "sweep: cannot undo: {} Nothing was moved.",
                problems::record_open_problem("sweep", id, &record.error)
            );
            let lost = "sweep cannot say where the files from that apply are now, and cannot move them back for you. Look in the folder you applied.";
            if matches!(record.error, etude_core::journal::JournalError::Seal(_)) {
                eprintln!(
                    "Keep that record: if the key is wrong, the right key opens it again. If it was altered, {lost}"
                );
            } else {
                eprintln!("Without that record {lost}");
            }
            // An id asked for one record, so nothing else is opened to answer it.
            if asked != UndoAsked::Id {
                eprintln!("That record may be the newest apply, so undo stops there.");
                print_other_records(id, sl);
            }
        }
        None => eprintln!(
            "{}",
            problems::record_folder_problem("sweep", &record.error)
        ),
    }
    etude_cli_support::envelope::status("refused");
    failure(
        etude_cli_support::reason::Code::JournalUnreadable,
        Some(etude_cli_support::reason::Subject::wait("journal", None)),
    )
}

/// How many records that open are listed with their own step.
const RECORDS_LISTED: usize = 5;
/// How many more records that do not open are named, after the one that stopped undo.
const UNOPENED_LISTED: usize = 2;

/// The way past a record that will not open: every other record that does, each
/// with a step that opens only it. Opening a record changes nothing, and one
/// that is good under another key is left for that key.
fn print_other_records(blocked: &str, sl: &dyn etude_core::journal::Sealer) {
    use etude_cli_support::counted;
    use etude_core::json as j;
    let Ok(candidates) = etude_core::journal::candidates_unordered("sweep") else {
        eprintln!("The other undo records could not be listed, so none are shown here.");
        return;
    };
    let (mut live, mut unopened, mut opened) = (Vec::new(), Vec::new(), 0usize);
    for candidate in candidates.iter().filter(|c| c.id != blocked) {
        match candidate.load("sweep", sl) {
            Ok(journal) => {
                opened += 1;
                if !journal_is_fully_undone(&journal, sl) {
                    live.push(journal);
                }
            }
            Err(error) => unopened.push((candidate.id.clone(), error)),
        }
    }
    let newest_first = |id: &str| {
        (
            std::cmp::Reverse(etude_core::journal::creation_time(id)),
            id.to_owned(),
        )
    };
    live.sort_by_key(|journal| newest_first(&journal.id));
    unopened.sort_by_key(|(id, _)| newest_first(id));
    for (id, error) in unopened.iter().take(UNOPENED_LISTED) {
        eprintln!(
            "sweep: {}",
            etude_cli_support::problems::record_open_problem("sweep", id, error)
        );
    }
    if unopened.len() > UNOPENED_LISTED {
        eprintln!(
            "sweep: {} cannot be opened.",
            counted(
                unopened.len() - UNOPENED_LISTED,
                "more record",
                "more records"
            )
        );
    }
    let listed = &live[..live.len().min(RECORDS_LISTED)];
    let more = live.len() - listed.len();
    if listed.is_empty() {
        eprintln!(
            "{}",
            if opened == 0 {
                "No other undo record opens with this key."
            } else {
                "The other undo records that open have nothing left to undo."
            }
        );
    } else {
        eprintln!(
            "{} {}. Each step reverses only that apply:",
            if live.len() == 1 {
                "This undo record"
            } else {
                "These undo records"
            },
            if live.len() == 1 { "opens" } else { "open" },
        );
        for journal in listed {
            eprintln!(
                "  {}\n    {}",
                etude_cli_support::shown(&journal.root),
                undo_id_step(&journal.id)
            );
        }
        if more > 0 {
            eprintln!(
                "  {} not listed.",
                counted(more, "more undo record opens,", "more undo records open,")
            );
        }
    }
    etude_cli_support::envelope::detail(j::obj(&[
        (
            "records_that_open",
            j::arr(listed.iter().map(|journal| {
                j::obj(&[
                    ("operation_id", j::str(&journal.id)),
                    ("folder", j::path(&journal.root)),
                    ("command", j::str(&undo_id_step(&journal.id))),
                ])
            })),
        ),
        ("more_records_that_open", j::num(more)),
        ("other_records_that_do_not_open", j::num(unopened.len())),
    ]));
}

fn sweep_journal_matching(
    sl: &dyn etude_core::journal::Sealer,
    matches: impl Fn(&etude_core::Journal) -> bool,
) -> Result<Option<etude_core::Journal>, RecordError> {
    let ids = match etude_core::journal::candidates_by_recency("sweep") {
        Ok(ids) => ids,
        Err(etude_core::journal::JournalError::NotFound) => return Ok(None),
        Err(error) => return Err(RecordError { id: None, error }),
    };
    let mut journals = Vec::new();
    for id in ids {
        // An unreadable journal may account for these same destinations.
        // Never use a different history to bypass a journal we cannot read.
        let j = id.load("sweep", sl).map_err(|error| RecordError {
            id: Some(id.id.clone()),
            error,
        })?;
        if matches(&j) {
            if journals.is_empty() && !journal_is_fully_undone(&j, sl) {
                // Ordinary undo needs no older context. Contention is visible
                // as planned destinations occupied while their origins are gone.
                if etude_core::apply::unrecorded_moves(&j) == 0 {
                    return Ok(Some(j));
                }
                journals.push(j);
            } else if !journals.is_empty() {
                journals.push(j);
            }
        }
    }
    if let Some(index) = etude_core::apply::next_undo_journal(&journals, sl) {
        return Ok(Some(journals.remove(index)));
    }
    Ok(None)
}

/// Find the newest sweep journal whose root is `target` and that still has
/// something to reverse.
///
/// Issue #8. `undo` used to take the newest journal and nothing else, so
/// applying to two folders left the first unreachable: its journal sat on disk
/// for its full retention as an index of the user's filenames, with no
/// remaining way to use it. That is the exposure without the benefit.
///
/// `stash pop` already took a path for the same reason. This is the same shape.
fn sweep_journal_for_root(
    sl: &dyn etude_core::journal::Sealer,
    target: &Path,
) -> Result<Option<etude_core::Journal>, RecordError> {
    sweep_journal_matching(sl, |j| {
        j.root.canonicalize().is_ok_and(|root| root == target)
    })
}

/// One record, opened by its id. No other record is searched to choose it, so
/// one that will not open cannot stand in the way.
fn undo_by_id(id: &str, sl: &dyn etude_core::journal::Sealer) -> ExitCode {
    use etude_core::journal::{Journal, JournalError};
    let mut journal = match Journal::load_sealed_exact("sweep", id, sl) {
        Ok(journal) => journal,
        Err(JournalError::NotFound) => {
            let code = status_unknown(id);
            eprintln!("Nothing was moved. `sweep status FOLDER` lists the ids for a folder.");
            etude_cli_support::progress::finish_without_work(
                "sweep undo",
                "undo",
                ExitCode::from(1),
            );
            return code;
        }
        Err(error) => {
            return refuse_unreadable_record(
                &RecordError {
                    id: Some(id.to_owned()),
                    error,
                },
                sl,
                UndoAsked::Id,
            );
        }
    };
    etude_cli_support::envelope::recovery_operation(&journal.id);
    if journal_is_fully_undone(&journal, sl) {
        println!("\nNothing to undo. That apply was already restored, or moved nothing.");
        etude_cli_support::envelope::reason(etude_cli_support::reason::Reason::nothing_to_do());
        etude_cli_support::envelope::status("nothing_to_do");
        etude_cli_support::progress::finish_without_work("sweep undo", "undo", ExitCode::from(1));
        return ExitCode::from(1);
    }
    finish_undo(&mut journal, sl, UndoAsked::Id)
}

fn cmd_undo(args: &[String]) -> ExitCode {
    let Some(sl) = sealer(false) else {
        return ExitCode::from(2);
    };
    let code = undo_with(args, &sl);
    // After the undo, not before: when the person brings back the key that
    // opens an old record, the run that uses it must still find that record.
    prune_old_records(&sl);
    code
}

fn undo_with(args: &[String], sl: &KeychainSeal) -> ExitCode {
    // check_positionals has already refused a folder beside it and a bad id.
    if let Some(id) = undo_id(args) {
        return undo_by_id(id, sl);
    }

    // A named path reverses that folder's apply, whichever one it was.
    // Correct by coincidence before this: `undo` reads no value-bearing flag,
    // so "first argument not starting with -" happened to be the path. The
    // day someone gives undo a flag with a value, it would have broken the
    // same way review did.
    let named = match path_arg("undo", &args[1..]) {
        Ok(p) => p,
        Err(msg) => {
            eprintln!("sweep: {msg}");
            return refusal(etude_cli_support::reason::Code::InvalidArguments, None);
        }
    };
    if let Some(path) = named {
        if path.is_file() {
            return path_is_a_file(&path);
        }
        let target = match path.canonicalize() {
            Ok(target) => target,
            Err(error) => {
                return etude_cli_support::problems::folder_open_problem("sweep", &path, &error)
                    .report();
            }
        };
        return match sweep_journal_for_root(sl, &target) {
            Err(record) => refuse_unreadable_record(&record, sl, UndoAsked::Folder),
            Ok(Some(mut j)) => finish_undo(&mut j, sl, UndoAsked::Folder),
            Ok(None) => {
                eprintln!(
                    "sweep: nothing to undo for {}. No apply of that folder is still\nreversible.",
                    etude_cli_support::shown(&target)
                );
                etude_cli_support::progress::finish_without_work(
                    "sweep undo",
                    "undo",
                    ExitCode::from(1),
                );
                ExitCode::from(1)
            }
        };
    }

    // No path named: reverse the newest apply that still HAS something to
    // reverse, rather than the newest journal full stop. Otherwise a second
    // `undo` reports "already restored" about the one it just did and the
    // apply before it stays unreachable forever, which is issue #8 wearing a
    // different hat: the first fix let you name a folder, this one lets you
    // just run it twice.
    match newest_undoable(sl) {
        Ok(Some(mut j)) => return finish_undo(&mut j, sl, UndoAsked::Newest),
        Ok(None) => {}
        Err(record) => return refuse_unreadable_record(&record, sl, UndoAsked::Newest),
    }
    // Everything readable has been reversed, or nothing is on record. Which of
    // the two decides the answer below.
    let candidates = match etude_core::journal::candidates_by_recency("sweep") {
        Ok(candidates) => candidates,
        Err(etude_core::journal::JournalError::NotFound) => Vec::new(),
        Err(error) => {
            return refuse_unreadable_record(
                &RecordError { id: None, error },
                sl,
                UndoAsked::Newest,
            );
        }
    };
    let Some(newest) = candidates.into_iter().next() else {
        eprintln!("sweep: nothing to undo. No apply is on record.");
        etude_cli_support::envelope::reason(etude_cli_support::reason::Reason::nothing_to_do());
        etude_cli_support::progress::finish_without_work("sweep undo", "undo", ExitCode::from(1));
        return ExitCode::from(1);
    };
    let mut j = match newest.load("sweep", sl) {
        Ok(j) => j,
        Err(error) => {
            return refuse_unreadable_record(
                &RecordError {
                    id: Some(newest.id.clone()),
                    error,
                },
                sl,
                UndoAsked::Newest,
            );
        }
    };
    if journal_is_fully_undone(&j, sl) {
        // Say which operation this is about. The newest journal being already
        // restored is a fact about an earlier run, and a user who just applied
        // something reads it as a fact about that. If they used --no-journal
        // there is no record to reverse, and the honest thing is to say so
        // rather than let a reassuring sentence stand in for one.
        println!("\nNothing to undo. The most recent recorded apply was already restored.");
        println!(
            "If you just ran apply with --no-journal, nothing was recorded and undo cannot \nreverse it."
        );
        etude_cli_support::progress::finish_without_work("sweep undo", "undo", ExitCode::from(1));
        return ExitCode::from(1);
    }
    finish_undo(&mut j, sl, UndoAsked::Newest)
}

/// How the person chose the record. It decides which step can be printed: a
/// retry by folder could land on another apply of that folder.
#[derive(Clone, Copy, PartialEq, Eq)]
enum UndoAsked {
    Newest,
    Folder,
    Id,
}

/// The part of undo that is the same whether a path was named or not.
fn finish_undo(
    j: &mut etude_core::Journal,
    sl: &dyn etude_core::journal::Sealer,
    asked: UndoAsked,
) -> ExitCode {
    let mut progress = etude_cli_support::progress::Reporter::for_operation("sweep undo", "undo");
    let code = run_undo(j, sl, asked, &mut progress);
    progress.finish(code);
    code
}

/// What to say after an undo when the routes profile still holds a route for
/// files this undo put back. The journal does not record that an apply saved
/// routes, so the answer comes from the profile: a route whose extension and
/// folder match a restored file's. Undo never edits the profile.
fn routes_kept_line(j: &etude_core::Journal) -> Option<String> {
    let profile = existing_profile::ExistingProfile::load().ok()?;
    let kept = kept_routes(j, &profile.routes);
    if kept.is_empty() {
        return None;
    }
    let list = kept
        .iter()
        .map(|(ext, folder)| format!(".{ext} -> {}", etude_cli_support::shown_text(folder)))
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "  Saved routes stay in the routes profile ({list}). To drop one, delete its [[routes]] block from {}",
        etude_cli_support::shown(&existing_profile::profile_location())
    ))
}

/// The profile routes that match a file this journal's undo put back.
fn kept_routes(j: &etude_core::Journal, routes: &[(String, String)]) -> Vec<(String, String)> {
    routes
        .iter()
        .filter(|(ext, folder)| {
            j.entries.iter().any(|entry| {
                entry.state == etude_core::journal::EntryState::Reversed
                    && entry
                        .to
                        .extension()
                        .is_some_and(|e| e.to_string_lossy().to_lowercase() == *ext)
                    && entry.to.parent() == Some(j.root.join(folder).as_path())
            })
        })
        .cloned()
        .collect()
}

fn run_undo(
    j: &mut etude_core::Journal,
    sl: &dyn etude_core::journal::Sealer,
    asked: UndoAsked,
    progress: &mut etude_cli_support::progress::Reporter<std::io::Stderr>,
) -> ExitCode {
    // Pass the sealer: undo persists each reversal as it happens now, so a
    // kill partway through leaves a journal that agrees with the disk.
    // Said before the counts, because it changes what they mean: a journal
    // that lost its tail describes less than the apply actually did, so
    // "Restored N files" is a floor rather than the whole story. Recovering
    // quietly from damaged state would be its own version of the bug this
    // reporting exists to prevent.
    etude_cli_support::envelope::recovery_operation(&j.id);
    let tail_was_torn = j.progress_tail_damaged;
    // Only an undo that is going ahead says so. More than one lost record is
    // refused below, before anything moves.
    let unrecorded_folders = etude_core::apply::unrecorded_move_folders(j);
    if tail_was_torn && etude_core::apply::unrecorded_moves(j) <= 1 {
        println!(
            "\n  NOTE: this undo record was cut short, so its last move is not on record.\n\
             \x20       Undo puts back everything it lists, and checks the folder for the\n\
             \x20       file whose entry was lost."
        );
    }

    let tag_journal = j.entries.iter().any(|entry| entry.tag.is_some());
    let r = etude_core::apply::undo_with_structured_progress(j, sl, |event| progress.update(event));
    progress.settle();
    let root = j.root.clone();
    let folder = etude_cli_support::shown(&root);
    let retry = if asked == UndoAsked::Id {
        undo_id_step(&j.id)
    } else {
        undo_step(&root)
    };
    if r.unrecorded_moves > 1 && tag_journal {
        eprintln!(
            "sweep: cannot undo: this undo record is missing more than one entry. {n} files in {folder}\n\
             hold the tags apply was writing, but the record says it never wrote them.\n\
             Nothing was changed. You can edit those tags by hand in Finder.",
            n = r.unrecorded_moves
        );
        etude_cli_support::envelope::status("refused");
        return failure(
            etude_cli_support::reason::Code::OperationIncomplete,
            Some(etude_cli_support::reason::Subject::wait("journal", None)),
        );
    }
    if r.unrecorded_moves > 1 {
        eprintln!(
            "sweep: cannot undo: this undo record is missing more than one entry. {n} files are in\n\
             the folders below, but the record says they were never moved. Undo could reach only\n\
             the first of them, so it moved nothing. Nothing is lost.\n\
             {folders}\n\
             To restore them, move them back by hand into {folder}.",
            n = r.unrecorded_moves,
            folders = unrecorded_folders
                .iter()
                .map(|path| format!("  {}", etude_cli_support::shown(path)))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        etude_cli_support::envelope::status("refused");
        return failure(
            etude_cli_support::reason::Code::OperationIncomplete,
            Some(etude_cli_support::reason::Subject::wait("journal", None)),
        );
    }
    // Report what actually happened before anything about the outcome: this
    // count is real even when `r.error` is set below.
    if r.directories_left_for_journals > 0 {
        println!(
            "Left {} that another undo record may still need.",
            etude_cli_support::counted(
                r.directories_left_for_journals,
                "empty folder",
                "empty folders"
            )
        );
        if asked == UndoAsked::Id {
            println!(
                "When that other record is done with {}, remove {} with: {retry}",
                if r.directories_left_for_journals == 1 {
                    "it"
                } else {
                    "them"
                },
                if r.directories_left_for_journals == 1 {
                    "it"
                } else {
                    "them"
                },
            );
        }
    }
    if tag_journal {
        tag_output::undo_detail(&r);
    } else {
        etude_cli_support::envelope::detail(etude_core::json::obj(&[
            ("restored", etude_core::json::num(r.restored)),
            (
                "directories_left_for_journals",
                etude_core::json::num(r.directories_left_for_journals),
            ),
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
    }
    // Any item not restored leaves the request unfinished, whatever the cause.
    let left_behind = !r.skipped_changed.is_empty()
        || !r.skipped_occupied.is_empty()
        || !r.skipped_missing.is_empty()
        || !r.tags_refused.is_empty();
    if left_behind || r.error.is_some() {
        etude_cli_support::envelope::status("incomplete");
    }
    etude_cli_support::envelope::effect("items_restored", r.restored);
    etude_cli_support::envelope::effect("changed_items_left", r.skipped_changed.len());
    etude_cli_support::envelope::effect("occupied_items_left", r.skipped_occupied.len());
    etude_cli_support::envelope::effect("missing_items", r.skipped_missing.len());
    if tag_journal {
        etude_cli_support::envelope::effect("tags_restored", r.tags_restored);
        etude_cli_support::envelope::effect("tag_files_left", r.tags_refused.len());
        tag_output::print_undo_headline(&r);
    } else {
        let left = r.skipped_changed.len() + r.skipped_occupied.len() + r.skipped_missing.len();
        if left > 0 {
            let total = r.restored + left;
            println!(
                "\nRestored {} of {} {}.",
                r.restored,
                total,
                if total == 1 { "file" } else { "files" }
            );
        } else {
            println!(
                "\nRestored {}.",
                etude_cli_support::counted(r.restored, "file", "files")
            );
        }
    }
    if !tag_journal && let Some(line) = routes_kept_line(j) {
        println!("{line}");
    }
    if !r.skipped_changed.is_empty() {
        if tag_journal {
            println!(
                "  {} left because identity or write ownership could not be verified:",
                etude_cli_support::counted(
                    r.skipped_changed.len(),
                    "tag file was",
                    "tag files were"
                )
            );
        } else {
            println!(
                "  {} since apply and {} left alone:",
                etude_cli_support::counted(
                    r.skipped_changed.len(),
                    "file changed",
                    "files changed"
                ),
                if r.skipped_changed.len() == 1 {
                    "was"
                } else {
                    "were"
                },
            );
        }
        for p in &r.skipped_changed {
            println!("    {}", etude_core::redact::path(p));
        }
    }
    tag_output::print_undo_refusals(&r);
    if !r.skipped_occupied.is_empty() {
        let one = r.skipped_occupied.len() == 1;
        let folders: Vec<&PathBuf> = r
            .skipped_occupied_origins
            .iter()
            .filter(|p| std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_dir()))
            .collect();
        let taker = if folders.len() == r.skipped_occupied.len() {
            if one {
                "another folder"
            } else {
                "other folders"
            }
        } else if folders.is_empty() {
            if one { "another file" } else { "other files" }
        } else {
            "other files or folders"
        };
        println!(
            "  {} not put back because {} original {} taken by {taker}. {} still at:",
            etude_cli_support::counted(r.skipped_occupied.len(), "file was", "files were"),
            if one { "its" } else { "their" },
            if one { "place is" } else { "places are" },
            if one { "It is" } else { "They are" },
        );
        for p in &r.skipped_occupied {
            println!("    {}", etude_core::redact::path(p));
        }
        if folders.len() < r.skipped_occupied.len() {
            println!(
                "  Move the other {} away, then run: {retry}",
                if r.skipped_occupied.len() - folders.len() == 1 {
                    "file"
                } else {
                    "files"
                },
            );
        }
        if !folders.is_empty() {
            for p in folders.iter().take(3) {
                println!(
                    "  Another folder holds a file's original place, in {}, under the name {}.",
                    p.parent().map(etude_cli_support::shown).unwrap_or_default(),
                    etude_core::redact::path(Path::new(p.file_name().unwrap_or_default())),
                );
            }
            println!(
                "  If {} empty, remove {}. If not, move {} away. Then run: {retry}",
                if folders.len() == 1 {
                    "that folder is"
                } else {
                    "those folders are"
                },
                if folders.len() == 1 { "it" } else { "them" },
                if folders.len() == 1 { "it" } else { "them" },
            );
        }
    }
    if !r.skipped_missing.is_empty() {
        let mut folders: Vec<&Path> = r
            .skipped_missing
            .iter()
            .filter_map(|path| path.parent())
            .collect();
        folders.sort();
        folders.dedup();
        let named: Vec<String> = folders
            .iter()
            .take(3)
            .map(|path| etude_cli_support::shown(path))
            .collect();
        let more = folders.len().saturating_sub(named.len());
        let one = r.skipped_missing.len() == 1;
        println!(
            "  {} not where apply left {} ({}{}).",
            etude_cli_support::counted(r.skipped_missing.len(), "file is", "files are"),
            if one { "it" } else { "them" },
            named.join(", "),
            if more > 0 {
                format!(
                    ", and {} more",
                    etude_cli_support::counted(more, "folder", "folders")
                )
            } else {
                String::new()
            },
        );
        println!("  If you moved that folder, move it back and run: {retry}");
        println!(
            "  If a later apply moved {}, undo that apply first, then run the same step. `sweep status {}` lists only the applies of that folder, not of a folder inside it or around it.",
            if one { "it" } else { "them" },
            etude_cli_support::quoted_path(&root),
        );
        println!(
            "  If you deleted {}, there is nothing to restore.",
            if one { "it" } else { "them" }
        );
        // Only `sweep undo` with no folder keeps stopping at this record. Anyone
        // who named a folder is not held up by it.
        if asked == UndoAsked::Newest
            && let Some(older) = other_folder_to_undo(sl, &root)
        {
            println!(
                "  sweep undo with no folder keeps stopping at this record. To undo the apply of {}, run: {}",
                etude_cli_support::shown(&older),
                undo_step(&older)
            );
        }
    }
    if r.already_reversed > 0 {
        println!(
            "  {} already restored by an earlier run that did not finish.",
            etude_cli_support::counted(r.already_reversed, "file was", "files were")
        );
    }
    if !r.reconciled.is_empty() {
        // Different from healed: nothing was reachable by two names here. An
        // earlier undo moved these home and was killed before it could write
        // that down, so this run only had to agree with the disk.
        println!(
            "  {} already home from an interrupted restore; the undo record now says so.",
            etude_cli_support::counted(r.reconciled.len(), "file was", "files were")
        );
    }
    if !r.healed.is_empty() {
        // Say it rather than fold it into `restored`. Nothing moved: a crash
        // between the link and the unlink had left one file answering to two
        // names, and the extra name is gone now. A user who is told "restored"
        // would go looking for a move that never happened.
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
    let saved = j.save_sealed(sl);
    let next_undo_corrects =
        "The next sweep undo will see which files are already home and correct the record.";
    if let Some(err) = r.error {
        let items: Vec<PathBuf> = j
            .entries
            .iter()
            .filter(|entry| entry.is_moved())
            .flat_map(|entry| [entry.to.clone(), entry.from.clone()])
            .collect();
        let tag_undo = j.entries.iter().any(|entry| entry.tag.is_some());
        let (doing, work) = if tag_undo {
            (
                "could not put back every file's old tags",
                etude_cli_support::problems::LockedWork::TagUndo,
            )
        } else {
            (
                "could not put every file back",
                etude_cli_support::problems::LockedWork::Move,
            )
        };
        refuse_apply(&err, &root, doing, &items, work);
        match saved {
            Ok(()) => {
                if let etude_core::apply::ApplyError::DestinationDirectoryChanged(path) = &err {
                    eprintln!(
                        "Move it back to {}, then run: {retry}",
                        etude_cli_support::shown(path),
                    );
                } else {
                    eprintln!("To continue from where this stopped, run: {retry}");
                }
            }
            Err(save_err) => eprintln!(
                "sweep: the undo record could not be updated either: {}. {next_undo_corrects}",
                etude_cli_support::plain_journal_error(&save_err)
            ),
        }
        if matches!(&err, etude_core::apply::ApplyError::Io(source) if etude_cli_support::problems::disk_full(source))
        {
            return failure(
                etude_cli_support::reason::Code::InsufficientSpace,
                Some(etude_cli_support::reason::Subject::wait(
                    "destination",
                    None,
                )),
            );
        }
        return failure(
            etude_cli_support::reason::Code::OperationIncomplete,
            Some(etude_cli_support::reason::Subject::wait("journal", None)),
        );
    }
    if let Err(save_err) = saved {
        eprintln!(
            "sweep: the restored files are back, but the undo record could not be updated: {}. {next_undo_corrects}",
            etude_cli_support::plain_journal_error(&save_err)
        );
        return failure(
            etude_cli_support::reason::Code::JournalWriteFailed,
            Some(etude_cli_support::reason::Subject::wait("journal", None)),
        );
    }
    if left_behind {
        // Exit 0 means the whole request was done, and this one was not.
        return failure(
            etude_cli_support::reason::Code::OperationIncomplete,
            Some(etude_cli_support::reason::Subject::wait("journal", None)),
        );
    }
    ExitCode::SUCCESS
}

/// Who owns a `{tool}-{id}.journal` filename, same `{tool}-` prefix convention
/// as `journal::latest_id` / `ids_by_recency`. Non-journals are ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JournalOwner {
    Sweep,
    Stash,
    Other,
}

fn classify_journal_filename(name: &str) -> JournalOwner {
    let Some(stem) = name.strip_suffix(".journal") else {
        return JournalOwner::Other;
    };
    if stem.starts_with("sweep-") {
        JournalOwner::Sweep
    } else if stem.starts_with("stash-") {
        JournalOwner::Stash
    } else {
        JournalOwner::Other
    }
}

/// Whether forget may destroy the shared keychain key, and how.
///
/// Stash and sweep share one key. Destroying it while a stash journal exists
/// strands that stash permanently. So we require an informed yes, or `--yes`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ForgetKeyGate {
    /// No live stash journal, or `--yes` already authorised destruction.
    Proceed,
    /// Stash present, TTY available: warn and ask.
    Ask,
    /// Stash present, no TTY and no `--yes`: refuse.
    Refuse,
}

fn forget_key_gate(args: &[String], stash_present: bool, is_tty: bool) -> ForgetKeyGate {
    if !stash_present {
        return ForgetKeyGate::Proceed;
    }
    if has(args, "--yes") {
        return ForgetKeyGate::Proceed;
    }
    if is_tty {
        ForgetKeyGate::Ask
    } else {
        ForgetKeyGate::Refuse
    }
}

fn forget_shared_key_consent() -> bool {
    use std::io::{self, Write};

    println!(
        "\n\
  sweep and stash share one keychain key.\n\
  Destroying it will make any live stash permanently unrecoverable.\n\
  The holding directory stays. Nothing can open it.\n"
    );
    print!("  destroy the shared key anyway? [y/N] > ");
    let _ = io::stdout().flush();
    let mut line = String::new();
    if io::stdin().read_line(&mut line).unwrap_or(0) == 0 {
        return false; // EOF is not consent
    }
    line.trim().eq_ignore_ascii_case("y")
}

fn destroy_shared_key() -> ExitCode {
    if std::env::var_os("ETUDE_JOURNAL_KEY").is_some() {
        // The journals are already gone, so this is not a refusal.
        eprintln!(
            "sweep: a supplied key is in use. Remove every retained copy yourself; forget cannot destroy it."
        );
        return ExitCode::SUCCESS;
    }
    if etude_keep::destroy_key() {
        println!("Destroyed the journal key in the keychain.");
        println!("Undo is no longer possible for any past run.");
        ExitCode::SUCCESS
    } else {
        eprintln!(
            "sweep: could not confirm the journal key was destroyed. It may still exist.\n\
             Check it in Keychain Access, under the name {}.",
            etude_keep::SERVICE
        );
        failure(
            etude_cli_support::reason::Code::JournalKeyUnavailable,
            Some(etude_cli_support::reason::Subject::wait("journal", None)),
        )
    }
}

fn refuse_shared_key_destroy(removed_nothing: bool) -> ExitCode {
    eprintln!(
        "sweep: refusing to destroy the shared journal key while a stash\n\
         record exists, because that stash could never be restored.{}\n\
         Pass `sweep forget --yes` to destroy it anyway.",
        if removed_nothing {
            " Nothing was removed."
        } else {
            ""
        }
    );
    // `forget` takes no --json, so this reason is never printed. It is the
    // same refusal `apply` gives without --yes: an explicit choice is missing.
    refusal(etude_cli_support::reason::Code::InvalidArguments, None)
}

/// Why forget stops for stash records, and what the person can do about them.
///
/// Forget removes only sweep's records. A stash record may be the only way
/// back for files in a holding folder, and no tool removes one it cannot open:
/// sweep's prune skips stash records and `stash pop` drops only records that
/// open and hold nothing. So a record nobody can open stays until the person
/// removes it, and this says how, without reading the key to sort them.
fn explain_kept_stash_records(records: &[PathBuf], shown_dir: &str) {
    const LISTED: usize = 10;
    let key = if std::env::var_os("ETUDE_JOURNAL_KEY").is_some() {
        "the key in ETUDE_JOURNAL_KEY, which forget cannot destroy, or the keychain key"
    } else {
        "ETUDE_JOURNAL_KEY, or the keychain key that forget destroys if you agree"
    };
    eprintln!(
        "sweep: {} in {shown_dir}. Forget keeps them, because they may still restore files.\n\
         \x20      See which still open: stash status --all\n\
         \x20      One that opens: restore it with `stash pop --id ID`, using the key it was\n\
         \x20      made with ({key}).\n\
         \x20      One that `stash status --all` says cannot be opened is never removed by\n\
         \x20      these tools. Once you are sure its files are back, remove that file by\n\
         \x20      hand. Each record is the file stash-ID.journal:",
        etude_cli_support::counted(records.len(), "stash record is", "stash records are")
    );
    for path in records.iter().take(LISTED) {
        eprintln!("         rm {}", etude_cli_support::quoted_path(path));
    }
    if records.len() > LISTED {
        eprintln!(
            "         ({} more in {shown_dir}, named stash-ID.journal)",
            records.len() - LISTED
        );
    }
}

/// Said before `--yes` destroys the keychain key while stash records exist:
/// the records it made stop opening, and their files stay where stash put them.
fn warn_stash_records_lose_their_key(count: usize) {
    eprintln!(
        "sweep: --yes destroys the keychain key now. {} made with it will no longer\n\
         \x20      open, and their files stay in the hidden .stash- folders. To keep them,\n\
         \x20      do not use --yes: pop each first with `stash pop --id ID`, then forget.",
        etude_cli_support::counted(count, "stash record", "stash records")
    );
}

fn cmd_forget(args: &[String]) -> ExitCode {
    use std::io::IsTerminal;

    use etude_cli_support::reason::{Code, Subject};
    let dir = etude_core::journal::state_dir();
    let shown_dir = etude_cli_support::shown(&dir);
    let mut n = 0;
    let mut stash_records = Vec::new();
    let mut not_removed = 0;
    let mut first_error = None;
    let mut sweep_journals = Vec::new();
    match std::fs::read_dir(&dir) {
        Ok(rd) => {
            for e in rd {
                let e = match e {
                    Ok(e) => e,
                    Err(error) => {
                        not_removed += 1;
                        first_error.get_or_insert(error);
                        continue;
                    }
                };
                let name = e.file_name().to_string_lossy().into_owned();
                match classify_journal_filename(&name) {
                    JournalOwner::Sweep => sweep_journals.push(e.path()),
                    JournalOwner::Stash => stash_records.push(e.path()),
                    JournalOwner::Other => {}
                }
            }
        }
        // No folder means no journals. That one case is a true "removed none".
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        // Whether a stash relies on the key is unknown, so the key stays too.
        Err(error) => {
            eprintln!(
                "sweep: could not read {shown_dir}: {}. Journals were not removed, and the key was left alone.",
                etude_cli_support::plain_io_error(&error)
            );
            return failure(
                if error.kind() == std::io::ErrorKind::PermissionDenied {
                    Code::PermissionDenied
                } else {
                    Code::JournalUnreadable
                },
                Some(Subject::wait(
                    if error.kind() == std::io::ErrorKind::PermissionDenied {
                        "input"
                    } else {
                        "journal"
                    },
                    None,
                )),
            );
        }
    }
    // Decide before removing anything: a command that ends in this refusal must
    // leave everything as it found it.
    stash_records.sort();
    let gate = forget_key_gate(
        args,
        !stash_records.is_empty(),
        std::io::stdin().is_terminal(),
    );
    if !stash_records.is_empty() {
        explain_kept_stash_records(&stash_records, &shown_dir);
    }
    if gate == ForgetKeyGate::Refuse {
        return refuse_shared_key_destroy(true);
    }
    // Sweep's own journals only, regardless of the answer to the question below.
    for path in sweep_journals {
        match std::fs::remove_file(path) {
            Ok(()) => n += 1,
            Err(error) => {
                not_removed += 1;
                first_error.get_or_insert(error);
            }
        }
    }
    println!(
        "Removed {} from {shown_dir}.",
        etude_cli_support::counted(n, "journal", "journals")
    );
    if let Some(error) = first_error {
        eprintln!(
            "sweep: could not remove {} from {shown_dir}: {}. The key was left alone.",
            etude_cli_support::counted(not_removed, "journal", "journals"),
            etude_cli_support::plain_io_error(&error)
        );
        return failure(
            Code::OperationIncomplete,
            Some(Subject::wait("journal", None)),
        );
    }

    match gate {
        ForgetKeyGate::Proceed => {
            if !stash_records.is_empty() && std::env::var_os("ETUDE_JOURNAL_KEY").is_none() {
                warn_stash_records_lose_their_key(stash_records.len());
            }
            destroy_shared_key()
        }
        ForgetKeyGate::Ask if forget_shared_key_consent() => destroy_shared_key(),
        ForgetKeyGate::Ask | ForgetKeyGate::Refuse => refuse_shared_key_destroy(false),
    }
}

fn verify() -> ExitCode {
    let dir = etude_core::journal::state_dir();
    let count = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.file_name().to_string_lossy().ends_with(".journal"))
                .count()
        })
        .unwrap_or(0);

    println!(
        "
sweep {v}

  What is compiled in
    content inspection     yes, but OFF unless --inspect-content AND you
                           consent at a separate prompt. --yes does not cover it.
    network code           none. etude-core has 0 dependencies; the binary
                           links no socket symbols.

  What it reads
    to decide              filesystem and Finder organization metadata only;
                           never file contents
    when it moves a file   the first and last 4 KiB, so undo can tell if that
                           file changed
    Finder tags            whether one is present, never its name; only
                           --scheme tags reads a file's tag list, to add to
                           it. Finder comments are never read
    one exception          if a tag lands on a file while sweep is moving it
                           and the file cannot be moved back because its
                           original place has been taken, sweep leaves it
                           where it is, counts it as an item it could not
                           return, and records that in the journal

  What is on this machine
    journals held          {count} in {dir}
    journal encryption     XChaCha20-Poly1305, keychain or ETUDE_JOURNAL_KEY
    journal path synced    {synced}
    journal expiry         {ttl} days

  What is NOT implemented. Do not expect it to work
    x                      extracting files out of a group during review
    PDF / Office / archive parsing   --inspect-content reads plain text only
    content grouping       what sweep reads can only make it refuse MORE
                           files; it never affects where anything moves

  Verify these claims yourself
    cargo test                     the full suite
    scripts/no-network-test.sh     the suite with sockets denied by the OS

  Destroy sweep's journals:  sweep forget
    The key is shared with stash, so it goes only when no stash journal is
    holding files, and you are asked first.
",
        v = env!("CARGO_PKG_VERSION"),
        count = count,
        dir = dir.display(),
        synced = if scan::is_synced(&dir) {
            "YES, move it"
        } else {
            "no"
        },
        ttl = etude_core::journal::TTL_DAYS,
    );
    ExitCode::SUCCESS
}

#[cfg(unix)]
fn disable_core_dumps() {
    // SAFETY: setrlimit with a valid resource and a zeroed limit is well-defined.
    unsafe {
        let lim = RLimit { cur: 0, max: 0 };
        setrlimit(RLIMIT_CORE, &lim);
    }
}
#[cfg(not(unix))]
fn disable_core_dumps() {}

#[cfg(unix)]
#[repr(C)]
struct RLimit {
    cur: u64,
    max: u64,
}
#[cfg(unix)]
const RLIMIT_CORE: i32 = 4;
#[cfg(unix)]
unsafe extern "C" {
    fn setrlimit(resource: i32, rlim: *const RLimit) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_named_note_escapes_control_characters_in_every_name() {
        let group = |name: &str| plan::Group {
            name: name.to_owned(),
            signal: plan::Signal::Mapped {
                ext: "bpy".into(),
                created: true,
                saved: false,
            },
            members: Vec::new(),
            accepted: true,
        };
        let plan = plan::Plan::display_only(
            PathBuf::from("/x"),
            vec![group("A\x1b[2Jone"), group("B\ntwo")],
        );
        assert_eq!(
            agent_named_note(&plan).unwrap(),
            "\n  2 folder names were chosen by your agent, not derived from your files: A?[2Jone, B?two"
        );
    }

    // unknown flags must fail even after a valid path, or consent can move files.
    #[test]
    fn an_unrecognised_flag_to_apply_is_rejected_not_dropped() {
        for typo in ["--dry-run", "-n", "--force", "--all"] {
            // Against apply's FLAGS, not against the command names. A blind
            // rename turned this into a comparison of typos to "apply",
            // "undo" and so on, which is true for every input and therefore
            // asserted nothing. A review caught it.
            let (_, apply_flags) = COMMAND_FLAGS
                .iter()
                .find(|(c, _)| *c == "apply")
                .expect("apply is declared");
            assert!(
                !apply_flags.iter().any(|(name, _)| *name == typo),
                "{typo} would be accepted by apply"
            );
            assert!(
                check_flags("apply", &[typo.to_string()]).is_err(),
                "{typo} must be refused at dispatch as well"
            );
        }
        let args = [
            "some/path".to_string(),
            "--dry-run".to_string(),
            "--yes".to_string(),
        ];
        assert!(apply_path(&args).is_err());
    }

    // apply must never infer consent to operate on the process's directory.
    #[test]
    fn apply_without_a_path_is_an_error_not_the_current_directory() {
        assert!(apply_path(&["--yes".to_string()]).is_err());
    }

    // wrong depth must not silently become the shallowest scan.
    #[test]
    fn parse_depth_rejects_garbage_and_out_of_range() {
        assert_eq!(parse_depth(&[]), Ok(1));
        assert_eq!(
            parse_depth(&["--depth".to_string(), "8".to_string()]),
            Ok(8)
        );
        assert!(parse_depth(&["--depth".to_string(), "999".to_string()]).is_err());
        assert!(parse_depth(&["--depth".to_string(), "0".to_string()]).is_err());
        assert!(parse_depth(&["--depth".to_string(), "banana".to_string()]).is_err());
    }

    #[test]
    fn folders_is_the_explicit_and_default_scheme() {
        assert_eq!(parse_scheme(&[]), Ok(()));
        assert_eq!(parse_scheme(&["--scheme".into(), "folders".into()]), Ok(()));
        assert_eq!(
            parse_scheme(&["--scheme".into(), "existing".into()]),
            Ok(())
        );
        // `tags` is built, on macOS only: it writes a macOS file attribute.
        assert_eq!(
            parse_scheme(&["--scheme".into(), "tags".into()]).is_ok(),
            cfg!(target_os = "macos")
        );
        assert!(parse_scheme(&["--scheme".into(), "labels".into()]).is_err());
    }

    /// A grace window the parser could not read must not become the default.
    ///
    /// `--since` decides which files are held back, so silently substituting
    /// 24h for a value someone typed tells them their instruction was obeyed
    /// when it was discarded. Same rule as parse_depth above.
    #[test]
    fn since_rejects_garbage_rather_than_falling_back_to_the_default() {
        let s = |v: &str| since_flag(&["--since".to_string(), v.to_string()]);
        assert_eq!(s("0"), Ok(Some(std::time::Duration::ZERO)));
        assert_eq!(
            s("6h"),
            Ok(Some(std::time::Duration::from_secs(6 * 60 * 60)))
        );
        assert_eq!(
            s("2d"),
            Ok(Some(std::time::Duration::from_secs(2 * 24 * 60 * 60)))
        );
        assert!(s("banana").is_err());
        assert!(s("6hh").is_err());
        assert!(s("").is_err());
        // present but with nothing after it
        assert!(since_flag(&["--since".to_string()]).is_err());
        // Release builds wrapped this and swept with a window nobody chose.
        assert!(s("18446744073709551615d").is_err());
        assert!(s("999999999999999999999").is_err());
        // The boundary the message names must be the boundary enforced.
        let most_days = u64::MAX / (24 * 60 * 60);
        assert!(s(&format!("{most_days}d")).is_ok());
        assert!(s(&format!("{}d", most_days + 1)).is_err());
        let most_hours = u64::MAX / 3600;
        assert!(s(&format!("{most_hours}h")).is_ok());
        assert!(s(&format!("{}h", most_hours + 1)).is_err());
    }

    /// What someone typed beats what their shell exported.
    #[test]
    fn an_explicit_since_outranks_the_harness_environment_variable() {
        // SAFETY: single-threaded test, variable removed before returning.
        unsafe { std::env::set_var("SWEEP_GRACE_SECS", "0") };
        let typed = since_flag(&["--since".to_string(), "6h".to_string()]);
        let untyped = since_flag(&[]);
        unsafe { std::env::remove_var("SWEEP_GRACE_SECS") };
        assert_eq!(typed, Ok(Some(std::time::Duration::from_secs(6 * 60 * 60))));
        assert_eq!(untyped, Ok(Some(std::time::Duration::ZERO)));
    }

    /// The flag a user can type has to be in the help they can read.
    #[test]
    fn every_user_facing_flag_appears_in_usage() {
        for (_, flags) in COMMAND_FLAGS {
            for (name, _) in *flags {
                assert!(
                    USAGE.contains(name),
                    "{name} is accepted but undocumented in `sweep help`"
                );
            }
        }
    }

    /// Every command that takes a path finds the same path, whichever side of
    /// the flags it sits on.
    ///
    /// Derived from COMMAND_FLAGS rather than hand-listed, so a command added
    /// later is covered without anyone remembering to cover it. That is the
    /// point of the fix: `apply` had this right and a test guarding it, while
    /// the scan path and `review` each hand-rolled the search and got it
    /// wrong in different ways.
    #[test]
    fn every_command_finds_the_path_on_either_side_of_its_flags() {
        for (cmd, flags) in COMMAND_FLAGS {
            if matches!(*cmd, "status" | "undo") {
                continue; // --id is a lookup selector, not a path option.
            }
            let Some((flag, takes_value)) = flags.iter().find(|(_, v)| *v) else {
                continue; // no value-bearing flag: the ambiguity cannot arise
            };
            assert!(takes_value);
            let value = match *flag {
                "--depth" => "2",
                "--since" => "1d",
                "--only" => "Documents",
                "--plan" | "--export-plan" | "--proposal" => "approved.plan",
                "--plan-digest" => {
                    "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                }
                other => panic!("{other} takes a value but this test has none for it"),
            };

            let after: Vec<String> = vec![
                "/tmp/somewhere".into(),
                (*flag).to_string(),
                value.to_string(),
            ];
            let before: Vec<String> = vec![
                (*flag).to_string(),
                value.to_string(),
                "/tmp/somewhere".into(),
            ];

            assert_eq!(
                path_arg(cmd, &after).expect("path after flags"),
                Some(PathBuf::from("/tmp/somewhere")),
                "{cmd}: the path was lost when it came before {flag}"
            );
            assert_eq!(
                path_arg(cmd, &before).expect("path before flags"),
                Some(PathBuf::from("/tmp/somewhere")),
                "{cmd}: {flag}'s value {value:?} was mistaken for the path, or \
                 the path was ignored and the current directory used instead"
            );
        }
    }

    // a value consumed by a flag is not a positional path.
    #[test]
    fn apply_depth_value_is_not_mistaken_for_the_path() {
        let args = [
            "--depth".to_string(),
            "3".to_string(),
            "--yes".to_string(),
            "/tmp/somewhere".to_string(),
        ];
        assert_eq!(apply_path(&args), Ok(PathBuf::from("/tmp/somewhere")));
    }

    // the command-level guard must return before scanning or moving the cwd.
    #[test]
    fn apply_with_no_path_never_touches_the_current_directory() {
        let root =
            std::env::temp_dir().join(format!("sweep-apply-cwd-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        fixtures::build(&root).unwrap();

        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(&root).unwrap();
        let _ = cmd_apply(&[
            "apply".to_string(),
            "--yes".to_string(),
            "--no-journal".to_string(),
        ]);
        std::env::set_current_dir(original).unwrap();

        let untouched = root.join("IMG_4400.HEIC").exists();
        // The fixture also makes a folder beside the root. Remove both before
        // asserting, so a failure does not leave them behind.
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(fixtures::outside_dir(&root));
        assert!(
            untouched,
            "a camera-burst file moved: apply operated on the cwd with no PATH given"
        );
    }

    // forget must recognise sweep's own journals by the `{tool}-` prefix.
    #[test]
    fn a_sweep_journal_filename_is_recognised_as_sweep_owned() {
        assert_eq!(
            classify_journal_filename("sweep-a1b2c3.journal"),
            JournalOwner::Sweep
        );
        assert_eq!(
            classify_journal_filename("notes.txt"),
            JournalOwner::Other,
            "non-journal mistaken for a journal"
        );
        assert_eq!(
            classify_journal_filename("sweep.journal"),
            JournalOwner::Other,
            "missing tool-id hyphen still counted as sweep"
        );
    }

    // a live stash journal is what gates key destruction; must be detected.
    #[test]
    fn a_stash_journal_filename_signals_stash_present() {
        assert_eq!(
            classify_journal_filename("stash-deadbeef.journal"),
            JournalOwner::Stash
        );
    }

    // --yes is the only non-interactive way past the shared-key gate.
    #[test]
    fn forget_yes_bypasses_the_shared_key_prompt() {
        let yes = ["forget".to_string(), "--yes".to_string()];
        let no = ["forget".to_string()];

        assert_eq!(
            forget_key_gate(&yes, true, false),
            ForgetKeyGate::Proceed,
            "--yes did not authorise destruction with a stash present"
        );
        assert_eq!(
            forget_key_gate(&no, true, true),
            ForgetKeyGate::Ask,
            "TTY + stash should ask, not proceed silently"
        );
        assert_eq!(
            forget_key_gate(&no, true, false),
            ForgetKeyGate::Refuse,
            "no TTY and no --yes must refuse, not destroy"
        );
        assert_eq!(
            forget_key_gate(&no, false, false),
            ForgetKeyGate::Proceed,
            "no stash still gated the key"
        );
    }

    // classification over a real state dir must find stash without deleting it.
    #[test]
    fn forget_classifies_journals_in_the_state_dir() {
        let state = std::env::temp_dir().join(format!(
            "sweep-forget-classify-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&state);
        std::fs::create_dir_all(&state).unwrap();
        std::fs::write(state.join("sweep-aaa.journal"), b"s").unwrap();
        std::fs::write(state.join("stash-bbb.journal"), b"t").unwrap();
        std::fs::write(state.join("noise.txt"), b"x").unwrap();

        unsafe { std::env::set_var("ETUDE_STATE_DIR", &state) };

        let dir = etude_core::journal::state_dir();
        let mut sweep = 0;
        let mut stash_present = false;
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            match classify_journal_filename(&e.file_name().to_string_lossy()) {
                JournalOwner::Sweep => sweep += 1,
                JournalOwner::Stash => stash_present = true,
                JournalOwner::Other => {}
            }
        }

        unsafe { std::env::remove_var("ETUDE_STATE_DIR") };
        let _ = std::fs::remove_dir_all(&state);

        assert_eq!(sweep, 1, "sweep journal not counted");
        assert!(stash_present, "stash journal not detected");
        assert_eq!(
            forget_key_gate(&["forget".to_string()], stash_present, false),
            ForgetKeyGate::Refuse,
            "detected stash did not require confirmation"
        );
    }

    #[test]
    fn apply_exit_code_maps_refusals_and_failures() {
        let collision = etude_core::apply::ApplyError::DestinationCollision(PathBuf::from("x"));
        let io = etude_core::apply::ApplyError::Io(std::io::Error::other("x"));
        assert_eq!(apply_exit_code(&collision, 0), ExitCode::from(2));
        assert_eq!(apply_exit_code(&io, 0), ExitCode::from(3));
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

    struct TestSeal;
    impl etude_core::journal::Sealer for TestSeal {
        fn seal(&self, bytes: &[u8]) -> Result<Vec<u8>, &'static str> {
            Ok(bytes.to_vec())
        }
        fn open(&self, bytes: &[u8]) -> Result<Vec<u8>, &'static str> {
            Ok(bytes.to_vec())
        }
    }

    #[test]
    fn journal_is_fully_undone_when_no_entry_is_done() {
        let undone = etude_core::Journal {
            id: "t".into(),
            tool: "sweep".into(),
            root: PathBuf::from("/tmp"),
            entries: vec![sample_entry(false), sample_entry(false)],
            created_directories: Vec::new(),
            validated_directories: Vec::new(),
            progress_tail_damaged: false,
        };
        assert!(journal_is_fully_undone(&undone, &TestSeal));

        let pending = etude_core::Journal {
            id: "t".into(),
            tool: "sweep".into(),
            root: PathBuf::from("/tmp"),
            entries: vec![sample_entry(false), sample_entry(true)],
            created_directories: Vec::new(),
            validated_directories: Vec::new(),
            progress_tail_damaged: false,
        };
        assert!(!journal_is_fully_undone(&pending, &TestSeal));
    }

    #[test]
    fn a_typod_scan_flag_is_refused_rather_than_scanned_anyway() {
        // The defect: `sweep ~/Desktop --explainn` printed an ordinary scan.
        // The person reading it believes they are looking at --explain output.
        // --jsonn is worse, because whatever was going to parse that gets
        // prose and a success-shaped exit.
        for typo in ["--explainn", "--jsonn", "--quite", "--depht"] {
            let err = check_flags("", &[typo.to_string()])
                .expect_err("a typo'd flag must not be accepted");
            assert!(
                err.contains("Did you mean"),
                "{typo} should suggest a real flag, got: {err}"
            );
        }
    }

    #[test]
    fn a_flag_that_is_nothing_like_a_real_one_gets_no_suggestion() {
        // A confident wrong suggestion is worse than none. The distance cap
        // is what keeps --frobnicate from being told it meant --json.
        let err = check_flags("", &["--frobnicate".to_string()]).expect_err("must refuse");
        assert!(
            !err.contains("Did you mean"),
            "should not guess at an unrelated word, got: {err}"
        );
    }

    #[test]
    fn a_real_flag_in_the_wrong_place_says_where_it_belongs() {
        // --only was accepted and ignored on a scan, which is the same silent
        // lie in a quieter shape. Calling a real flag "unknown" would be its
        // own small lie, so it names the right command instead.
        let err = check_flags("", &["--only".to_string()]).expect_err("must refuse");
        assert!(
            err.contains("sweep apply"),
            "should name where it works: {err}"
        );
        assert!(!err.contains("unknown"), "--only is not unknown: {err}");
    }

    #[test]
    fn every_flag_the_scan_path_accepts_is_still_accepted() {
        // The regression guard. A whitelist that drifts from the real flag
        // surface breaks working commands, which is worse than the bug it
        // fixes. Every one of these is documented in `sweep help`.
        let ok: &[&[&str]] = &[
            &[],
            &["--json"],
            &["--quiet"],
            &["--explain"],
            &["--allow-sync"],
            &["--inspect-content"],
            &["--depth", "2"],
            &["--json", "--quiet"],
            &["--explain", "--depth", "3"],
            &["--depth", "8", "--inspect-content"],
        ];
        for args in ok {
            let v: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
            assert!(
                check_flags("", &v).is_ok(),
                "a documented invocation was refused: {args:?}"
            );
        }
    }

    #[test]
    fn a_value_is_not_mistaken_for_a_flag() {
        // --depth takes a value, so the whitelist has to skip it. Without
        // that, `--depth 2` would work but `--depth --json` would report the
        // wrong problem.
        assert!(check_flags("", &["--depth".into(), "2".into()]).is_ok());
        // And everything after `--` is a path, however flag-shaped it looks.
        assert!(check_flags("", &["--".into(), "--not-a-flag".into()]).is_ok());
    }

    /// The gate. Every command in the dispatch must appear in COMMAND_FLAGS,
    /// so a seventh command cannot ship with no flag checking the way `undo`,
    /// `verify` and `forget` did.
    ///
    /// This asserts against the source of `main` rather than a second list,
    /// because a second list is the thing that kept going stale.
    #[test]
    fn every_dispatched_command_declares_its_flags() {
        let src = include_str!("main.rs");
        let dispatch = src
            .split_once("match args.first().map(String::as_str) {")
            .and_then(|(_, rest)| rest.split_once("\n}"))
            .map(|(d, _)| d)
            .expect("the dispatch match must be findable");

        let mut dispatched: Vec<String> = Vec::new();
        for line in dispatch.lines() {
            let t = line.trim();
            if !t.starts_with("Some(") {
                continue;
            }
            // Every literal in the arm, not just the first. An or-pattern like
            // Some("purge" | "scrub") carries two, and a review caught the
            // earlier version reading only the first and missing the rest.
            let arm = t.split("=>").next().unwrap_or(t);
            for piece in arm.split('"').skip(1).step_by(2) {
                // help/version are answered at dispatch, never reaching a command.
                if matches!(
                    piece,
                    "help" | "--help" | "-h" | "version" | "--version" | "-V" | "--"
                ) {
                    continue;
                }
                dispatched.push(piece.to_string());
            }
        }
        // Naming them, rather than counting them. A count only catches "fewer
        // than before": if a reformat broke the parse while the old arms still
        // matched, a seventh command could slip past a length floor and this
        // test would pass having checked nothing.
        for known in ["apply", "undo", "forget", "review", "verify", "lesson"] {
            assert!(
                dispatched.iter().any(|d| d == known),
                "the dispatch parser no longer finds `{known}`, so this test is \
                 not reading the dispatch any more. Fix the parser before \
                 trusting a green run. Found: {dispatched:?}"
            );
        }
        for name in dispatched {
            assert!(
                COMMAND_FLAGS.iter().any(|(c, _)| *c == name),
                "`sweep {name}` is dispatched but declares no flags in \
                 COMMAND_FLAGS. An empty slice is the right answer if it reads \
                 none, but it has to say so: leaving it out is how three \
                 commands came to accept anything at all."
            );
        }
    }

    /// No command accepts a flag it does not read. The empty ones are the
    /// point: a flag nobody reads is an error, not a no-op.
    #[test]
    fn a_command_that_reads_no_flags_accepts_none() {
        for cmd in ["verify", "lesson"] {
            let (_, flags) = COMMAND_FLAGS
                .iter()
                .find(|(c, _)| *c == cmd)
                .expect("declared");
            assert!(flags.is_empty(), "{cmd} is expected to read no flags");
            assert!(
                check_flags(cmd, &["--frobnicate".to_string()]).is_err(),
                "{cmd} must refuse a flag it does not read"
            );
            assert!(
                check_flags(cmd, &["--yes".to_string()]).is_err(),
                "{cmd} must refuse even a real flag that belongs elsewhere"
            );
        }
    }

    #[test]
    fn consented_inspection_receipt_reports_reads_without_contents() {
        use plan::Inspector;
        scan::reset_receipt();
        let path = std::env::temp_dir().join(format!(
            "etudes-receipt-inspection-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, b"synthetic ordinary text payload").unwrap();
        let mut inspector = ReceiptInspector(inspect::ContentInspector::new());
        let _ = inspector.inspect(&path, "txt");
        std::fs::remove_file(&path).unwrap();
        let receipt = scan::receipt_json();
        assert!(receipt.contains("consented_text_prefixes"));
        assert!(receipt.contains("\"observed\":1"));
        assert!(!receipt.contains("synthetic ordinary text payload"));
    }

    #[test]
    fn undo_id_is_one_record_and_never_beside_a_folder() {
        let args = |words: &[&str]| words.iter().map(|w| w.to_string()).collect::<Vec<_>>();
        assert!(check_positionals("undo", &args(&["undo", "--id", "1-2-3-ab"])).is_ok());
        for bad in [
            &["undo", "--id", "1-2-3-ab", "/tmp/a"][..],
            &["undo", "/tmp/a", "--id", "1-2-3-ab"],
            &["undo", "--id", "../up"],
            &["undo", "--id", "-1"],
        ] {
            assert!(check_positionals("undo", &args(bad)).is_err(), "{bad:?}");
        }
        // After `--` it is a folder that happens to be called that.
        assert!(check_positionals("undo", &args(&["undo", "--", "--id"])).is_ok());
        assert_eq!(undo_id(&args(&["undo", "--", "--id", "x"])), None);
        assert!(check_flags("undo", &args(&["--id", "a", "--id", "b"])).is_err());
        assert!(check_flags("undo", &args(&["--id", ""])).is_err());
        assert!(check_flags("undo", &args(&["--id"])).is_err());
    }

    #[test]
    fn undo_accepts_json_and_refuses_mutation_flags() {
        assert!(check_flags("undo", &["--json".into()]).is_ok());
        for flag in ["--yes", "--no-journal", "--frobnicate"] {
            assert!(check_flags("undo", &[flag.into()]).is_err());
        }
    }

    /// A real flag given to the wrong command names the right one rather than
    /// being called unknown.
    #[test]
    fn a_flag_on_the_wrong_command_names_the_right_one() {
        let err = check_flags("undo", &["--yes".to_string()]).expect_err("refused");
        assert!(err.contains("apply"), "should point at apply: {err}");
        assert!(!err.contains("unknown"), "--yes is a real flag: {err}");

        let err = check_flags("", &["--only".to_string()]).expect_err("refused");
        assert!(err.contains("apply"), "should point at apply: {err}");
    }

    /// Flags that take a value must not swallow the next flag as that value in
    /// a way that hides a second mistake.
    #[test]
    fn a_value_taking_flag_skips_only_its_value() {
        assert!(check_flags("", &["--depth".into(), "2".into()]).is_ok());
        assert!(check_flags("", &["--depth".into(), "2".into(), "--json".into()]).is_ok());
        assert!(check_flags("", &["--".into(), "--not-a-flag".into()]).is_ok());
    }

    /// A repeated flag is refused, not silently reduced to its first
    /// occurrence.
    ///
    /// Measured before fixing, on a real tree: `--depth 1 --depth 3` scanned
    /// 6 items and `--depth 3 --depth 1` scanned 12. Same flags, different
    /// order, different meaning, nothing said. On apply it decided which
    /// files moved -- `--only A --only B --yes` moved A and left every file
    /// in B untouched while reporting success.
    ///
    /// No flag in this tool accumulates or counts, so a repeat is always a
    /// mistake and always one where the user believes something is
    /// happening that is not.
    #[test]
    fn a_repeated_flag_is_refused_rather_than_silently_ignored() {
        for args in [
            vec![
                "--depth".to_string(),
                "1".into(),
                "--depth".into(),
                "3".into(),
            ],
            vec!["--json".to_string(), "--json".into()],
        ] {
            let err = check_flags("", &args)
                .expect_err("a repeated flag must be refused, not reduced to the first");
            assert!(
                err.contains("more than once"),
                "the message must say what is wrong, got: {err}"
            );
        }
        // apply's own flags, where the consequence is which files move.
        let err = check_flags(
            "apply",
            &[
                "--only".to_string(),
                "a".into(),
                "--only".into(),
                "b".into(),
            ],
        )
        .expect_err("a repeated --only must be refused");
        assert!(err.contains("more than once"), "got: {err}");
    }

    /// The other half: a flag used once, and a value that happens to look
    /// like a flag name, must still be accepted. A duplicate check that
    /// compares values as well as flags would reject `--only --only`'s
    /// legitimate cousin: a group genuinely named after a flag.
    #[test]
    fn using_each_flag_once_is_still_accepted() {
        assert!(check_flags("", &["--depth".into(), "3".into()]).is_ok());
        assert!(
            check_flags("", &["--explain".into(), "--depth".into(), "2".into()]).is_ok(),
            "distinct flags must not be mistaken for repeats"
        );
        // A value that is not flag-shaped but repeats a flag's NAME is a
        // value, not a second flag: only args in flag position are checked.
        assert!(
            check_flags("apply", &["--only".into(), "depth".into()]).is_ok(),
            "a group named like a flag is still a value"
        );
    }

    /// A flag-shaped value means the value was forgotten, and swallowing it
    /// turns a typo into a confusing outcome rather than an error: `--only
    /// --yes` used to filter for a group literally named "--yes", find none,
    /// and exit 1 saying there was nothing to apply.
    ///
    /// This is a DIFFERENT case from a repeated flag, and the difference
    /// matters: here the second token is rejected as a missing value, not as
    /// a repeat, even when it happens to be a flag that already appeared.
    /// `sweep apply DIR --yes --only --yes` reports on `--yes` while two
    /// `--yes` tokens exist -- a review raised that as the one place a caret
    /// would still earn its keep, and it is not covered by refusing repeats.
    #[test]
    fn a_flag_where_a_value_belongs_is_an_error_not_a_value() {
        let err = check_flags("apply", &["--only".into(), "--yes".into()])
            .expect_err("--only swallowed --yes as a group name");
        assert!(err.contains("another option"), "got: {err}");

        let err = check_flags("", &["--depth".into(), "--json".into()])
            .expect_err("--depth swallowed --json as a depth");
        assert!(err.contains("another option"), "got: {err}");

        let err = check_flags("", &["--depth".into()]).expect_err("--depth with nothing after it");
        assert!(err.contains("got nothing"), "got: {err}");

        // But a value that merely looks unusual is still a value.
        assert!(check_flags("apply", &["--only".into(), "2026-Photos".into()]).is_ok());
    }

    /// Single-dash flags were ignored everywhere, since every check only ever
    /// looked at `--`. Found by fixing a test that had been comparing typos to
    /// command names and asserting nothing.
    #[test]
    fn a_single_dash_flag_is_checked_too() {
        for cmd in ["", "apply", "undo", "forget"] {
            assert!(
                check_flags(cmd, &["-x".to_string()]).is_err(),
                "`-x` was ignored by {cmd:?}"
            );
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tag_report_tests {
    use super::*;
    use etude_core::apply::{ApplyFaults, ApplyReport};
    use etude_core::plan::{Group, Plan, Signal};
    use std::path::Path;

    fn tag(path: &Path) {
        let status = std::process::Command::new("xattr")
            .args(["-w", "com.apple.metadata:_kMDItemUserTags", "opaque"])
            .arg(path)
            .status()
            .expect("xattr runs");
        assert!(status.success());
    }

    fn one_file_plan(label: &str) -> (PathBuf, PathBuf, Plan) {
        let root =
            std::env::temp_dir().join(format!("sweep_tag_exit_{label}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let file = root.join("a.txt");
        std::fs::write(&file, b"contents").unwrap();
        let out = scan::scan(
            &root,
            &ScanConfig {
                grace: None,
                ..ScanConfig::default()
            },
        )
        .unwrap();
        let plan = Plan::with_groups(
            &out,
            vec![Group {
                name: "Collected".into(),
                signal: Signal::Collected { count: 1 },
                members: vec![file.clone()],
                accepted: true,
            }],
        );
        (root, file, plan)
    }

    /// The origin is taken between the move and the return, so the tagged
    /// item stays where it landed. The run ends with the counts printed and
    /// exit 3 as `incomplete`, never a bare error and never a clean 0.
    #[test]
    fn an_item_that_could_not_be_returned_exits_incomplete_with_its_count() {
        let (root, file, plan) = one_file_plan("stranded");
        let origin = file.clone();
        let mut tag_it = |_: usize, from: &Path| tag(from);
        let mut occupy = |_: usize, _: &Path| std::fs::write(&origin, b"occupant").unwrap();
        let report = etude_core::apply::apply_with_faults_for_tests(
            &plan,
            "sweep",
            None,
            ApplyFaults {
                after_validate: None,
                after_journal: None,
                before_rename: Some(&mut tag_it),
                after_rename: Some(&mut occupy),
                ..Default::default()
            },
        )
        .expect("apply reports instead of failing");
        assert_eq!(report.tagged_not_returned, 1);
        assert_eq!(report.moved, 1);
        assert_eq!(std::fs::read(&file).unwrap(), b"occupant");
        assert_eq!(
            finish_apply(&report, "{}", false, "sweep undo", Path::new("/nowhere")),
            ExitCode::from(3)
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_hold_the_journal_could_not_record_exits_incomplete_with_its_count() {
        let report = ApplyReport {
            held_tagged: 1,
            hold_record_failed: 1,
            ..Default::default()
        };
        assert_eq!(
            finish_apply(&report, "{}", false, "sweep undo", Path::new("/nowhere")),
            ExitCode::from(3)
        );
    }

    #[test]
    fn the_locked_after_the_scan_line_gives_the_step_that_finds_the_item() {
        let one = locked_after_scan_note(1, Path::new("/nowhere"));
        assert!(one.starts_with("1 item was locked after the scan and stays in place."));
        assert!(one.contains("chflags nouchg"), "{one}");
        assert!(
            one.contains("See which with: find '/nowhere' -flags +uchg"),
            "{one}"
        );
        let two = locked_after_scan_note(2, Path::new("/nowhere"));
        assert!(two.starts_with("2 items were locked after the scan and stay in place."));
    }

    /// An item returned because it was tagged mid-move is a count in a normal
    /// run; a run where every item was held moved nothing and is reported as
    /// having nothing to do.
    #[test]
    fn held_items_are_counts_and_an_all_held_run_reports_nothing_to_do() {
        let some = ApplyReport {
            moved: 2,
            held_tagged: 1,
            ..Default::default()
        };
        assert_eq!(
            finish_apply(&some, "{}", false, "sweep undo", Path::new("/nowhere")),
            ExitCode::SUCCESS
        );
        let all = ApplyReport {
            held_tagged: 2,
            held_cross_volume: 1,
            ..Default::default()
        };
        assert_eq!(
            finish_apply(&all, "{}", false, "sweep undo", Path::new("/nowhere")),
            ExitCode::from(1)
        );
        let nothing = ApplyReport::default();
        assert_eq!(
            finish_apply(&nothing, "{}", false, "sweep undo", Path::new("/nowhere")),
            ExitCode::SUCCESS
        );
    }
}
