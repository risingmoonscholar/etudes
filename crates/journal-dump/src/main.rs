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

/// Observe the journal's durable per-entry undo records, then stop the undo
/// process while it is still in the witnessed partial state. This is a
/// stress-only diagnostic: it emits counts and never prints journal paths.
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
    let timeout = Duration::from_secs(10);
    loop {
        let journal = match etude_core::Journal::load_sealed(tool, id, &sealer) {
            Ok(journal) => journal,
            // Undo may be appending a frame while the diagnostic opens it.
            // A truncated in-flight frame is transient; retry until its
            // fsync completes or the bounded observation window expires.
            Err(_) => {
                if started.elapsed() >= timeout {
                    eprintln!("journal-dump: undo journal could not be read consistently");
                    return ExitCode::from(1);
                }
                std::thread::sleep(Duration::from_millis(1));
                continue;
            }
        };
        let reversed = journal
            .entries
            .iter()
            .filter(|entry| entry.state == etude_core::journal::EntryState::Reversed)
            .count();
        let total = journal.entries.len();
        if reversed >= target && reversed < total {
            // SIGSTOP preserves the precise journal/filesystem state long
            // enough for the caller to deliver its SIGKILL and test recovery.
            #[cfg(unix)]
            // SAFETY: pid is validated as positive; the caller supplies the
            // undo process it just started in this stress scenario.
            let stopped = unsafe { kill(pid, sigstop()) == 0 };
            #[cfg(not(unix))]
            let stopped = false;
            if stopped {
                println!("{reversed} {total}");
                return ExitCode::SUCCESS;
            }
        }
        if started.elapsed() >= timeout {
            eprintln!("journal-dump: undo did not reach a stoppable partial state");
            return ExitCode::from(1);
        }
        std::thread::sleep(Duration::from_millis(1));
    }
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
