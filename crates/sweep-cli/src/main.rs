//! sweep — organise the obvious, leave the private alone.
//!
//! v0.1 is analysis only: `sweep PATH` produces a plan and prints it. `apply`
//! and `undo` are specified in docs/SPEC.md and land in M5/M6; they refuse
//! rather than pretend.

use std::path::PathBuf;
use std::process::ExitCode;

use sweep_core::plan;
use sweep_core::scan::{self, ScanConfig};

const USAGE: &str = "\
sweep — organise the obvious, leave the private alone

USAGE
    sweep [PATH] [FLAGS]      analyse and build a plan; changes nothing
    sweep verify              print sweep's own privacy posture
    sweep help

FLAGS
    --depth N       recursion depth (default 1, max 8)
    --quiet         counts and signals only; never prints a filename
    --explain       print the signal trace for every file
    --allow-sync    proceed even inside a cloud-synced folder

Nothing is moved by any of these. apply/undo are not in v0.1.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // Core dumps can carry filenames from the heap into a file the OS may offer
    // to upload. Close that before doing any work. THREAT-MODEL § T5.
    disable_core_dumps();

    match args.first().map(String::as_str) {
        None => run_scan(&std::env::current_dir().unwrap_or_default(), &args),
        Some("help" | "--help" | "-h") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some("verify") => verify(),
        Some("apply") => cmd_apply(&args),
        Some("undo") => cmd_undo(),
        Some("forget") => cmd_forget(),
        Some("review") => {
            eprintln!(
                "sweep: interactive review is not implemented yet.\n\
                 Use `sweep PATH` to see the plan, then `sweep apply PATH --yes`\n\
                 to accept every group, or --only NAME to accept one."
            );
            ExitCode::from(3)
        }
        Some(p) if p.starts_with('-') => run_scan(&std::env::current_dir().unwrap_or_default(), &args),
        Some(p) => run_scan(&PathBuf::from(expand_tilde(p)), &args),
    }
}

fn has(args: &[String], flag: &str) -> bool {
    args.iter().any(|a| a == flag)
}

fn value(args: &[String], flag: &str) -> Option<String> {
    let i = args.iter().position(|a| a == flag)?;
    args.get(i + 1).cloned()
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

fn run_scan(path: &PathBuf, args: &[String]) -> ExitCode {
    let quiet = has(args, "--quiet");
    let explain = has(args, "--explain");
    let cfg = ScanConfig {
        depth: value(args, "--depth").and_then(|v| v.parse().ok()).unwrap_or(1),
        allow_sync: has(args, "--allow-sync"),
        ..Default::default()
    };

    let outcome = match scan::scan(path, &cfg) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("sweep: {e}");
            return ExitCode::from(2);
        }
    };

    let plan = plan::build(&outcome);

    if plan.groups.is_empty() {
        println!(
            "\nScanned {} items  ·  names, sizes and dates only  ·  no contents read\n\n\
             Nothing here needs organising.\n\n\
             Nothing has been moved.  Nothing left this machine.",
            plan.scanned
        );
        return ExitCode::from(1);
    }

    render(&plan, quiet, explain);
    ExitCode::SUCCESS
}

fn render(p: &plan::Plan, quiet: bool, explain: bool) {
    println!(
        "\nScanned {} items  ·  names, sizes and dates only  ·  no contents read\n",
        p.scanned
    );

    let width = p.groups.iter().map(|g| g.name.chars().count()).max().unwrap_or(10).max(12);
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
    if personal + unclear > 0 {
        println!("\n  {:<width$}  {:>3} files", "Left alone", personal + unclear, width = width);
        if personal > 0 {
            // One short line. The per-category breakdown is available under
            // --explain; the summary must stay glanceable and must not read
            // like an inventory of the user's private life.
            println!("    {personal} look like personal records — sweep does not touch these");
            if explain {
                for (cat, n) in &counts {
                    println!("      {n:>3}  {}", cat.describe());
                }
            }
        }
        if unclear > 0 {
            println!("    {unclear} no clear group");
        }
    }

    if p.skipped_hidden + p.skipped_symlink > 0 {
        println!(
            "\n  skipped {} hidden items and {} symlinks",
            p.skipped_hidden, p.skipped_symlink
        );
    }
    if p.root_is_synced {
        println!("\n  warning: this folder is inside a cloud-synced tree");
    }

    if explain && !quiet {
        println!("\n--- signal trace ---");
        for g in &p.groups {
            println!("\n{} — {}", g.name, g.signal.describe());
            for m in &g.members {
                println!("    {}", m.display());
            }
        }
    }

    println!("\nNothing has been moved.  Nothing left this machine.");
    if !quiet {
        println!("Note: this listing is in your terminal scrollback.");
    }
    println!("Review: sweep review     Apply: sweep apply   (not in v0.1)");
}

