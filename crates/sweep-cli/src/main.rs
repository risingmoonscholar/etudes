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
    sweep forget                 remove sweep's journals; ask before destroying
                                 a key stash also relies on
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

// apply rejects everything not listed here; a silent typo can authorize moves.
const APPLY_FLAGS: &[(&str, bool)] = &[
    ("--yes", false),
    ("--only", true),
    ("--no-journal", false),
    ("--depth", true),
    ("--allow-sync", false),
];

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
        if let Some((_, takes_value)) = APPLY_FLAGS.iter().find(|(name, _)| *name == a) {
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
        ..Default::default()
    };
    let outcome = match scan::scan(path, &cfg) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("sweep: {e}");
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
            "\nScanned {} items  ·  names, sizes and dates only  ·  no contents read",
            plan.scanned
        );
        print_refused_by_policy_note(&plan);
        print_unreadable_warning(&plan);
        println!(
            "\nNothing here needs organising.\n\n\
             Nothing has been moved.  Nothing left this machine."
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

/// The one place this fact is worded, so both the empty-groups branch and
/// `render()` say the same thing. Distinct from `skipped_system`: sweep did
/// not choose to leave this out, it tried to read it and could not — the
/// scan is incomplete, and "Scanned N items" above must not be read as a
/// total. Names both real-world causes from the filed issue (a permission
/// bit, or a path too long for the OS) rather than assuming EACCES — the
/// same `read_dir` failure covers either, and unlike the other skip counts
/// this one is worth naming as a WARNING, not a footnote.
fn print_unreadable_warning(p: &plan::Plan) {
    if p.skipped_unreadable > 0 {
        println!(
            "\n  WARNING: {} {} could not be read (permission denied, a path too long, \
             or another read failure) — contents unknown, NOT included in the count above",
            p.skipped_unreadable,
            if p.skipped_unreadable == 1 {
                "directory"
            } else {
                "directories"
            }
        );
    }
}

/// A deliberate policy refusal — sweep could have entered these and chose
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
    print_refused_by_policy_note(p);
    print_unreadable_warning(p);
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
        ..Default::default()
    };
    let outcome = match scan::scan(&path, &cfg) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("sweep: {e}");
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

/// Refusals are policy stops (2); everything else is a genuine
/// failure (3) -- see README's "Meaningful exit codes".
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
        DestinationExists(_) | DestinationCollision(_) | DestinationIsSynced(_) => {
            ExitCode::from(2)
        }
        Io(_) | Journal(_) | Injected(_) => ExitCode::from(3),
    }
}

/// No done entries means undo already ran — exit 1, don't call undo again.
fn journal_is_fully_undone(j: &etude_core::Journal) -> bool {
    !j.entries.iter().any(|e| e.done)
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
        ..Default::default()
    };
    let outcome = match scan::scan(&path, &cfg) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("sweep: {e}");
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

fn cmd_undo() -> ExitCode {
    let Some(sl) = sealer() else {
        return ExitCode::from(2);
    };
    let mut j = match etude_core::Journal::latest_sealed("sweep", &sl) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("sweep: {e}");
            let code = if matches!(e, etude_core::journal::JournalError::NotFound) {
                1
            } else {
                3
            };
            return ExitCode::from(code);
        }
    };
    if journal_is_fully_undone(&j) {
        println!("\nNothing to undo. This journal was already restored.");
        return ExitCode::from(1);
    }
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

/// Who owns a `{tool}-{id}.journal` filename — same `{tool}-` prefix convention
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
/// strands that stash permanently — so we require an informed yes, or `--yes`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ForgetKeyGate {
    /// No live stash journal, or `--yes` already authorised destruction.
    Proceed,
    /// Stash present, TTY available — warn and ask.
    Ask,
    /// Stash present, no TTY and no `--yes` — refuse.
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
  Destroying it will make any live stash permanently unrecoverable\n\
  — the holding directory stays, but nothing can open it.\n"
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
    // Sweep's own journals only — always, regardless of the key decision below.
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

#[cfg(test)]
mod tests {
    use super::*;

    // unknown flags must fail even after a valid path, or consent can move files.
    #[test]
    fn an_unrecognised_flag_to_apply_is_rejected_not_dropped() {
        for typo in ["--dry-run", "-n", "--force", "--all"] {
            assert!(
                !APPLY_FLAGS.iter().any(|(name, _)| *name == typo),
                "{typo} would be accepted by apply"
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

    // a live stash journal is what gates key destruction — must be detected.
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

    fn sample_entry(done: bool) -> etude_core::journal::Entry {
        etude_core::journal::Entry {
            from: PathBuf::from("/tmp/a"),
            to: PathBuf::from("/tmp/b"),
            method: etude_core::journal::Method::Rename,
            size: 1,
            mtime_secs: 0,
            inode: 0,
            edge_hash: 0,
            done,
        }
    }

    #[test]
    fn journal_is_fully_undone_when_no_entry_is_done() {
        let undone = etude_core::Journal {
            id: "t".into(),
            tool: "sweep".into(),
            root: PathBuf::from("/tmp"),
            entries: vec![sample_entry(false), sample_entry(false)],
        };
        assert!(journal_is_fully_undone(&undone));

        let pending = etude_core::Journal {
            id: "t".into(),
            tool: "sweep".into(),
            root: PathBuf::from("/tmp"),
            entries: vec![sample_entry(false), sample_entry(true)],
        };
        assert!(!journal_is_fully_undone(&pending));
    }
}
