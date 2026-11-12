//! The gate: is this file safe to touch?
//!
//! Every plan runs the gate before anything else, and the gate answers only
//! that one question. It never decides where a file goes: a file it lets
//! through is a candidate, and what happens to a candidate is the grouping
//! code's business (or the tag plan's). A file it holds is held for every
//! scheme, so no grouping rule, saved route, `--map` or tag can reach it.
//!
//! The holds run in a fixed order, and a file that qualifies for several gets
//! the first. The order is part of the output: it decides which reason a
//! person reads. `gate::tests` pins it.
//!
//! 1. The scan's own holds (`ScanOutcome::project_holds`), copied first.
//! 2. A loose project document: `ProjectDocument`.
//! 3. A file a loose project document could reference: `NearProjectDocument`.
//! 4. A personal-record name: `LooksPersonal`.
//! 5. The content inspector, when the person allowed one: `LooksPersonal`.
//! 6. A download still in flight: `InFlight`.
//! 7. An unreadable modification time while a window is set: `TooRecent`.
//! 8. A modification time inside the window: `TooRecent`.
//!
//! Holds decided while walking the folder (locked items, tagged items,
//! aliases, packages, project folders) stay in `scan.rs`. Sweep must not go
//! inside those items, or must count them without reading them, so only the
//! walk can decide them; the gate receives the results as step 1.

use std::path::PathBuf;
use std::time::Duration;

use crate::Untouched;
use crate::plan::Inspector;
use crate::scan::{Entry, ScanOutcome};

/// What the gate is told, beyond the scan itself.
pub struct GatePolicy<'p> {
    /// The grace window: a file changed within it is too recent to judge.
    /// Plans pass the scan's own window (`ScanOutcome::grace`), so the plan
    /// applies the window the scan ran with.
    pub grace: Option<Duration>,
    /// Let a file through even when its readable modification time is inside
    /// the window. Used only to ask whether recent files would group once the
    /// window passes. A file whose time cannot be read is still held: waiting
    /// cannot make that time readable.
    pub ignore_window: bool,
    /// Consented content reading. Its answer can do exactly one thing: hold a
    /// file as personal.
    pub inspector: Option<&'p mut dyn Inspector>,
}

/// The gate's answer: what it held and why, and what it let through.
pub struct Gated<'a> {
    /// Held files, the scan's holds first, then the rest in scan order.
    pub held: Vec<(PathBuf, Untouched)>,
    /// Every entry the gate let through, in scan order. Folders included: a
    /// folder no rule claims is still reported as left alone.
    pub candidates: Vec<&'a Entry>,
}

/// Families a loose project document can reference: a .flp's samples are
/// Media, a .blend's textures are Images.
const HELD_FAMILIES: &[&str] = &["Media", "Images", "Scripts", "Data"];

