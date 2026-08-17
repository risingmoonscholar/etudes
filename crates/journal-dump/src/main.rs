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
    let Some(arg) = std::env::args().nth(1) else {
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
