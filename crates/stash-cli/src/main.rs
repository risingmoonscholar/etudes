//! stash — clean now, decide later.
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
//! file, no second state store, nothing to fall out of sync — the deadline is
//! derived from the filesystem rather than recorded next to it.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use sweep_core::plan::{Group, Plan, Signal};
use sweep_core::scan::{self, ScanConfig};

const USAGE: &str = "\
stash — clean now, decide later

USAGE
    stash [PATH] [--for DURATION]   move everything into a hidden holding folder
    stash pop                       bring it all back now
    stash status                    what is stashed, and when it is due back
    stash help

DURATION
    30m  2h  3d  1w        default: no deadline, restore whenever

stash moves EVERYTHING, including files sweep would refuse to organise.
That is deliberate: clearing a folder for a screen share means clearing it.
Everything is reversible, and stash prints what it took.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("help" | "--help" | "-h") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some("pop" | "restore") => cmd_pop(),
        Some("status" | "list") => cmd_status(&args),
        None => cmd_stash(&std::env::current_dir().unwrap_or_default(), &args),
        Some(p) if p.starts_with('-') => {
            cmd_stash(&std::env::current_dir().unwrap_or_default(), &args)
        }
        Some(p) => cmd_stash(&PathBuf::from(expand_tilde(p)), &args),
    }
}

fn expand_tilde(p: &str) -> String {
    match p.strip_prefix("~/") {
        Some(rest) => std::env::var("HOME").map(|h| format!("{h}/{rest}")).unwrap_or(p.into()),
        None => p.to_string(),
    }
}

fn value(args: &[String], flag: &str) -> Option<String> {
    let i = args.iter().position(|a| a == flag)?;
    args.get(i + 1).cloned()
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
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
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
            return ExitCode::from(2);
        }
    };
    if outcome.entries.is_empty() {
        println!("Nothing to stash — {} is already clear.", path.display());
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
            signal: Signal::SharedToken { token: "stash".into(), count },
            members,
            accepted: true,
        }],
        untouched: Vec::new(),
        scanned: outcome.entries.len(),
        skipped_hidden: outcome.skipped_hidden,
        skipped_symlink: outcome.skipped_symlink,
        root_is_synced: outcome.root_is_synced,
    };

    let Some(sl) = sealer() else { return ExitCode::from(2) };
    match sweep_core::apply::apply(&plan, Some(&sl), None) {
        Ok(r) => {
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
                println!("\n  {} hidden items were left in place.", outcome.skipped_hidden);
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("stash: {e}");
            eprintln!("Nothing further was moved. `stash pop` reverses what did happen.");
            ExitCode::from(3)
        }
    }
}

fn cmd_pop() -> ExitCode {
    let Some(sl) = sealer() else { return ExitCode::from(2) };
    let mut j = match sweep_core::Journal::latest_sealed(&sl) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("stash: {e}");
            return ExitCode::from(1);
        }
    };
    match sweep_core::apply::undo(&mut j) {
        Ok(r) => {
            println!("\nRestored {} items.", r.restored);
            if !r.skipped_changed.is_empty() {
                println!("  {} changed while stashed and were left alone:", r.skipped_changed.len());
                for p in &r.skipped_changed {
                    println!("    {}", sweep_core::redact::path(p));
                }
            }
            if !r.skipped_missing.is_empty() {
                println!("  {} were already gone.", r.skipped_missing.len());
            }
            let _ = j.save_sealed(&sl);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("stash: {e}");
            ExitCode::from(3)
        }
    }
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

    match find_holding(&root) {
        None => {
            println!("Nothing stashed in {}.", root.display());
            ExitCode::from(1)
        }
        Some(dir) => {
            let n = std::fs::read_dir(&dir).map(|r| r.flatten().count()).unwrap_or(0);
            let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            println!("\n{n} items stashed from {}.", root.display());
            match deadline_of(name) {
                Some(t) if t <= now_secs() => {
                    println!("  OVERDUE since {} — run `stash pop`", human_time(t));
                }
                Some(t) => println!("  Due back {}", human_time(t)),
                None => println!("  No deadline set."),
            }
            println!("\nRestore with: stash pop");
            ExitCode::SUCCESS
        }
    }
}

/// Epoch seconds to a readable local-ish stamp, without a date dependency.
fn human_time(epoch: u64) -> String {
    let now = now_secs();
    let delta = epoch as i64 - now as i64;
    let abs = delta.unsigned_abs();
    let unit = if abs < 3600 {
        format!("{} minutes", abs / 60)
    } else if abs < 86_400 {
        format!("{} hours", abs / 3600)
    } else {
        format!("{} days", abs / 86_400)
    };
    if delta >= 0 { format!("in {unit}") } else { format!("{unit} ago") }
}

struct KeychainSeal {
    key: [u8; 32],
}

impl sweep_core::journal::Sealer for KeychainSeal {
    fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>, &'static str> {
        sweep_keep::seal(&self.key, plaintext).map_err(|_| "could not seal the record")
    }
    fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, &'static str> {
        sweep_keep::open(&self.key, sealed).map_err(|_| "wrong key or the record was altered")
    }
}

/// Refuses rather than writing an unencrypted record of what was stashed.
fn sealer() -> Option<KeychainSeal> {
    match sweep_keep::key() {
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
    fn the_deadline_round_trips_through_the_directory_name() {
        // The deadline lives in the name, so this IS the storage layer.
        let name = holding_name(Some(1_800_000_000));
        assert_eq!(deadline_of(&name), Some(1_800_000_000));
        assert_eq!(deadline_of(&holding_name(None)), None, "0 should read as no deadline");
        assert_eq!(deadline_of("Screenshots"), None, "an ordinary folder read as a stash");
    }
}
