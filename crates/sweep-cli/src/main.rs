//! sweep: organise the obvious, leave the private alone.
//!
//! `sweep PATH` plans and changes nothing. `apply` executes, `undo` reverses.
//! The undo journal is sealed with a key held in the login keychain; if sealing
//! is unavailable sweep refuses rather than writing plaintext.

mod inspect;
mod review;

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
    sweep apply PATH --only NAME move one group
    sweep undo [PATH]            reverse the most recent apply, or that folder's
    sweep forget                 remove sweep's journals; ask before destroying
                                 a key stash also relies on
    sweep verify                 print sweep's own privacy posture
    sweep lesson [N]             seven exercises against a folder you throw away

FLAGS
    --depth N       recursion depth (default 1, max 8)
    --since N[h|d]  leave files changed in the last N alone (default 1d, 0 off)
    --json          machine-readable plan on stdout (for agents)
    --quiet         counts and signals only; never prints a filename
    --explain       print the signal trace for every file
    --allow-sync    proceed even inside a cloud-synced folder
    --no-journal    apply without recording undo
    --version       print the version and exit
    --inspect-content  read text file contents to refuse MORE files
                       (asks first; never affects where anything moves)

Only `apply` moves anything. Files that look like personal records are
never moved, in any mode.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // Core dumps can carry filenames from the heap into a file the OS may offer
    // to upload. Close that before doing any work. THREAT-MODEL § T5.
    disable_core_dumps();

    // One-time move from the old XDG-style state directory to the correct
    // macOS one (issue #23). Before anything reads state_dir(), so nothing
    // else in this run can look in the new location and find it empty while
    // the old one still has what it's looking for.
    etude_core::journal::migrate_legacy_state_dir();

    // Journals past their TTL are dropped before anything else. Keeping an
    // index of the user's filenames forever keeps the exposure forever.
    let expired = etude_core::journal::prune_expired();
    if expired > 0 {
        eprintln!("sweep: dropped {expired} journal(s) older than 30 days");
    }

    // Every command's flags are checked here, once, before the command runs.
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
        return ExitCode::from(2);
    }

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
        Some("undo") => cmd_undo(&args),
        Some("forget") => cmd_forget(&args),
        Some("review") => cmd_review(&args),
        Some("--") => match args.get(1) {
            Some(p) => run_scan(&PathBuf::from(expand_tilde(p)), &args),
            None => run_scan(&std::env::current_dir().unwrap_or_default(), &args),
        },
        Some(p) if p.starts_with('-') => {
            run_scan(&std::env::current_dir().unwrap_or_default(), &args)
        }
        Some(p) => run_scan(&PathBuf::from(expand_tilde(p)), &args),
    }
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
         Contents were not read. The signals are names, sizes and dates.\n\
         \n\
         The same plan comes in two other shapes:\n\
         \n\
             sweep Desktop --json     for something that will parse it\n\
             sweep Desktop --quiet    counts only, never a filename",
    ),
    (
        "Move one group instead of all of them",
        "    sweep apply Desktop --only acme\n\
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
                    "sweep: lesson takes a step from 1 to {}, got {v:?}",
                    LESSON.len()
                );
                return ExitCode::from(2);
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

