//! Print a sealed journal in plaintext, for diagnosing a failed stress trial.
//!
//! Why this exists: the journal is encrypted with a key held in the login
//! keychain. Copying the file preserves ciphertext and nothing else, so an
//! artifact uploaded from CI is unreadable by the time anyone opens it -- the
//! runner that held the key is gone. The only moment a journal can be made
//! readable is on the machine that wrote it, while it still has the key.
//!
//! Dev-only: `publish = false`, and not a binary of any `*-cli` crate, so none
//! of the documented install lines produce it. That is not the same as being
//! uninstallable -- anyone with the source tree can build or `cargo install
//! --path` it. The bar it raises is against a user who installed the tools,
//! not against someone holding the repo, and the at-rest posture is unchanged
//! either way: this still needs the login keychain key, exactly as `undo` does.
//!
//!     journal-dump <path-to-.journal>
//!
//! Exit: 0 printed · 1 could not read or open it · 2 bad usage.

use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, Instant};

struct KeychainSeal {
    key: [u8; 32],
}

impl etude_core::journal::Sealer for KeychainSeal {
    fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>, &'static str> {
        etude_keep::seal(&self.key, plaintext).map_err(|_| "could not seal")
    }
    fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, &'static str> {
        etude_keep::open(&self.key, sealed).map_err(|_| "wrong key or the journal was altered")
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "--watch-undo") {
        return watch_undo(&args);
    }
    if args.first().is_some_and(|arg| arg == "--watch-apply") {
        return watch_apply(&args);
    }
    if args.first().is_some_and(|arg| arg == "--watch-apply-saved") {
        return watch_apply_saved(&args);
    }
    if args
        .first()
        .is_some_and(|arg| arg == "--watch-apply-stepped")
    {
        return step_apply(&args);
    }
    // A decrypted journal is the full pathname history of everything the
    // tools ever moved. That answers only to a person at a terminal, for the
    // same reason stash's --paths does: the reach of the read is the whole
    // machine, and a non-interactive caller gets a refusal with the reason
    // rather than the history.
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        eprintln!(
            "journal-dump: a decrypted journal lists every path the tools ever\n\
             touched, so it prints only for a person at a terminal. Refusing."
        );
        return ExitCode::from(2);
    }
    let Some(arg) = args.first() else {
        eprintln!("usage: journal-dump <path-to-.journal>");
        return ExitCode::from(2);
    };
    let path = Path::new(&arg);

    // The loader addresses journals by (tool, id) under ETUDE_STATE_DIR rather
    // than by path, so point that at the file's own directory and split the
    // name. A journal is named `<tool>-<id>.journal`.
    let Some(dir) = path.parent() else {
        eprintln!("journal-dump: {arg} has no parent directory");
        return ExitCode::from(2);
    };
    let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
        eprintln!("journal-dump: {arg} is not a journal filename");
        return ExitCode::from(2);
    };
    let Some((tool, id)) = stem.split_once('-') else {
        eprintln!("journal-dump: {stem} is not <tool>-<id>");
        return ExitCode::from(2);
    };
    unsafe { std::env::set_var("ETUDE_STATE_DIR", dir) };

    let key = match etude_keep::key() {
        Ok(k) => k,
        Err(e) => {
            eprintln!("journal-dump: no key in the keychain: {e:?}");
            eprintln!("  A journal can only be opened on the machine that wrote it.");
            return ExitCode::from(1);
        }
    };
    let sealer = KeychainSeal { key };

    match etude_core::Journal::load_sealed(tool, id, &sealer) {
        Ok(j) => {
            // encode() is the journal's own plaintext form, so this stays
            // correct if entries gain fields.
            print!("{}", j.encode());
            ExitCode::SUCCESS
        }
        Err(e) => {
            // A load failure is itself the finding when a trial stranded a
            // file: print it rather than exiting silently.
            eprintln!("journal-dump: could not open {arg}: {e:?}");
            ExitCode::from(1)
        }
    }
}

