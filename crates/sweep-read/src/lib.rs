//! Content inspection for sweep (v0.3).
//!
//! Read docs/V03-CONTENT.md before changing anything here. The single invariant
//! this crate exists to uphold:
//!
//! > **Content findings can only ever widen refusal. They never create or name
//! > a group.**
//!
//! Nothing in this crate returns a topic, a label, or a suggested destination.
//! The only output is "leave this file alone, and here is the category", which
//! is why reading more makes sweep act less.

pub mod buf;
pub mod scan;

use std::fs::File;
use std::path::Path;
use std::time::{Duration, Instant};

pub use scan::Found;

/// Per-file wall-clock budget. A stalled read skips the file rather than
/// hanging the whole run.
pub const DEADLINE: Duration = Duration::from_millis(500);

/// What happened when sweep looked at one file. Reported so the user can see
/// exactly what was inspected (the brief's output requirement).
#[derive(Debug, Default, Clone, Copy)]
pub struct Stats {
    /// Files whose contents were actually read.
    pub inspected: usize,
    /// Skipped: extension not in the text allowlist.
    pub skipped_not_text: usize,
    /// Skipped: contained NUL bytes, so it is binary.
    pub skipped_binary: usize,
    /// Skipped: exceeded the wall-clock budget.
    pub skipped_slow: usize,
    /// Files that content inspection moved into the untouched set.
    pub newly_refused: usize,
    /// Files whose buffer could not be locked against swap.
    pub unlocked_reads: usize,
}

impl Stats {
    /// True when at least one read happened without page locking, which the CLI
    /// must disclose rather than let the user assume the strong guarantee.
    pub fn had_unlocked_reads(&self) -> bool {
        self.unlocked_reads > 0
    }
}

/// Inspect one file. Returns `Some(category)` only when it must not be moved.
///
/// Never returns anything usable as a group name — see the module invariant.
pub fn inspect(path: &Path, ext: &str, stats: &mut Stats) -> Option<Found> {
    if !scan::TEXT_EXTS.contains(&ext) {
        stats.skipped_not_text += 1;
        return None;
    }

    let started = Instant::now();
    let mut f = match File::open(path) {
        Ok(f) => f,
        Err(_) => return None,
    };

    let buf = match buf::LockedBuf::read_capped(&mut f) {
        Ok(b) => b,
        Err(_) => return None,
    };

    if started.elapsed() > DEADLINE {
        stats.skipped_slow += 1;
        return None; // buf drops here, and drops erase
    }
    if !buf.locked() {
        stats.unlocked_reads += 1;
    }
    if scan::looks_binary(buf.bytes()) {
        stats.skipped_binary += 1;
        return None;
    }

    stats.inspected += 1;
    let found = scan::scan(buf.bytes());
    if found.is_some() {
        stats.newly_refused += 1;
    }
    found
    // buf drops here: pages unlocked, bytes zeroed. Nothing is retained.
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp(name: &str, body: &[u8]) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("sweep_read_{}_{name}", std::process::id()));
        let mut f = std::fs::File::create(&p).expect("create");
        f.write_all(body).expect("write");
        p
    }

    #[test]
    fn a_text_file_containing_an_ssn_is_refused() {
        let p = tmp("ssn.txt", b"notes\nSSN 123-45-6789\n");
        let mut s = Stats::default();
        assert_eq!(inspect(&p, "txt", &mut s), Some(Found::Identity));
        assert_eq!(s.inspected, 1);
        assert_eq!(s.newly_refused, 1);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn a_non_text_extension_is_never_opened() {
        // The file contains an obvious SSN. It must still not be read, because
        // .pdf is outside the allowlist and v0.3 parses nothing.
        let p = tmp("doc.pdf", b"SSN 123-45-6789");
        let mut s = Stats::default();
        assert_eq!(inspect(&p, "pdf", &mut s), None);
        assert_eq!(s.inspected, 0, "a non-allowlisted file was read");
        assert_eq!(s.skipped_not_text, 1);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn binary_content_in_a_text_extension_is_abandoned() {
        let p = tmp("fake.txt", b"\x00\x01\x02binary");
        let mut s = Stats::default();
        assert_eq!(inspect(&p, "txt", &mut s), None);
        assert_eq!(s.skipped_binary, 1);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn an_ordinary_note_is_not_refused() {
        let p = tmp("ok.md", b"# Notes\nShip the redesign in March.\n");
        let mut s = Stats::default();
        assert_eq!(inspect(&p, "md", &mut s), None);
        assert_eq!(s.inspected, 1);
        assert_eq!(s.newly_refused, 0);
        let _ = std::fs::remove_file(p);
    }
}