/// Parses `--depth N`. Absent means the documented default of 1.
/// Present-but-invalid (not a number, or outside 1..=8) is an explicit
/// error, never a silent fallback -- a wrong depth changes what gets
/// scanned and must not fail quietly.
fn parse_depth(args: &[String]) -> Result<u8, String> {
    match value(args, "--depth") {
        None => Ok(1),
        Some(v) => match v.parse::<u8>() {
            Ok(d) if (1..=8).contains(&d) => Ok(d),
            _ => Err(format!("--depth must be a number from 1 to 8, got {v:?}")),
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
            format!("--since must be a number, optionally with h or d, got {raw:?}")
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
            format!("--since {raw:?} is too large. The longest window is {most}{sym}")
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
            ("--quiet", false),
            ("--explain", false),
            ("--allow-sync", false),
            ("--inspect-content", false),
            ("--since", true),
        ],
    ),
    (
        "apply",
        &[
            ("--yes", false),
            ("--only", true),
            ("--no-journal", false),
            ("--depth", true),
            ("--allow-sync", false),
            ("--since", true),
        ],
    ),
    (
        "review",
        &[
            ("--allow-sync", false),
            ("--no-journal", false),
            ("--depth", true),
            ("--since", true),
        ],
    ),
    ("forget", &[("--yes", false)]),
    ("undo", &[]),
    ("verify", &[]),
    ("lesson", &[]),
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
    // No flag here is repeatable -- none accumulate, none are counters --
    // so a repeat is always a mistake, and always one where the user
    // believes something is happening that is not.
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
            if seen.contains(&a.as_str()) {
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
                            Some(v) if v.starts_with('-') && v != "--" => {
                                return Err(format!(
                                    "{a} needs a value, and `{v}` is another option."
                                ));
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

fn apply_path(args: &[String]) -> Result<PathBuf, String> {
    let mut path = None;
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "--" {
            if path.is_none() {
                path = args.get(i + 1).map(|p| PathBuf::from(expand_tilde(p)));
            }
            break;
        }
        // Only to skip a flag's value while hunting for the path. Whether the
        // flag is allowed at all is decided at dispatch now.
        if let Some((_, allowed)) = COMMAND_FLAGS.iter().find(|(c, _)| *c == "apply")
            && let Some((_, takes_value)) = allowed.iter().find(|(name, _)| *name == a)
        {
            i += if *takes_value { 2 } else { 1 };
            continue;
        }
        if a.starts_with('-') {
            return Err(format!("unknown option {a}. Run `sweep help`."));
        }
        if path.is_none() {
            path = Some(PathBuf::from(expand_tilde(a)));
        }
        i += 1;
    }

    path.ok_or_else(|| "apply requires an explicit PATH. Run `sweep help`.".to_string())
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
fn scan_and_plan(
    path: &Path,
    args: &[String],
) -> Result<(plan::Plan, Option<etude_read::Stats>), ExitCode> {
    let depth = match parse_depth(args) {
        Ok(d) => d,
        Err(msg) => {
            eprintln!("sweep: {msg}");
            return Err(ExitCode::from(2));
        }
    };
    let cfg = ScanConfig {
        depth,
        allow_sync: has(args, "--allow-sync"),
        grace: match since_flag(args) {
            Ok(g) => g.or(ScanConfig::default().grace),
            Err(msg) => {
                eprintln!("sweep: {msg}");
                return Err(ExitCode::from(2));
            }
        },
        ..Default::default()
    };
    let outcome = match scan::scan(path, &cfg) {
        Ok(o) => o,
        Err(e) => {
            refuse_scan(&e);
            return Err(scan_exit_code(&e));
        }
    };

    if !has(args, "--inspect-content") {
        return Ok((plan::build(&outcome), None));
    }

    // Consent to reading is separate from consent to moving. --yes does not
    // cover it, deliberately.
    match inspect::consent_interactive() {
        Ok(true) => {}
        Ok(false) => {
            println!("  Contents were not read. Continuing on names and dates only.\n");
            return Ok((plan::build(&outcome), None));
        }
        Err(e) => {
            refuse("could not read your answer to the consent prompt", &e);
            return Err(ExitCode::from(3));
        }
    }

    let mut insp = inspect::ContentInspector::new();
    let p = plan::build_with(&outcome, Some(&mut insp));
    Ok((p, Some(insp.stats)))
}

fn run_scan(path: &Path, args: &[String]) -> ExitCode {
    let quiet = has(args, "--quiet");
    let explain = has(args, "--explain");

    let (plan, stats) = match scan_and_plan(path, args) {
        Ok(v) => v,
        Err(code) => return code,
    };

    if plan.groups.is_empty() && has(args, "--json") {
        println!("{}", plan.to_json());
        return ExitCode::from(1);
    }
    if plan.groups.is_empty() {
        println!(
            "\nScanned {} items  ·  names, sizes and dates only  ·  no contents read",
            plan.scanned
        );
        print_refused_by_policy_note(&plan);
        print_projects_skipped_note(&plan);
        print_unreadable_warning(&plan);
        // "Nothing here needs organising" is only true when nothing was held
        // back. Files inside the grace window or still downloading DO need
        // organising -- just not yet -- and saying otherwise sends someone
        // away believing their folder was looked at and found tidy.
        let held = plan.too_recent() + plan.in_flight();
        if held > 0 {
            println!();
            let recent = plan.too_recent();
            if recent == 1 {
                println!("  1 file changed too recently to judge and was left alone");
            } else if recent > 1 {
                println!("  {recent} files changed too recently to judge and were left alone");
            }
            let downloading = plan.in_flight();
            if downloading == 1 {
                println!("  1 download is still in progress and was left alone");
            } else if downloading > 1 {
                println!("  {downloading} downloads are still in progress and were left alone");
            }
            println!(
                "\nNothing else here needs organising. Run again later, or pass\n\
                 --since 0 to include everything.\n\n\
                 Nothing has been moved.  Nothing left this machine."
            );
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
        println!("{}", plan.to_json());
        return ExitCode::SUCCESS;
    }

    render(&plan, quiet, explain, stats.is_some());
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
                "directory"
            } else {
                "directories"
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
/// Say which project folders were stepped over.
///
/// Silence here would be the grace window's mistake again: a folder that
/// looks tidied while a project inside it was deliberately skipped, with
/// nothing saying so. A user who wonders why their project is untouched
/// should not have to guess.
fn print_projects_skipped_note(p: &plan::Plan) {
    if p.skipped_project == 0 {
        return;
    }
    if p.skipped_project == 1 {
        println!("\n  1 folder was left alone because it holds a project file");
    } else {
        println!(
            "\n  {} folders were left alone because they hold project files",
            p.skipped_project
        );
    }
}

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

fn render(p: &plan::Plan, quiet: bool, explain: bool, read_contents: bool) {
    // This line must never claim more restraint than actually happened.
    let basis = if read_contents {
        "names, dates, and the contents of some text files"
    } else {
        "names, sizes and dates only  ·  no contents read"
    };
    println!("\nScanned {} items  ·  {basis}\n", p.scanned);

    let width = p
        .groups
        .iter()
        .map(|g| g.name.chars().count())
        .max()
        .unwrap_or(10)
        .max(12);
    for g in &p.groups {
        println!(
            "  {:<width$}  {:>3} files   {}",
            g.name,
            g.members.len(),
            g.signal.describe(),
            width = width
        );
    }

    let counts = p.sensitive_counts();
    let personal: usize = counts.values().sum();
    let unclear = p.no_clear_group();
    // Two separate outcomes, reported separately. They shared a "Left alone"
    // heading, privacy line first, and a real user read "Left alone 32" plus
    // the only sentence with words in it and concluded the tool had refused
    // 32 files for privacy. One file had been. A count and its reason must
    // not come from different lines.
    if personal + unclear + p.too_recent() + p.in_flight() > 0 {
        println!();
    }
    if personal > 0 {
        // Singular agreement matters here: "1 look like" reads straight past
        // the 1 as if it were a plural count. The per-category breakdown
        // stays behind --explain; the summary must not read like an
        // inventory of the user's private life.
        if personal == 1 {
            println!("  1 file looks like a personal record and was not touched");
        } else {
            println!("  {personal} files look like personal records and were not touched");
        }
        if explain {
            for (cat, n) in &counts {
                println!("      {n:>3}  {}", cat.describe());
            }
        }
    }
    // Each reason says why on its own line, for the same reason the personal
    // and ungrouped counts were split this morning: a number whose reason
    // lives on a different line gets attached to the wrong reason.
    let recent = p.too_recent();
    if recent > 0 {
        if recent == 1 {
            println!("  1 file changed too recently to judge and was left alone");
        } else {
            println!("  {recent} files changed too recently to judge and were left alone");
        }
    }
    let downloading = p.in_flight();
    if downloading > 0 {
        if downloading == 1 {
            println!("  1 download is still in progress and was left alone");
        } else {
            println!("  {downloading} downloads are still in progress and were left alone");
        }
    }
    if unclear > 0 {
        if unclear == 1 {
            println!("  1 file matched no group and was left where it is");
        } else {
            println!("  {unclear} files matched no group and were left where they are");
        }
    }

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
    print_projects_skipped_note(p);
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
    println!("Review: sweep review <path>     Apply: sweep apply <path> --yes");
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

/// Fetch the key, creating it on first use.
///
/// On failure sweep refuses rather than falling back to a plaintext journal.
/// Silently degrading is the failure mode a privacy tool must not have.
fn sealer() -> Option<KeychainSeal> {
    match etude_keep::key() {
        Ok(key) => Some(KeychainSeal { key }),
        Err(e) => {
            refuse("could not get the journal key from the keychain", &e);
            eprintln!(
                "Refusing to write an unencrypted journal.\n\
                 Re-run with --no-journal to proceed without undo."
            );
            None
        }
    }
}

/// `sweep review PATH`: scan, decide interactively, apply in one pass.
///
/// No plan is persisted between commands. See the module docs in review.rs:
/// a stored plan is a second plaintext index of the user's filenames, and
/// deleting the asset beats protecting it.
fn cmd_review(args: &[String]) -> ExitCode {
    let path = args
        .iter()
        .skip(1)
        .find(|a| !a.starts_with('-'))
        .map(|p| PathBuf::from(expand_tilde(p)))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    let depth = match parse_depth(args) {
        Ok(d) => d,
        Err(msg) => {
            eprintln!("sweep: {msg}");
            return ExitCode::from(2);
        }
    };
    let cfg = ScanConfig {
        depth,
        allow_sync: has(args, "--allow-sync"),
        grace: match since_flag(args) {
            Ok(g) => g.or(ScanConfig::default().grace),
            Err(msg) => {
                eprintln!("sweep: {msg}");
                return ExitCode::from(2);
            }
        },
        ..Default::default()
    };
    let outcome = match scan::scan(&path, &cfg) {
        Ok(o) => o,
        Err(e) => {
            refuse_scan(&e);
            return scan_exit_code(&e);
        }
    };
    let mut p = plan::build(&outcome);
    if p.groups.is_empty() {
        println!("\nNothing here needs organising.");
        return ExitCode::from(1);
    }

    match review::run(&mut p) {
        Ok(review::Outcome::Cancelled) => ExitCode::SUCCESS,
        Err(e) => {
            refuse("could not finish the review", &e);
            ExitCode::from(3)
        }
        Ok(review::Outcome::Apply) => {
            let use_journal = !has(args, "--no-journal");
            let sl = if use_journal {
                match sealer() {
                    Some(s) => Some(s),
                    None => return ExitCode::from(2),
                }
            } else {
                None
            };
            run_apply(&p, sl)
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

/// A scan error, named only when it needs naming.
///
/// Every ScanError variant except `Io` already says what it is -- "not a
/// directory: ~/x", "refused: will not run as root", "refused: N items
/// exceeds the M item cap". Wrapping those in "could not read that folder"
/// produces two sentences arguing with each other, which is exactly the
/// problem the NotFound case in `cmd_undo` avoids. A review caught the
/// first version of this doing it to every variant.
///
/// `Io` is the only bare one, and the only one where the reader is left
/// guessing what sweep was attempting.
fn refuse_scan(e: &etude_core::scan::ScanError) {
    match e {
        etude_core::scan::ScanError::Io(_) => refuse("could not read that folder", e),
        _ => eprintln!("sweep: {e}"),
    }
}

/// An apply error, named only when it needs naming.
///
/// Same discipline as `refuse_scan`. `DestinationExists`,
/// `DestinationCollision`, `IsSynced` and `CannotCompareNames` are refusals
/// raised in preflight, before anything has moved, and each says so itself.
/// A review pointed out that "could not finish moving the files" in front of
/// one of those claims progress that never happened.
fn refuse_apply(e: &etude_core::apply::ApplyError) {
    use etude_core::apply::ApplyError as E;
    match e {
        E::Io(_) => refuse("could not move the files", e),
        // Journal is NOT wrapped, and the reason is worth keeping: it is
        // raised from record_done/record_undone/save_sealed, which run
        // AFTER a move or restore has already succeeded. "could not move
        // the files" in front of a journal write failure is simply false --
        // the files moved, the record of them did not. A review caught the
        // first version of this doing exactly that, which is the same
        // overclaim as the "could not finish putting the files back" it was
        // brought in to replace.
        //
        // JournalError already names its own subsystem: "journal io: ...",
        // "journal malformed: ...", "no journal found". It needs no phrase
        // in front of it, and the caller's own follow-up lines (about
        // whether the journal is resumable) carry the consequence.
        _ => eprintln!("sweep: {e}"),
    }
}

fn scan_exit_code(e: &etude_core::scan::ScanError) -> ExitCode {
    if e.is_refusal() {
        ExitCode::from(2)
    } else {
        ExitCode::from(3)
    }
}

/// Destination* variants are safety refusals (2); Io/Journal/Injected are failures (3).
fn apply_exit_code(e: &etude_core::apply::ApplyError) -> ExitCode {
    use etude_core::apply::ApplyError::*;
    match e {
        DestinationExists(_)
        | DestinationCollision(_)
        | DestinationIsSynced(_)
        | CannotCompareNames(_) => ExitCode::from(2),
        Io(_) | Journal(_) | Injected(_) => ExitCode::from(3),
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

/// Shared tail of `apply` and `review`.
fn run_apply(p: &plan::Plan, sl: Option<KeychainSeal>) -> ExitCode {
    match etude_core::apply::apply(
        p,
        "sweep",
        sl.as_ref().map(|s| s as &dyn etude_core::journal::Sealer),
        None,
    ) {
        Ok(r) => {
            println!("\nMoved {} files.", r.moved);
            match r.journal_path {
                Some(jp) => {
                    println!("Undo with: sweep undo");
                    println!("Encrypted journal: {}", jp.display());
                }
                None => println!("No journal was written. This cannot be undone."),
            }
            println!("\nNothing left this machine.");
            ExitCode::SUCCESS
        }
        Err(e) => {
            refuse_apply(&e);
            eprintln!("The journal is resumable. `sweep undo` reverses what did happen.");
            apply_exit_code(&e)
        }
    }
}

/// `sweep apply PATH --yes | --only NAME`
///
/// Re-scans rather than trusting a stored plan. The filesystem may have changed
/// since the plan was printed, and a stale plan is the write-freshness failure:
/// the record says one thing and the tree says another.
fn cmd_apply(args: &[String]) -> ExitCode {
    let path = match apply_path(&args[1..]) {
        Ok(path) => path,
        Err(msg) => {
            eprintln!("sweep: {msg}");
            return ExitCode::from(2);
        }
    };

    let accept_all = has(args, "--yes");
    let only = value(args, "--only");
    let use_journal = !has(args, "--no-journal");

    if !accept_all && only.is_none() {
        eprintln!(
            "sweep: refusing to apply without an explicit choice.\n\
             Pass --yes to accept every group, or --only NAME for one."
        );
        return ExitCode::from(2);
    }

    let depth = match parse_depth(args) {
        Ok(d) => d,
        Err(msg) => {
            eprintln!("sweep: {msg}");
            return ExitCode::from(2);
        }
    };
    let cfg = ScanConfig {
        depth,
        allow_sync: has(args, "--allow-sync"),
        grace: match since_flag(args) {
            Ok(g) => g.or(ScanConfig::default().grace),
            Err(msg) => {
                eprintln!("sweep: {msg}");
                return ExitCode::from(2);
            }
        },
        ..Default::default()
    };
    let outcome = match scan::scan(&path, &cfg) {
        Ok(o) => o,
        Err(e) => {
            refuse_scan(&e);
            return scan_exit_code(&e);
        }
    };
    let mut p = plan::build(&outcome);
    for g in &mut p.groups {
        g.accepted = match &only {
            Some(n) => &g.name == n,
            None => true,
        };
    }
    if p.moves() == 0 {
        eprintln!("sweep: nothing to apply.");
        return ExitCode::from(1);
    }

    if !use_journal {
        println!("\n  --no-journal: nothing will be recorded, so `sweep undo` will not work.\n");
    }

    let sl = if use_journal {
        match sealer() {
            Some(s) => Some(s),
            None => return ExitCode::from(2),
        }
    } else {
        None
    };

    run_apply(&p, sl)
}

/// The newest sweep journal that still has entries to reverse.
fn newest_undoable(sl: &dyn etude_core::journal::Sealer) -> Option<etude_core::Journal> {
    let ids = etude_core::journal::ids_by_recency("sweep").ok()?;
    ids.into_iter()
        .filter_map(|id| etude_core::Journal::load_sealed("sweep", &id, sl).ok())
        // Same reason as journal_is_fully_undone: a journal whose records were
        // lost has nothing marked Moved, yet its files are at their
        // destinations. Skipping it here hides it from undo entirely.
        .find(|j| {
            j.entries.iter().any(|e| e.is_moved()) || etude_core::apply::unrecorded_moves(j) > 0
        })
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
) -> Option<etude_core::Journal> {
    let ids = etude_core::journal::ids_by_recency("sweep").ok()?;
    ids.into_iter()
        .filter_map(|id| etude_core::Journal::load_sealed("sweep", &id, sl).ok())
        .find(|j| {
            // Same filesystem question as the no-argument route. A journal
            // whose records were lost has nothing marked Moved while its files
            // are at their destinations; filtering on is_moved alone hides it
            // and the caller is told no apply of this folder is reversible.
            (j.entries.iter().any(|e| e.is_moved()) || etude_core::apply::unrecorded_moves(j) > 0)
                && j.root.canonicalize().is_ok_and(|root| root == target)
        })
}

fn cmd_undo(args: &[String]) -> ExitCode {
    let Some(sl) = sealer() else {
        return ExitCode::from(2);
    };

    // A named path reverses that folder's apply, whichever one it was.
    if let Some(named) = args.iter().skip(1).find(|a| !a.starts_with('-')) {
        let path = PathBuf::from(expand_tilde(named));
        let Ok(target) = path.canonicalize() else {
            eprintln!("sweep: no folder at {}", path.display());
            return ExitCode::from(3);
        };
        return match sweep_journal_for_root(&sl, &target) {
            Some(mut j) => finish_undo(&mut j, &sl),
            None => {
                eprintln!(
                    "sweep: nothing to undo for {}. No apply of that folder is still\nreversible.",
                    target.display()
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
    if let Some(mut j) = newest_undoable(&sl) {
        return finish_undo(&mut j, &sl);
    }
    let mut j = match etude_core::Journal::latest_sealed("sweep", &sl) {
        Ok(j) => j,
        Err(e) => {
            // NotFound is not a failure to open the journal, it is the
            // absence of one -- "could not open the journal: no journal
            // found" would be two sentences arguing with each other. It
            // keeps its own plain rendering; everything else gets the
            // operation named.
            if matches!(e, etude_core::journal::JournalError::NotFound) {
                eprintln!("sweep: {e}");
                return ExitCode::from(1);
            }
            refuse("could not open the most recent journal", &e);
            return ExitCode::from(3);
        }
    };
    if journal_is_fully_undone(&j) {
        // Say which operation this is about. The newest journal being already
        // restored is a fact about an earlier run, and a user who just applied
        // something reads it as a fact about that. If they used --no-journal
        // there is no record to reverse, and the honest thing is to say so
        // rather than let a reassuring sentence stand in for one.
        println!("\nNothing to undo. The most recent recorded apply was already restored.");
        println!(
            "If you just ran apply with --no-journal, nothing was recorded and undo cannot \nreverse it."
        );
        return ExitCode::from(1);
    }
    finish_undo(&mut j, &sl)
}

/// The part of undo that is the same whether a path was named or not.
fn finish_undo(j: &mut etude_core::Journal, sl: &dyn etude_core::journal::Sealer) -> ExitCode {
    // Pass the sealer: undo persists each reversal as it happens now, so a
    // kill partway through leaves a journal that agrees with the disk.
    // Said before the counts, because it changes what they mean: a journal
    // that lost its tail describes less than the apply actually did, so
    // "Restored N files" is a floor rather than the whole story. Recovering
    // quietly from damaged state would be its own version of the bug this
    // reporting exists to prevent.
    let tail_was_torn = j.progress_tail_damaged;
    if tail_was_torn {
        println!(
            "\n  NOTE: this journal was cut short, so its last recorded move is missing.\n\
             \x20       Everything it did record is being reversed, and the file whose\n\
             \x20       record was lost is recovered by checking the filesystem."
        );
    }

    let r = etude_core::apply::undo(j, sl);
    if r.unrecorded_moves > 1 {
        eprintln!(
            "sweep: refused. This journal is missing more than one record: {n} files are\n\
             at their destinations while the journal says they were never moved.\n\n\
             A crash between a move and its record loses exactly one record, and that\n\
             one is recoverable. Losing several means the journal itself was damaged,\n\
             and undo can only reach the first of them -- so it would put a few back,\n\
             leave the rest where they are, and report success. Nothing has been\n\
             touched instead.\n\n\
             The files are still at their destinations. Nothing is lost.",
            n = r.unrecorded_moves
        );
        return ExitCode::from(3);
    }
    // Report what actually happened before anything about the outcome: this
    // count is real even when `r.error` is set below.
    println!("\nRestored {} files.", r.restored);
    if !r.skipped_changed.is_empty() {
        println!(
            "  {} changed since apply and were left alone:",
            r.skipped_changed.len()
        );
        for p in &r.skipped_changed {
            println!("    {}", etude_core::redact::path(p));
        }
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
    if let Some(err) = r.error {
        refuse_apply(&err);
        match saved {
            Ok(()) => {
                eprintln!(
                    "The journal is resumable. `sweep undo` will pick up where this left off."
                );
            }
            Err(save_err) => eprintln!(
                "sweep: additionally, the journal could not be updated ({save_err}). It may not reflect the files just restored."
            ),
        }
        return ExitCode::from(3);
    }
    if let Err(save_err) = saved {
        eprintln!("sweep: undo finished, but the journal could not be saved: {save_err}");
        return ExitCode::from(3);
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
    if etude_keep::destroy_key() {
        println!("Destroyed the journal key in the keychain.");
        println!("Undo is no longer possible for any past run.");
        ExitCode::SUCCESS
    } else {
        eprintln!("sweep: could not confirm the journal key was destroyed.");
        ExitCode::from(3)
    }
}

fn refuse_shared_key_destroy() -> ExitCode {
    eprintln!(
        "sweep: refusing to destroy the shared journal key while a stash\n\
         journal exists. Pass `sweep forget --yes` to destroy it anyway."
    );
    ExitCode::from(2)
}

fn cmd_forget(args: &[String]) -> ExitCode {
    use std::io::IsTerminal;

    let dir = etude_core::journal::state_dir();
    let mut n = 0;
    let mut stash_present = false;
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            match classify_journal_filename(&name) {
                JournalOwner::Sweep => {
                    if std::fs::remove_file(e.path()).is_ok() {
                        n += 1;
                    }
                }
                JournalOwner::Stash => stash_present = true,
                JournalOwner::Other => {}
            }
        }
    }
    // Sweep's own journals only, always, regardless of the key decision below.
    println!("Removed {n} journal(s) from {}.", dir.display());

    match forget_key_gate(args, stash_present, std::io::stdin().is_terminal()) {
        ForgetKeyGate::Proceed => destroy_shared_key(),
        ForgetKeyGate::Ask => {
            if forget_shared_key_consent() {
                destroy_shared_key()
            } else {
                refuse_shared_key_destroy()
            }
        }
        ForgetKeyGate::Refuse => refuse_shared_key_destroy(),
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

  What is on this machine
    journals held          {count} in {dir}
    journal encryption     XChaCha20-Poly1305, key in the login keychain
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

  Destroy all journals and the key:  sweep forget
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

        assert!(
            root.join("IMG_4400.HEIC").exists(),
            "a camera-burst file moved: apply operated on the cwd with no PATH given"
        );
        let _ = std::fs::remove_dir_all(&root);
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
            tool: "sweep".into(),
            root: PathBuf::from("/tmp"),
            entries: vec![sample_entry(false), sample_entry(false)],
            progress_tail_damaged: false,
        };
        assert!(journal_is_fully_undone(&undone));

        let pending = etude_core::Journal {
            id: "t".into(),
            tool: "sweep".into(),
            root: PathBuf::from("/tmp"),
            entries: vec![sample_entry(false), sample_entry(true)],
            progress_tail_damaged: false,
        };
        assert!(!journal_is_fully_undone(&pending));
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
        for cmd in ["undo", "verify", "lesson"] {
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
