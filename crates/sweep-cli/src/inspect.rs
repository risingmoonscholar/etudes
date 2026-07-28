//! The `--inspect-content` gate and adapter.
//!
//! Two things live here: the consent flow, and the bridge from `sweep-read`'s
//! `Found` to `sweep-core`'s `Category`.
//!
//! Consent is separate from `--yes` on purpose. Agreeing to move files is not
//! agreeing to have them read; the brief calls for informed confirmation and a
//! single blanket flag would not be informed.

use std::io::{self, BufRead, IsTerminal, Write};

use sweep_core::plan::Inspector;
use sweep_core::Category;
use sweep_read::{Found, Stats};

pub struct ContentInspector {
    pub stats: Stats,
}

impl ContentInspector {
    pub fn new() -> Self {
        Self { stats: Stats::default() }
    }
}

impl Inspector for ContentInspector {
    fn inspect(&mut self, path: &std::path::Path, ext: &str) -> Option<Category> {
        sweep_read::inspect(path, ext, &mut self.stats).map(|f| match f {
            Found::Identity => Category::Identity,
            Found::Financial => Category::Financial,
            Found::Credential => Category::Credential,
            Found::Medical => Category::Medical,
        })
    }
}

/// State the terms, then require an explicit yes.
///
/// Returns false when the user declines or when there is no terminal to ask.
pub fn consent(input: &mut dyn BufRead, is_tty: bool) -> io::Result<bool> {
    println!(
"
  --inspect-content will read the contents of some files.

  What is read:   .txt .md .csv .log .json .xml .yml and similar,
                  up to 1 MiB each, from this folder only.
  What is not:    PDFs, Office documents, images, archives. None are
                  opened or parsed.
  What happens:   contents stay in memory, are erased immediately
                  after scanning, and are never written anywhere.
  What it does:   finds personal data in files whose names look
                  innocent, so sweep leaves MORE files alone.
                  It never affects where anything is moved to.

  Nothing leaves this machine, with or without this flag."
    );

    if !is_tty {
        eprintln!(
            "\nsweep: --inspect-content needs a terminal to confirm. Refusing to\n\
             read file contents without asking."
        );
        return Ok(false);
    }

    print!("\n  read file contents? [y/N] > ");
    io::stdout().flush()?;
    let mut line = String::new();
    if input.read_line(&mut line)? == 0 {
        return Ok(false); // EOF is not consent
    }
    Ok(line.trim().eq_ignore_ascii_case("y"))
}

/// Ask on the real terminal.
pub fn consent_interactive() -> io::Result<bool> {
    let tty = io::stdin().is_terminal();
    let stdin = io::stdin();
    let mut locked = stdin.lock();
    consent(&mut locked, tty)
}

/// The disclosure the brief requires: what was inspected, and what was kept.
pub fn report(stats: &Stats) {
    println!(
        "\n  Contents read:  {} files  ({} skipped as non-text, {} as binary, {} too slow)",
        stats.inspected, stats.skipped_not_text, stats.skipped_binary, stats.skipped_slow
    );
    println!("  Newly left alone because of what was read: {}", stats.newly_refused);
    println!("  Retained from those reads: nothing. No text, no thumbnails, no index.");

    if stats.had_unlocked_reads() {
        println!(
            "\n  Note: {} read(s) could not be locked against swap on this machine\n  \
             (RLIMIT_MEMLOCK is small on macOS). Contents were still erased after\n  \
             use and never written by sweep, but the OS may have paged them.",
            stats.unlocked_reads
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn ask(script: &str, tty: bool) -> bool {
        let mut c = Cursor::new(script.as_bytes().to_vec());
        consent(&mut c, tty).expect("consent")
    }

    #[test]
    fn only_an_explicit_yes_grants_consent() {
        assert!(ask("y\n", true));
        assert!(ask("Y\n", true));
        assert!(!ask("n\n", true), "n granted consent");
        assert!(!ask("\n", true), "bare enter granted consent");
        assert!(!ask("yes please\n", true), "fuzzy answer granted consent");
    }

    #[test]
    fn eof_is_not_consent() {
        assert!(!ask("", true), "EOF granted consent");
    }

    #[test]
    fn without_a_terminal_it_refuses_rather_than_assuming() {
        // A pipe must never be read as permission to open the user's documents.
        assert!(!ask("y\n", false), "consent granted without a terminal");
    }
}
