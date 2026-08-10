//! sweep — organise the obvious, leave the private alone.
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
sweep — organise the obvious, leave the private alone

USAGE
    sweep [PATH] [FLAGS]         analyse and build a plan; changes nothing
    sweep review PATH            walk each group, rename or skip, then apply
    sweep apply PATH --yes       move every proposed group
    sweep apply PATH --only NAME move one group
    sweep undo                   reverse the most recent apply
    sweep forget                 destroy all journals and the key
    sweep verify                 print sweep's own privacy posture

FLAGS
    --depth N       recursion depth (default 1, max 8)
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

    // Journals past their TTL are dropped before anything else. Keeping an
    // index of the user's filenames forever keeps the exposure forever.
    let expired = etude_core::journal::prune_expired();
    if expired > 0 {
        eprintln!("sweep: dropped {expired} journal(s) older than 30 days");
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
        Some("apply") => cmd_apply(&args),
        Some("undo") => cmd_undo(),
        Some("forget") => cmd_forget(),
        Some("review") => cmd_review(&args),
        Some(p) if p.starts_with('-') => {
            run_scan(&std::env::current_dir().unwrap_or_default(), &args)
        }
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

/// Scan and build a plan, running content inspection when the user asked for
/// it AND consented. Returns the plan plus any inspection stats to disclose.
fn scan_and_plan(
    path: &Path,
    args: &[String],
) -> Result<(plan::Plan, Option<etude_read::Stats>), ExitCode> {
    let cfg = ScanConfig {
        depth: value(args, "--depth")
            .and_then(|v| v.parse().ok())
            .unwrap_or(1),
        allow_sync: has(args, "--allow-sync"),
        ..Default::default()
    };
    let outcome = match scan::scan(path, &cfg) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("sweep: {e}");
            return Err(ExitCode::from(2));
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
            eprintln!("sweep: {e}");
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
            "\nScanned {} items  ·  names, sizes and dates only  ·  no contents read\n\n\
             Nothing here needs organising.\n\n\
             Nothing has been moved.  Nothing left this machine.",
            plan.scanned
        );
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
    if personal + unclear > 0 {
        println!(
            "\n  {:<width$}  {:>3} files",
            "Left alone",
            personal + unclear,
            width = width
        );
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
            eprintln!("sweep: {e}");
            eprintln!(
                "Refusing to write an unencrypted journal.\n\
                 Re-run with --no-journal to proceed without undo."
            );
            None
        }
    }
}

/// `sweep review PATH` — scan, decide interactively, apply in one pass.
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

    let cfg = ScanConfig {
        depth: value(args, "--depth")
            .and_then(|v| v.parse().ok())
            .unwrap_or(1),
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
    if p.groups.is_empty() {
        println!("\nNothing here needs organising.");
        return ExitCode::from(1);
    }

    match review::run(&mut p) {
        Ok(review::Outcome::Cancelled) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("sweep: {e}");
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
            eprintln!("sweep: {e}");
            eprintln!("The journal is resumable. `sweep undo` reverses what did happen.");
            ExitCode::from(3)
        }
    }
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
        depth: value(args, "--depth")
            .and_then(|v| v.parse().ok())
            .unwrap_or(1),
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

fn cmd_undo() -> ExitCode {
    let Some(sl) = sealer() else {
        return ExitCode::from(2);
    };
    let mut j = match etude_core::Journal::latest_sealed("sweep", &sl) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("sweep: {e}");
            return ExitCode::from(1);
        }
    };
    match etude_core::apply::undo(&mut j) {
        Ok(r) => {
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
            let _ = j.save_sealed(&sl);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("sweep: {e}");
            ExitCode::from(3)
        }
    }
}

fn cmd_forget() -> ExitCode {
    let dir = etude_core::journal::state_dir();
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
    etude_keep::destroy_key();
    println!("Removed {n} journal(s) from {}.", dir.display());
    println!("Destroyed the journal key in the keychain.");
    println!("Undo is no longer possible for any past run.");
    ExitCode::SUCCESS
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

  What is NOT implemented — do not expect it to work
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
            "YES — move it"
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