/// Step an undo between confirmed stops until its journal, read only while
/// the undo is stopped, records the target count of durable restores. This
/// is a stress-only diagnostic: it emits counts and never prints journal paths.
fn watch_undo(args: &[String]) -> ExitCode {
    if args.len() != 4 {
        eprintln!("usage: journal-dump --watch-undo <journal> <target> <pid>");
        return ExitCode::from(2);
    }
    let path = Path::new(&args[1]);
    let Ok(target) = args[2].parse::<usize>() else {
        eprintln!("journal-dump: invalid progress target");
        return ExitCode::from(2);
    };
    let Ok(pid) = args[3].parse::<i32>() else {
        eprintln!("journal-dump: invalid process id");
        return ExitCode::from(2);
    };
    if pid <= 0 {
        eprintln!("journal-dump: invalid process id");
        return ExitCode::from(2);
    }
    let Some(dir) = path.parent() else {
        eprintln!("journal-dump: journal has no parent directory");
        return ExitCode::from(2);
    };
    let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
        eprintln!("journal-dump: invalid journal name");
        return ExitCode::from(2);
    };
    let Some((tool, id)) = stem.split_once('-') else {
        eprintln!("journal-dump: invalid journal name");
        return ExitCode::from(2);
    };
    if tool != "sweep" {
        eprintln!("journal-dump: undo progress requires a sweep journal");
        return ExitCode::from(2);
    }
    unsafe { std::env::set_var("ETUDE_STATE_DIR", dir) };
    let key = match etude_keep::key() {
        Ok(key) => key,
        Err(_) => {
            eprintln!("journal-dump: journal key unavailable");
            return ExitCode::from(1);
        }
    };
    let sealer = KeychainSeal { key };
    let started = Instant::now();
    // The larger late fixture can take over ten seconds on a loaded host.
    // Bound observation without confusing slow progress with a tool failure.
    let deadline = started + Duration::from_secs(30);
    // Pause, then read. A journal load refuses a file that changes while it
    // is read, and a running undo appends a record per restored file. Reading
    // a running undo therefore races its writes: on a CI host, every read of
    // the 2000-file fixture overlapped an append, and the undo finished
    // unobserved.
    // So the undo runs only in short slices between confirmed stops, and the
    // journal is read only while the undo is stopped. Each slice is short
    // against the work left after the target (see `Slices`), so the undo
    // cannot pass the target and finish unobserved.
    let mut slices = Slices::new();
    let mut first_progress = false;
    let mut stops = 0u64;
    while Instant::now() < deadline {
        let Some(mut stopped) = StoppedProcess::stop(pid, deadline) else {
            eprintln!(
                "journal-dump: elapsed_us={} undo exited or did not stop; interruption unproven",
                started.elapsed().as_micros()
            );
            return ExitCode::from(1);
        };
        slices.stopped();
        stops += 1;
        let paused = etude_core::Journal::load_sealed_exact(tool, id, &sealer);
        // A stop inside a journal write leaves a torn tail or a file that is
        // mid-change. Dropping the guard resumes the undo for another slice.
        let Ok(paused) = paused else {
            slices.run(stopped, None, target);
            continue;
        };
        if paused.progress_tail_damaged {
            slices.run(stopped, None, target);
            continue;
        }
        let reversed = paused
            .entries
            .iter()
            .filter(|entry| entry.state == etude_core::journal::EntryState::Reversed)
            .count();
        let moved = paused
            .entries
            .iter()
            .filter(|entry| entry.is_moved())
            .count();
        if reversed > 0 && !first_progress {
            eprintln!(
                "journal-dump: elapsed_us={} stops={stops} first_progress={reversed}/{}",
                started.elapsed().as_micros(),
                paused.entries.len()
            );
            first_progress = true;
        }
        if reversed == paused.entries.len() || (reversed > 0 && moved == 0) {
            eprintln!("journal-dump: undo already fully reversed; interruption unproven");
            return ExitCode::from(1);
        }
        if reversed >= target && reversed > 0 && moved > 0 {
            eprintln!(
                "journal-dump: elapsed_us={} stops={stops} confirmed_stopped_count={reversed}/{}",
                started.elapsed().as_micros(),
                paused.entries.len()
            );
            // The caller owns resume and termination from this point.
            stopped.resume_on_drop = false;
            println!("{reversed} {}", paused.entries.len());
            return ExitCode::SUCCESS;
        }
        slices.run(stopped, Some(reversed), target);
    }
    eprintln!(
        "journal-dump: elapsed_us={} stops={stops} no stopped partial undo before deadline; interruption unproven",
        started.elapsed().as_micros()
    );
    ExitCode::from(1)
}

