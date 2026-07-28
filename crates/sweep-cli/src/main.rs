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
        Some("review" | "apply" | "undo" | "forget") => {
            eprintln!(
                "sweep: `{}` is specified in docs/SPEC.md but not implemented in v0.1.\n\
                 v0.1 analyses only. Nothing has been moved, so nothing needs undoing.",
                args[0]
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

fn verify() -> ExitCode {
    println!(
        "\nsweep {}\n  \
         content inspection:      not compiled in\n  \
         sweep-core dependencies: 0\n  \
         network-capable crates:  0\n  \
         journal:                 none written by v0.1\n  \
         apply/undo:              not implemented — nothing can be moved\n",
        env!("CARGO_PKG_VERSION")
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
