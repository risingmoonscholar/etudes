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

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use etude_core::plan::{Group, Plan, Signal};
use etude_core::scan::{self, ScanConfig};

const USAGE: &str = "\
stash: clean now, decide later

USAGE
    stash [PATH] [--for DURATION]   move everything into a hidden holding folder
    stash pop [PATH]                bring back the stash for PATH (or here)
    stash status [PATH]             what is stashed, and when it is due back
    --json                          machine-readable output (for agents)
    --version                       print the version and exit
    stash help

DURATION
    30m  2h  3d  1w        default: no deadline, restore whenever

stash moves EVERYTHING, including files sweep would refuse to organise.
That is deliberate: clearing a folder for a screen share means clearing it.
Everything is reversible, and stash prints what it took.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

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
        Some("pop" | "restore") => cmd_pop(&args),
        Some("status" | "list") => cmd_status(&args),
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
const STASH_FLAGS: &[&str] = &["--for", "--json"];

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
            "--for" => i += 2,
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
    let t: u64 = name.strip_prefix(".stash-")?.parse().ok()?;
    (t > 0).then_some(t)
}

fn find_holding(root: &Path) -> Option<PathBuf> {
    std::fs::read_dir(root)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.is_dir()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(".stash-"))
        })
}

fn cmd_stash(path: &Path, args: &[String]) -> ExitCode {
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

    if find_holding(&outcome.root).is_some() {
        eprintln!(
            "stash: this folder already has a stash. Run `stash pop` first,\n\
             or stash a different folder."
        );
        return ExitCode::from(2);
    }

    // One group, everything in it. No detectors, no decisions.
    let members: Vec<PathBuf> = outcome.entries.iter().map(|e| e.path.clone()).collect();
    let count = members.len();
    let plan = Plan {
        root: outcome.root.clone(),
        groups: vec![Group {
            name: holding_name(deadline),
            signal: Signal::SharedToken {
                token: "stash".into(),
                count,
            },
            members,
            accepted: true,
        }],
        untouched: Vec::new(),
        scanned: outcome.entries.len(),
        skipped_hidden: outcome.skipped_hidden,
        skipped_symlink: outcome.skipped_symlink,
        skipped_system: outcome.skipped_system,
        skipped_unreadable: outcome.skipped_unreadable,
        root_is_synced: outcome.root_is_synced,
        allow_sync: outcome.allow_sync,
    };

    let json = flag(args, "--json");
    let Some(sl) = sealer() else {
        return ExitCode::from(2);
    };
    match etude_core::apply::apply(&plan, "stash", Some(&sl), None) {
        Ok(r) => {
            if json {
                use etude_core::json as j;
                println!(
                    "{}",
                    j::obj(&[
                        ("action", j::str("stash")),
                        ("root", j::path(&outcome.root)),
                        ("moved", j::num(r.moved)),
                        ("holding", j::str(&holding_name(deadline))),
                        ("due", deadline.map(j::num).unwrap_or_else(|| "null".into())),
                        ("skipped_hidden", j::num(outcome.skipped_hidden)),
                    ])
                );
                return ExitCode::SUCCESS;
            }
            println!("\nStashed {} items.", r.moved);
            println!("{} is clear.\n", path.display());
            match deadline {
                Some(t) => {
                    println!("  Due back: {}", human_time(t));
                    println!("  stash does not run in the background. Run `stash pop`,");
                    println!("  or `stash status` to see what is overdue.");
                }
                None => println!("  No deadline. Restore with: stash pop"),
            }
            if outcome.skipped_hidden > 0 {
                println!(
                    "\n  {} hidden {} left in place.",
                    outcome.skipped_hidden,
                    if outcome.skipped_hidden == 1 {
                        "item was"
                    } else {
                        "items were"
                    }
                );
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("stash: {e}");
            eprintln!("Nothing further was moved. `stash pop` reverses what did happen.");
            apply_exit_code(&e)
        }
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

/// Load a journal by id, warning to stderr rather than silently vanishing it
/// when the failure is a damaged journal (not simply absent). Without this,
/// a truncated journal reads as "no stash here" instead of "a stash exists
/// and can't be trusted". This is the same half-load-as-silence shape as
/// issue #3, just one layer up. `load_sealed` now refuses damaged journals
/// loudly. But `.ok()` at this call site was throwing that refusal away.
fn load_or_warn(
    tool: &str,
    id: &str,
    sealer: &dyn etude_core::journal::Sealer,
    damaged: &mut bool,
) -> Option<etude_core::Journal> {
    match etude_core::Journal::load_sealed(tool, id, sealer) {
        Ok(j) => Some(j),
        Err(etude_core::journal::JournalError::NotFound) => None,
        Err(e) => {
            *damaged = true;
            eprintln!("{tool}: journal {id} is damaged and was skipped: {e}");
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
fn journal_for_root(
    tool: &str,
    sealer: &dyn etude_core::journal::Sealer,
    target: &Path,
) -> (Option<etude_core::Journal>, bool) {
    let mut damaged = false;
    let ids = match etude_core::journal::ids_by_recency(tool) {
        Ok(ids) => ids,
        Err(_) => return (None, false),
    };
    let found = ids
        .into_iter()
        .filter_map(|id| load_or_warn(tool, &id, sealer, &mut damaged))
        .find(|j| {
            j.root
                .canonicalize()
                .is_ok_and(|root| root == target && find_holding(&root).is_some())
        });
    (found, damaged)
}

fn journal_roots(tool: &str, sealer: &dyn etude_core::journal::Sealer) -> Vec<PathBuf> {
    let mut damaged = false;
    etude_core::journal::ids_by_recency(tool)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|id| load_or_warn(tool, &id, sealer, &mut damaged))
        .filter_map(|j| j.root.canonicalize().ok())
        .filter(|root| find_holding(root).is_some())
        .collect()
}

fn cmd_pop(args: &[String]) -> ExitCode {
    let Some(sl) = sealer() else {
        return ExitCode::from(2);
    };
    if let Err(e) = etude_core::journal::ids_by_recency("stash") {
        eprintln!("stash: {e}");
        return ExitCode::from(1);
    }
    let named = args.iter().skip(1).find(|a| !a.starts_with('-'));
    let path = named
        .map(|p| PathBuf::from(expand_tilde(p)))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let Ok(target) = path.canonicalize() else {
        eprintln!("stash: no stash found for {}", path.display());
        return ExitCode::from(1);
    };
    let (found, damaged) = journal_for_root("stash", &sl, &target);
    let mut j = match found {
        Some(j) => j,
        // load_or_warn already printed which journal is damaged and why;
        // exit 3 (a real failure) rather than 1 (nothing to do) so a caller
        // that only checks the exit class doesn't read this the same as a
        // folder that was simply never stashed.
        None if damaged => {
            eprintln!("stash: cannot restore {}. See above", target.display());
            return ExitCode::from(3);
        }
        None if named.is_some() => {
            eprintln!("stash: no stash found for {}", target.display());
            return ExitCode::from(1);
        }
        None => {
            eprintln!("stash: nothing is stashed here: {}", target.display());
            let mut others = Vec::new();
            for root in journal_roots("stash", &sl) {
                if root != target && !others.contains(&root) {
                    others.push(root);
                }
            }
            for root in others {
                eprintln!("stash: a live stash exists in {}", root.display());
            }
            return ExitCode::from(1);
        }
    };
    if journal_is_fully_undone(&j) {
        println!("\nNothing to restore. This stash was already popped.");
        return ExitCode::from(1);
    }
    // Before the counts, because it changes what they mean. A journal cut
    // short describes less than the stash actually did, so the count below is
    // a floor. Restoring quietly from damaged state is the failure this
    // disclosure exists to prevent -- the same reason `sweep undo` says it.
    let tail_was_torn = j.progress_tail_damaged;
    if tail_was_torn {
        eprintln!(
            "stash: this journal is damaged: a progress record was truncated, so the\n\
             \x20      last move it began is not written down. Everything it did record is\n\
             \x20      being restored, and the item whose record was lost is recovered by\n\
             \x20      checking the filesystem rather than the journal."
        );
    }

    let r = etude_core::apply::undo(&mut j, &sl);
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
    println!("\nRestored {} items.", r.restored);
    if !r.skipped_changed.is_empty() {
        println!(
            "  {} changed while stashed and were left alone:",
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
    if let Some(err) = r.error {
        eprintln!("stash: {err}");
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
    if tail_was_torn {
        // Restored, and still an error exit. The items are back, but the
        // journal was damaged and a caller reading only the exit code has to
        // learn that something went wrong -- exit 0 here would tell a script
        // this was an ordinary pop.
        //
        // NOT symmetrical with `sweep undo`, which prints its NOTE and can
        // still exit 0 on the same condition. An earlier version of this
        // comment claimed they matched; they do not. The pre-existing test in
        // exit_codes.rs pins stash at 3 for a damaged journal, so stash keeps
        // it, and which of the two is right is an open question rather than a
        // settled one: exit 3 after a fully successful restore reads as
        // failure to a script, and a torn tail is the ordinary outcome of any
        // interrupted run.
        return ExitCode::from(3);
    }
    ExitCode::SUCCESS
}

fn cmd_status(args: &[String]) -> ExitCode {
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
            let n = std::fs::read_dir(&dir)
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
                        ("due", due.map(j::num).unwrap_or_else(|| "null".into())),
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
    match etude_keep::key() {
        Ok(key) => Some(KeychainSeal { key }),
        Err(e) => {
            eprintln!("stash: {e}");
            eprintln!("Refusing to record a stash in the clear.");
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

    fn stash_plan(root: &Path) -> Plan {
        let root = root.canonicalize().expect("root");
        let members = vec![root.join("one.txt"), root.join("two.txt")];
        Plan {
            root,
            groups: vec![Group {
                name: ".stash-0".into(),
                signal: Signal::SharedToken {
                    token: "stash".into(),
                    count: members.len(),
                },
                members,
                accepted: true,
            }],
            untouched: Vec::new(),
            scanned: 2,
            skipped_hidden: 0,
            skipped_symlink: 0,
            skipped_system: 0,
            skipped_unreadable: 0,
            root_is_synced: false,
            allow_sync: false,
        }
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
        // leading flag meant "stash here". Only these two may lead.
        assert_eq!(STASH_FLAGS, &["--for", "--json"]);
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

        etude_core::apply::apply(&stash_plan(&first), "stash", Some(&TestSeal), None)
            .expect("first stash");
        etude_core::apply::apply(&stash_plan(&second), "stash", Some(&TestSeal), None)
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

        etude_core::apply::apply(&stash_plan(&root), "stash", Some(&TestSeal), None)
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
}