/// Read only the supplied trial directory and root. Successful observation
/// transfers a confirmed stopped child to the caller; every missed window
/// resumes it. No pathname history is printed by this stress-only mode.
fn watch_apply(args: &[String]) -> ExitCode {
    use etude_core::journal::EntryState;
    if args.len() != 5 {
        eprintln!("usage: journal-dump --watch-apply <state-dir> <root> <victim> <pid>");
        return ExitCode::from(2);
    }
    let Ok(pid) = args[4].parse::<i32>() else {
        return ExitCode::from(2);
    };
    if pid <= 0 {
        return ExitCode::from(2);
    }
    let state_dir = Path::new(&args[1]);
    let Ok(root) = std::fs::canonicalize(&args[2]) else {
        eprintln!("journal-dump: trial root unavailable; injection unproven");
        return ExitCode::from(1);
    };
    let victim = Path::new(&args[3]);
    unsafe { std::env::set_var("ETUDE_STATE_DIR", state_dir) };
    let Ok(key) = etude_keep::key() else {
        eprintln!("journal-dump: apply journal key unavailable; injection unproven");
        return ExitCode::from(1);
    };
    let sealer = KeychainSeal { key };
    let deadline = Instant::now() + Duration::from_secs(10);
    // A zombie is not an injectable live apply, even though kill(pid, 0)
    // can still succeed for it while its parent has not called wait().
    while let Some(state) = process_state(pid) {
        if state.contains('Z') || Instant::now() >= deadline {
            break;
        }
        if let Ok(files) = std::fs::read_dir(state_dir) {
            for file in files.flatten() {
                let name = file.file_name();
                let Some(id) = name
                    .to_str()
                    .and_then(|s| s.strip_prefix("sweep-"))
                    .and_then(|s| s.strip_suffix(".journal"))
                else {
                    continue;
                };
                let Ok(observed) = etude_core::Journal::load_sealed_exact("sweep", id, &sealer)
                else {
                    continue;
                };
                if observed.root != root {
                    continue;
                }
                let moved = observed.entries.iter().filter(|e| e.is_moved()).count();
                if moved == 0 {
                    continue;
                }
                if moved == observed.entries.len() {
                    eprintln!("journal-dump: apply already finished; injection unproven");
                    return ExitCode::from(1);
                }
                // Signal only after real journal progress. A successful
                // signal delivery alone does not prove the OS stopped it.
                let Some(mut stopped) = StoppedProcess::stop(pid, deadline) else {
                    continue;
                };
                let Ok(paused) = etude_core::Journal::load_sealed_exact("sweep", id, &sealer)
                else {
                    // Drop resumes a child stopped inside a journal write.
                    continue;
                };
                if paused.root != root || paused.progress_tail_damaged {
                    continue;
                }
                let moved = paused.entries.iter().filter(|e| e.is_moved()).count();
                let Some((index, target)) = paused
                    .entries
                    .iter()
                    .enumerate()
                    .find(|(_, entry)| entry.from == victim)
                else {
                    continue;
                };
                if moved > 0
                    && moved < paused.entries.len()
                    && target.state == EntryState::Planned
                    && target.from.is_file()
                    // A tag entry changes a file in place, so its destination
                    // is the file itself.
                    && (target.tag.is_some() || !target.to.exists())
                {
                    // The caller owns resume/termination from this point.
                    stopped.resume_on_drop = false;
                    println!("{moved} {} {index}", paused.entries.len());
                    return ExitCode::SUCCESS;
                }
                // A pre-signal observation may already be stale. Never
                // relabel that missed injection as a pass or tool failure.
                eprintln!(
                    "journal-dump: target no longer pending while stopped; injection unproven"
                );
                return ExitCode::from(1);
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    eprintln!("journal-dump: no stopped apply with a pending target; injection unproven");
    ExitCode::from(1)
}

/// Stop an apply once its journal records at least `moves` moves and at
/// least one entry is still planned with its source in place. The journal is
/// read only while the apply is confirmed stopped: a load refuses a file that
/// changes during the read, and a running apply appends a record per move.
/// Between stops the apply runs for a slice that is short against the work
/// left (see `Slices`), so it cannot finish unobserved. Success leaves the apply stopped for the caller and prints
/// `<moved> <total>`. Every miss resumes it and exits 1.
fn step_apply(args: &[String]) -> ExitCode {
    use etude_core::journal::EntryState;
    if args.len() != 5 {
        eprintln!("usage: journal-dump --watch-apply-stepped <state-dir> <root> <pid> <moves>");
        return ExitCode::from(2);
    }
    let Ok(pid) = args[3].parse::<i32>() else {
        return ExitCode::from(2);
    };
    let Ok(moves) = args[4].parse::<usize>() else {
        return ExitCode::from(2);
    };
    if pid <= 0 || moves == 0 {
        return ExitCode::from(2);
    }
    let state_dir = Path::new(&args[1]);
    let Ok(root) = std::fs::canonicalize(&args[2]) else {
        eprintln!("journal-dump: trial root unavailable; injection unproven");
        return ExitCode::from(1);
    };
    unsafe { std::env::set_var("ETUDE_STATE_DIR", state_dir) };
    let Ok(key) = etude_keep::key() else {
        eprintln!("journal-dump: apply journal key unavailable; injection unproven");
        return ExitCode::from(1);
    };
    let sealer = KeychainSeal { key };
    let journal_ids = || -> Vec<String> {
        std::fs::read_dir(state_dir)
            .map(|files| {
                files
                    .flatten()
                    .filter_map(|file| {
                        file.file_name()
                            .to_str()
                            .and_then(|s| s.strip_prefix("sweep-"))
                            .and_then(|s| s.strip_suffix(".journal"))
                            .map(str::to_string)
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    // Records already there belong to earlier runs, not to this apply.
    let earlier = journal_ids();
    let mut slices = Slices::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut stops = 0u64;
    while Instant::now() < deadline {
        let Some(mut stopped) = StoppedProcess::stop(pid, deadline) else {
            eprintln!("journal-dump: apply exited or did not stop; injection unproven");
            return ExitCode::from(1);
        };
        slices.stopped();
        stops += 1;
        let mut progress = None;
        let ids: Vec<String> = journal_ids()
            .into_iter()
            .filter(|id| !earlier.contains(id))
            .collect();
        for id in &ids {
            let Ok(paused) = etude_core::Journal::load_sealed_exact("sweep", id, &sealer) else {
                continue;
            };
            if paused.root != root || paused.progress_tail_damaged {
                continue;
            }
            let moved = paused.entries.iter().filter(|e| e.is_moved()).count();
            if moved < moves {
                progress = Some(moved);
                continue;
            }
            let pending = paused
                .entries
                .iter()
                .any(|e| e.state == EntryState::Planned && e.from.is_file() && !e.to.exists());
            if !pending {
                eprintln!(
                    "journal-dump: apply had no pending entry when stopped ({moved} moved); injection unproven"
                );
                return ExitCode::from(1);
            }
            eprintln!(
                "journal-dump: stops={stops} confirmed_stopped_moves={moved}/{}",
                paused.entries.len()
            );
            // The caller owns resume and termination from this point.
            stopped.resume_on_drop = false;
            println!("{moved} {}", paused.entries.len());
            return ExitCode::SUCCESS;
        }
        slices.run(stopped, progress, moves);
    }
    eprintln!(
        "journal-dump: stops={stops} no stopped apply with {moves} moves before deadline; injection unproven"
    );
    ExitCode::from(1)
}

/// Stop an apply after its journal is saved and before its first move, for a
/// contender to act in that window. The journal file appears only when its
/// bytes are complete (written to a temp name, then renamed), and the save
/// then syncs the folder, so the apply is stopped inside that sync or soon
/// after. Under the confirmed stop the journal is reloaded: it must be for the
/// trial root, hold no move, and every source must still be in place.
/// Anything else resumes the apply and reports the window as missed.
fn watch_apply_saved(args: &[String]) -> ExitCode {
    if !(4..=5).contains(&args.len()) {
        eprintln!("usage: journal-dump --watch-apply-saved <state-dir> <root> <pid> [sweep|stash]");
        return ExitCode::from(2);
    }
    let Ok(pid) = args[3].parse::<i32>() else {
        return ExitCode::from(2);
    };
    if pid <= 0 {
        return ExitCode::from(2);
    }
    let tool = args.get(4).map_or("sweep", String::as_str);
    if tool != "sweep" && tool != "stash" {
        eprintln!("journal-dump: the tool must be sweep or stash");
        return ExitCode::from(2);
    }
    let prefix = format!("{tool}-");
    let state_dir = Path::new(&args[1]);
    let Ok(root) = std::fs::canonicalize(&args[2]) else {
        eprintln!("journal-dump: trial root unavailable; injection unproven");
        return ExitCode::from(1);
    };
    unsafe { std::env::set_var("ETUDE_STATE_DIR", state_dir) };
    let Ok(key) = etude_keep::key() else {
        eprintln!("journal-dump: apply journal key unavailable; injection unproven");
        return ExitCode::from(1);
    };
    let sealer = KeychainSeal { key };
    let journal_ids = || -> Vec<String> {
        std::fs::read_dir(state_dir)
            .map(|files| {
                files
                    .flatten()
                    .filter_map(|file| {
                        file.file_name()
                            .to_str()
                            .and_then(|s| s.strip_prefix(prefix.as_str()))
                            .and_then(|s| s.strip_suffix(".journal"))
                            .map(str::to_string)
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    // Records already there belong to earlier runs, not to this apply.
    let earlier = journal_ids();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        // No ps(1) here: it takes milliseconds, longer than the window. An
        // apply that has exited fails kill(pid, 0), or, as a zombie, fails
        // the confirmed stop below.
        if !signal_process(pid, 0) {
            break;
        }
        let Some(id) = journal_ids().into_iter().find(|id| !earlier.contains(id)) else {
            std::thread::sleep(Duration::from_micros(50));
            continue;
        };
        let Some(mut stopped) = StoppedProcess::stop(pid, deadline) else {
            break;
        };
        let Ok(paused) = etude_core::Journal::load_sealed_exact(tool, &id, &sealer) else {
            eprintln!(
                "journal-dump: the new journal did not load while stopped; injection unproven"
            );
            return ExitCode::from(1);
        };
        let moved = paused.entries.iter().filter(|e| e.is_moved()).count();
        let pending = !paused.entries.is_empty()
            && paused.entries.iter().all(|entry| {
                entry.state == etude_core::journal::EntryState::Planned
                    && entry.from.is_file()
                    && !entry.to.exists()
            });
        if paused.root == root && moved == 0 && !paused.progress_tail_damaged && pending {
            // The caller owns resume from this point.
            stopped.resume_on_drop = false;
            println!("{moved} {}", paused.entries.len());
            return ExitCode::SUCCESS;
        }
        eprintln!(
            "journal-dump: apply was not stopped before its first move ({moved} moved); injection unproven"
        );
        return ExitCode::from(1);
    }
    eprintln!("journal-dump: no stopped apply with a saved journal; injection unproven");
    ExitCode::from(1)
}

/// How long a stepped process runs between two confirmed stops. Before the
/// first progress the slice doubles from 1 ms to at most 8 ms. After it, the
/// slice is half the time the measured rate needs to reach the target, from
/// 1 ms to 50 ms. A fixed 1 ms slice spent most of the deadline on stopped
/// journal reads when the host was loaded. Half the time to the target stops
/// the process near the target, far from the end of its work.
struct Slices {
    ran: Duration,
    next: Duration,
    resumed: Option<Instant>,
}

impl Slices {
    fn new() -> Self {
        Self {
            ran: Duration::ZERO,
            next: Duration::from_millis(1),
            resumed: None,
        }
    }

    /// Count the time since the last resume as run time.
    fn stopped(&mut self) {
        if let Some(resumed) = self.resumed.take() {
            self.ran += resumed.elapsed();
        }
    }

    /// Resume the stopped process for one slice. `done` is the progress seen
    /// in this stop, or `None` when the journal could not be read.
    fn run(&mut self, stopped: StoppedProcess, done: Option<usize>, target: usize) {
        const MIN: Duration = Duration::from_millis(1);
        match done {
            Some(done) if done > 0 => {
                let per_item = self.ran / u32::try_from(done).unwrap_or(u32::MAX);
                let left = u32::try_from(target.saturating_sub(done)).unwrap_or(u32::MAX);
                self.next = (per_item * left / 2).clamp(MIN, Duration::from_millis(50));
            }
            Some(_) => self.next = (self.next * 2).min(Duration::from_millis(8)),
            None => self.next = MIN,
        }
        drop(stopped);
        self.resumed = Some(Instant::now());
        std::thread::sleep(self.next);
    }
}

/// The process state as ps(1) prints its first letter: R, S, T or Z.
/// On macOS this asks the kernel with proc_pidinfo(2). Starting ps takes
/// milliseconds, and a stepped undo confirms thousands of stops.
#[cfg(target_os = "macos")]
fn process_state(pid: i32) -> Option<String> {
    // struct proc_bsdinfo is 136 bytes; pbi_status (p_stat) is at offset 4.
    const PROC_PIDTBSDINFO: i32 = 3;
    const SIZE: i32 = 136;
    let mut info = [0u8; SIZE as usize];
    // SAFETY: the buffer is exactly the size the flavor writes.
    let written = unsafe { proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, info.as_mut_ptr(), SIZE) };
    if written != SIZE {
        return None;
    }
    let status = u32::from_le_bytes(info[4..8].try_into().ok()?);
    // p_stat: SIDL 1, SRUN 2, SSLEEP 3, SSTOP 4, SZOMB 5.
    Some(
        match status {
            2 => "R",
            4 => "T",
            5 => "Z",
            _ => "S",
        }
        .to_string(),
    )
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn proc_pidinfo(pid: i32, flavor: i32, arg: u64, buffer: *mut u8, size: i32) -> i32;
}

#[cfg(not(target_os = "macos"))]
fn process_state(pid: i32) -> Option<String> {
    let output = std::process::Command::new("ps")
        .args(["-o", "state=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let state = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (output.status.success() && !state.is_empty()).then_some(state)
}

struct StoppedProcess {
    pid: i32,
    resume_on_drop: bool,
}

impl StoppedProcess {
    fn stop(pid: i32, deadline: Instant) -> Option<Self> {
        if !signal_process(pid, sigstop()) {
            return None;
        }
        let guard = Self {
            pid,
            resume_on_drop: true,
        };
        while Instant::now() < deadline {
            let state = process_state(pid)?;
            if state.contains('T') {
                return Some(guard);
            }
            if state.contains('Z') {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        None
    }
}

impl Drop for StoppedProcess {
    fn drop(&mut self) {
        if self.resume_on_drop {
            signal_process(self.pid, sigcont());
        }
    }
}

fn signal_process(pid: i32, signal: i32) -> bool {
    #[cfg(unix)]
    // SAFETY: the watcher validates a positive PID supplied by its caller.
    {
        unsafe { kill(pid, signal) == 0 }
    }
    #[cfg(not(unix))]
    false
}

#[cfg(target_os = "linux")]
const fn sigcont() -> i32 {
    18
}

#[cfg(not(target_os = "linux"))]
const fn sigcont() -> i32 {
    19
}

#[cfg(unix)]
unsafe extern "C" {
    fn kill(pid: i32, signal: i32) -> i32;
}

#[cfg(target_os = "macos")]
const fn sigstop() -> i32 {
    17
}

#[cfg(target_os = "linux")]
const fn sigstop() -> i32 {
    19
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "linux"))))]
const fn sigstop() -> i32 {
    17
}