/// `sweep apply PATH --yes | --only NAME`
///
/// Re-scans rather than trusting a stored plan. The filesystem may have changed
/// since the plan was printed, and a stale plan is the write-freshness failure:
/// the record says one thing and the tree says another.
fn cmd_apply(args: &[String]) -> ExitCode {
    let path = args
        .iter()
        .skip(1)
        .find(|a| !a.starts_with('-'))
        .map(|p| PathBuf::from(expand_tilde(p)))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

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

    let cfg = ScanConfig {
        depth: value(args, "--depth").and_then(|v| v.parse().ok()).unwrap_or(1),
        allow_sync: has(args, "--allow-sync"),
        ..Default::default()
    };
    let outcome = match scan::scan(&path, &cfg) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("sweep: {e}");
            return ExitCode::from(2);
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
        println!(
            "\n  --no-journal: nothing will be recorded, so `sweep undo` will not work.\n"
        );
    }

    match sweep_core::apply::apply(&p, use_journal, None) {
        Ok(r) => {
            println!("\nMoved {} files.", r.moved);
            match r.journal_path {
                Some(jp) => {
                    println!("Undo with: sweep undo");
                    println!(
                        "\n  The journal at {} records your filenames in PLAINTEXT.\n  \
                         Encryption is not implemented yet. Use --no-journal to avoid it.",
                        jp.display()
                    );
                }
                None => println!("No journal was written. This cannot be undone."),
            }
            println!("\nNothing left this machine.");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("sweep: {e}");
            eprintln!("The journal is resumable. `sweep undo` reverses what did happen.");
            ExitCode::from(3)
        }
    }
}

fn cmd_undo() -> ExitCode {
    let mut j = match sweep_core::Journal::latest() {
        Ok(j) => j,
        Err(e) => {
            eprintln!("sweep: {e}");
            return ExitCode::from(1);
        }
    };
    match sweep_core::apply::undo(&mut j) {
        Ok(r) => {
            println!("\nRestored {} files.", r.restored);
            if !r.skipped_changed.is_empty() {
                println!(
                    "  {} changed since apply and were left alone:",
                    r.skipped_changed.len()
                );
                for p in &r.skipped_changed {
                    println!("    {}", sweep_core::redact::path(p));
                }
            }
            if !r.skipped_missing.is_empty() {
                println!("  {} were already gone.", r.skipped_missing.len());
            }
            let _ = j.save();
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("sweep: {e}");
            ExitCode::from(3)
        }
    }
}

fn cmd_forget() -> ExitCode {
    let dir = sweep_core::journal::state_dir();
    let mut n = 0;
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().ends_with(".journal")
                && std::fs::remove_file(e.path()).is_ok()
            {
                n += 1;
            }
        }
    }
    println!("Removed {n} journal(s) from {}.", dir.display());
    println!("Undo is no longer possible for any past run.");
    ExitCode::SUCCESS
}

fn verify() -> ExitCode {
    let dir = sweep_core::journal::state_dir();
    let count = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.file_name().to_string_lossy().ends_with(".journal"))
                .count()
        })
        .unwrap_or(0);

    println!(
        "\nsweep {}\n  \
         content inspection:      not compiled in\n  \
         sweep-core dependencies: 0\n  \
         network-capable crates:  0\n  \
         journals held:           {count} in {}\n  \
         journal encryption:      NOT IMPLEMENTED — journals are plaintext\n  \
         journal path synced:     {}\n  \
         interactive review:      not implemented\n\n\
         Destroy all journals with: sweep forget\n",
        env!("CARGO_PKG_VERSION"),
        dir.display(),
        if scan::is_synced(&dir) { "YES — move it" } else { "no" },
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