/// Sort the scan into held files and candidates. Reads no file contents
/// unless the policy carries an inspector, and decides no destination.
pub fn gate<'a>(scan: &'a ScanOutcome, mut policy: GatePolicy<'_>) -> Gated<'a> {
    let mut held: Vec<(PathBuf, Untouched)> = scan.project_holds.clone();
    let mut candidates: Vec<&Entry> = Vec::new();

    // Loose project documents at the top level, before anything else. A .flp
    // sitting in Downloads used to refuse the ENTIRE sweep (it went through
    // the same check as Cargo.toml, which genuinely does mark a project
    // root). Now the document stays put, and so does its reference surface:
    // a .flp's samples are Media, a .blend's textures are Images, so holding
    // the families the document could point at protects a flat project
    // folder while the PDFs and installers beside it still sort.
    //
    // Screenshot-named files are exempt from the hold. On macOS screenshots
    // land on the Desktop -- the folder most likely to also hold a stray
    // project file -- and a project referencing a file named
    // "Screenshot 2026-08-12 at 9.15.11 AM.png" is not a real layout. The
    // name pattern is the same one the Screenshots detector uses.
    let loose_documents: Vec<&Entry> = scan
        .entries
        .iter()
        .filter(|e| !e.is_dir && crate::scan::is_document_marker(&e.name))
        .collect();
    let held_near_document = |e: &Entry| -> Option<String> {
        let first = loose_documents.first()?;
        if crate::classify::is_screenshot(e) {
            return None;
        }
        let fam = crate::classify::type_family(&e.ext)?;
        HELD_FAMILIES.contains(&fam).then(|| first.name.clone())
    };

    for e in &scan.entries {
        if !e.is_dir && crate::scan::is_document_marker(&e.name) {
            held.push((e.path.clone(), Untouched::ProjectDocument));
            continue;
        }
        if let Some(marker) = held_near_document(e) {
            held.push((e.path.clone(), Untouched::NearProjectDocument(marker)));
            continue;
        }
        // Filename first: it is free, and a file already refused by name is
        // never opened. Reading it could only confirm what we already decided.
        if let Some(cat) = crate::sensitive::looks_personal(e) {
            held.push((e.path.clone(), Untouched::LooksPersonal(cat)));
            continue;
        }
        if let Some(insp) = policy.inspector.as_deref_mut()
            && let Some(cat) = insp.inspect(&e.path, &e.ext)
        {
            held.push((e.path.clone(), Untouched::LooksPersonal(cat)));
            continue;
        }
        // A download still running. Moving one leaves a partial file at a
        // destination the downloader is not writing to, and it never
        // finishes. Checked before the grace window because it is true
        // regardless of age -- a stalled download from last week is still in
        // flight.
        let lower = e.name.to_ascii_lowercase();
        if crate::scan::IN_FLIGHT_SUFFIXES
            .iter()
            .any(|s| lower.ends_with(s))
        {
            held.push((e.path.clone(), Untouched::InFlight));
            continue;
        }
        // Too recent to judge. See ScanConfig::grace for why this is mtime
        // and never atime.
        // `--since 0` means no window, and it has to mean that for every
        // file. elapsed() returns Err for a timestamp in the future, and
        // unwrap_or(true) turned that error into "too recent" even when the
        // window was zero -- so three PDFs dated 2030 were held back by a
        // scan that had been told to hold nothing back. Future mtimes are
        // ordinary: clock skew, restored backups, unpacked archives, network
        // volumes.
        //
        // With a real window a future mtime still counts as too recent. That
        // is the conservative reading of a file whose clock disagrees with
        // this one, and it is now a decision rather than the fallback of an
        // unwrap.
        // A file whose mtime could not be read, with a window in force. The
        // window's question is "was this changed recently", and the honest
        // answer here is that sweep does not know -- so it holds the file
        // back rather than treating unknown as old. Bypassing the check meant
        // an unreadable timestamp was silently the most permissive case.
        if let Some(window) = policy.grace
            && !window.is_zero()
            && e.modified.is_none()
        {
            held.push((e.path.clone(), Untouched::TooRecent));
            continue;
        }
        if !policy.ignore_window
            && let Some(window) = policy.grace
            && !window.is_zero()
            && let Some(modified) = e.modified
            && modified
                .elapsed()
                .map(|since| since < window)
                .unwrap_or(true)
        {
            held.push((e.path.clone(), Untouched::TooRecent));
            continue;
        }
        candidates.push(e);
    }

    Gated { held, candidates }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Category;
    use std::path::Path;
    use std::time::SystemTime;

    const DAY: Duration = Duration::from_secs(24 * 60 * 60);

    fn entry(name: &str, modified: Option<SystemTime>) -> Entry {
        let ext = name
            .rsplit_once('.')
            .map(|(_, e)| e.to_ascii_lowercase())
            .unwrap_or_default();
        Entry {
            path: PathBuf::from("/fixture").join(name),
            name: name.to_string(),
            ext,
            size: 1,
            modified,
            is_dir: false,
            is_package: false,
        }
    }

    fn scan_of(entries: Vec<Entry>) -> ScanOutcome {
        ScanOutcome {
            snapshot: None,
            grace: None,
            root: PathBuf::from("/fixture"),
            entries,
            project_holds: Vec::new(),
            skipped_hidden: 0,
            skipped_symlink: 0,
            aliases_skipped: 0,
            locked_skipped: 0,
            locked_system: 0,
            skipped_system: 0,
            skipped_project: 0,
            skipped_in_flight: 0,
            skipped_package: 0,
            skipped_unreadable: 0,
            skipped_tagged: 0,
            include_tagged: false,
            root_is_synced: false,
            allow_sync: false,
        }
    }

    /// Calls every file personal, and remembers which files it was asked
    /// about.
    struct Refuses {
        category: Category,
        asked: Vec<PathBuf>,
    }

    impl Inspector for Refuses {
        fn inspect(&mut self, path: &Path, _ext: &str) -> Option<Category> {
            self.asked.push(path.to_path_buf());
            Some(self.category)
        }
    }

    /// One file that qualifies for several holds, and the hold it must get.
    struct Case {
        name: &'static str,
        modified: Option<SystemTime>,
        /// Put a loose project document beside the file.
        beside_document: bool,
        /// Run with a content inspector that refuses every file it is asked about.
        inspector: bool,
        qualifies_for: &'static [u8],
        held_as: Untouched,
    }

    fn held_reason(case: &Case) -> (Option<Untouched>, Vec<PathBuf>) {
        let file = entry(case.name, case.modified);
        let path = file.path.clone();
        let mut entries = vec![file];
        if case.beside_document {
            entries.push(entry("mix.flp", None));
        }
        let scan = scan_of(entries);
        let mut inspector = Refuses {
            category: Category::Medical,
            asked: Vec::new(),
        };
        let gated = gate(
            &scan,
            GatePolicy {
                grace: Some(DAY),
                ignore_window: false,
                inspector: case
                    .inspector
                    .then_some(&mut inspector as &mut dyn Inspector),
            },
        );
        let reason = gated
            .held
            .iter()
            .find(|(p, _)| *p == path)
            .map(|(_, why)| why.clone());
        (reason, inspector.asked)
    }

    /// A file that qualifies for several holds gets the earliest one in the
    /// order at the top of this module. The order decides which reason a
    /// person reads, so it is pinned pair by pair: each row qualifies for at
    /// least two adjacent holds, by number. Two pairs cannot be built, and
    /// are said so here rather than faked: no project document extension is
    /// in a family a document can reference (2 and 3), and a time is either
    /// unreadable or readable (7 and 8).
    #[test]
    fn a_file_that_qualifies_for_several_holds_gets_the_earliest() {
        let now = Some(SystemTime::now());
        let cases = [
            Case {
                // A project document with a personal name is still a document.
                name: "tax_return.flp",
                modified: now,
                beside_document: false,
                inspector: true,
                qualifies_for: &[2, 4, 5, 8],
                held_as: Untouched::ProjectDocument,
            },
            Case {
                name: "tax_return.png",
                modified: now,
                beside_document: true,
                inspector: true,
                qualifies_for: &[3, 4, 5, 8],
                held_as: Untouched::NearProjectDocument("mix.flp".into()),
            },
            Case {
                // Beside a document, but the inspector would refuse it too.
                name: "texture.png",
                modified: now,
                beside_document: true,
                inspector: true,
                qualifies_for: &[3, 5, 8],
                held_as: Untouched::NearProjectDocument("mix.flp".into()),
            },
            Case {
                // The name says identity; the inspector would say medical.
                name: "passport.pdf",
                modified: now,
                beside_document: false,
                inspector: true,
                qualifies_for: &[4, 5, 8],
                held_as: Untouched::LooksPersonal(Category::Identity),
            },
            Case {
                name: "tax_return.pdf.part",
                modified: now,
                beside_document: false,
                inspector: false,
                qualifies_for: &[4, 6, 8],
                held_as: Untouched::LooksPersonal(Category::Tax),
            },
            Case {
                name: "notes.pdf.crdownload",
                modified: now,
                beside_document: false,
                inspector: true,
                qualifies_for: &[5, 6, 8],
                held_as: Untouched::LooksPersonal(Category::Medical),
            },
            Case {
                name: "movie.mp4.part",
                modified: None,
                beside_document: false,
                inspector: false,
                qualifies_for: &[6, 7],
                held_as: Untouched::InFlight,
            },
            Case {
                name: "movie.mp4.part",
                modified: now,
                beside_document: false,
                inspector: false,
                qualifies_for: &[6, 8],
                held_as: Untouched::InFlight,
            },
        ];
        for case in &cases {
            let (reason, _) = held_reason(case);
            assert_eq!(
                reason.as_ref(),
                Some(&case.held_as),
                "{} qualifies for holds {:?}",
                case.name,
                case.qualifies_for
            );
        }
    }

    /// A file held before the inspector's turn is never shown to it: a file
    /// already refused by name is never opened.
    #[test]
    fn the_inspector_is_not_asked_about_a_file_an_earlier_hold_took() {
        for (name, beside_document) in [
            ("tax_return.flp", false),
            ("texture.png", true),
            ("passport.pdf", false),
        ] {
            let (_, asked) = held_reason(&Case {
                name,
                modified: None,
                beside_document,
                inspector: true,
                qualifies_for: &[],
                held_as: Untouched::NoClearGroup,
            });
            assert!(
                !asked.iter().any(|p| p.ends_with(name)),
                "{name} was shown to the inspector after an earlier hold took it"
            );
        }
    }

    /// The scan's own holds come first, in the scan's order, before anything
    /// the gate decides.
    #[test]
    fn the_scans_holds_come_first() {
        let mut scan = scan_of(vec![entry("tax_return.pdf", None)]);
        scan.project_holds = vec![
            (
                PathBuf::from("/fixture/song/kick.wav"),
                Untouched::NearProjectDocument("song.als".into()),
            ),
            (
                PathBuf::from("/fixture/song/song.als"),
                Untouched::ProjectDocument,
            ),
        ];
        let gated = gate(
            &scan,
            GatePolicy {
                grace: None,
                ignore_window: false,
                inspector: None,
            },
        );
        let mut want = scan.project_holds.clone();
        want.push((
            PathBuf::from("/fixture/tax_return.pdf"),
            Untouched::LooksPersonal(Category::Tax),
        ));
        assert_eq!(gated.held, want);
        assert!(gated.candidates.is_empty());
    }

    /// Holds 7 and 8 never meet on one file, so their order is not a choice.
    /// What separates them: ignoring the window lets a recent file through,
    /// but not a file whose time cannot be read, because waiting cannot make
    /// that time readable.
    #[test]
    fn ignoring_the_window_still_holds_a_file_whose_time_cannot_be_read() {
        let scan = scan_of(vec![
            entry("unknown.pdf", None),
            entry("recent.pdf", Some(SystemTime::now())),
        ]);
        let gated = gate(
            &scan,
            GatePolicy {
                grace: Some(DAY),
                ignore_window: true,
                inspector: None,
            },
        );
        assert_eq!(
            gated.held,
            vec![(PathBuf::from("/fixture/unknown.pdf"), Untouched::TooRecent)]
        );
        assert_eq!(gated.candidates.len(), 1);
        assert_eq!(gated.candidates[0].name, "recent.pdf");
    }
}
