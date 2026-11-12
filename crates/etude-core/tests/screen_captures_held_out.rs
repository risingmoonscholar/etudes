//! Screen captures where a correct-looking match is most likely wrong.
//!
//! Held-out tests, written by a reviewer who did not write the change. Each
//! one targets a place where a plausible simplification (normalising the
//! name, probing every file, trusting the first file's evidence, letting a
//! capture skip a hold) would still pass the change's own tests.
//!
//! The attribute is set through `etude_core::xattr`, with the value a real
//! `screencapture -x` wrote on a GitHub `macos-15` runner (macOS 15.8.2):
//! <https://github.com/risingmoonscholar/etudes/actions/runs/43233341758>.

use std::fs;
use std::path::{Path, PathBuf};

use etude_core::Untouched;
use etude_core::evidence::capture::{self, Capture};
use etude_core::evidence::capture_names::STILL_PREFIXES;
use etude_core::evidence::{CaptureBasis, CaptureKind};
use etude_core::plan::{self, Plan};
use etude_core::scan::{self, ScanConfig, ScanOutcome};

/// The binary plist `true` that `screencapture -x` wrote on the runner.
#[cfg(target_os = "macos")]
const MARK: &[u8] = &[
    0x62, 0x70, 0x6c, 0x69, 0x73, 0x74, 0x30, 0x30, 0x09, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x09,
];

struct Folder(PathBuf);

impl Folder {
    fn new(label: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("etude_held_out_{label}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        Folder(root.canonicalize().unwrap())
    }

    fn file(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, b"synthetic").unwrap();
        path
    }

    #[cfg(target_os = "macos")]
    fn marked(&self, name: &str) -> PathBuf {
        let path = self.file(name);
        etude_core::xattr::write(&path, capture::ATTRIBUTE, MARK)
            .expect("set the screen-capture attribute");
        path
    }

    fn scan(&self) -> ScanOutcome {
        scan::scan(
            &self.0,
            &ScanConfig {
                grace: None,
                ..ScanConfig::default()
            },
        )
        .expect("scan")
    }

    fn plan(&self) -> Plan {
        plan::build(&self.scan(), plan::Request::default())
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn os_name(name: &str) -> bool {
    capture::recognise_by_name(name).map(|c| c.basis) == Some(CaptureBasis::OsName)
}

// Used by the macOS-only tests; unused elsewhere would fail clippy there.
#[cfg(target_os = "macos")]
fn capture_of(out: &ScanOutcome, name: &str) -> Option<Capture> {
    out.entries
        .iter()
        .find(|e| e.name == name)
        .unwrap_or_else(|| panic!("{name} is in the scan: {:?}", out.entries))
        .capture
}

fn untouched<'p>(plan: &'p Plan, name: &str) -> Option<&'p Untouched> {
    plan.untouched
        .iter()
        .find(|(path, _)| path.file_name().is_some_and(|n| n == name))
        .map(|(_, reason)| reason)
}

fn grouped(plan: &Plan, name: &str) -> bool {
    plan.groups.iter().any(|g| {
        g.members
            .iter()
            .any(|m| m.file_name().is_some_and(|n| n == name))
    })
}

/// The receipt row for `category` as (attempted, observed, failed), or `None`
/// when nothing in that category was recorded.
fn receipt(category: &str) -> Option<(u64, u64, u64)> {
    let json = scan::receipt_json();
    let key = format!("\"category\":\"{category}\"");
    let row = &json[json.find(&key)? + key.len()..];
    let row = &row[..row.find('}').unwrap()];
    let field = |name: &str| -> u64 {
        let at = row.find(&format!("\"{name}\":")).unwrap() + name.len() + 3;
        row[at..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse()
            .unwrap()
    };
    Some((field("attempted"), field("observed"), field("failed")))
}

/// The prefix from the table that begins with `word`.
fn still_prefix(word: &str) -> &'static str {
    STILL_PREFIXES
        .iter()
        .find(|p| p.starts_with(word))
        .unwrap_or_else(|| panic!("the table has a still prefix starting {word:?}"))
}

// 1. Dutch: the soft hyphen is part of the word macOS writes.
#[test]
fn dutch_needs_the_soft_hyphen_macos_writes() {
    let prefix = still_prefix("Scherm\u{ad}afbeelding");
    assert_eq!(prefix, "Scherm\u{ad}afbeelding ");
    let real = format!("{prefix}2026-11-12 om 09.41.12.png");
    assert_eq!(
        capture::recognise_by_name(&real),
        Some(Capture {
            kind: CaptureKind::Still,
            basis: CaptureBasis::OsName,
        })
    );
    let typed = real.replace('\u{ad}', "");
    assert_eq!(typed, "Schermafbeelding 2026-11-12 om 09.41.12.png");
    assert!(!os_name(&typed), "{typed:?} is not a name macOS wrote");
}

// 2. French: the apostrophe macOS writes is U+2019, not U+0027.
#[test]
fn french_needs_the_typographic_apostrophe() {
    let real = "Capture d\u{2019}\u{e9}cran 2026-11-12 \u{e0} 09.41.12.png";
    assert!(STILL_PREFIXES.iter().any(|p| real.starts_with(p)));
    assert_eq!(
        capture::recognise_by_name(real),
        Some(Capture {
            kind: CaptureKind::Still,
            basis: CaptureBasis::OsName,
        })
    );
    let straight = real.replace('\u{2019}', "'");
    assert!(
        !os_name(&straight),
        "{straight:?} is not a name macOS wrote"
    );
}

// 3. Arabic: the prefix starts and ends with a right-to-left mark.
#[test]
fn arabic_keeps_both_right_to_left_marks() {
    let prefix = still_prefix("\u{200f}");
    assert!(prefix.ends_with('\u{200f}'), "{prefix:?}");
    assert!(prefix.chars().filter(|c| *c == '\u{200f}').count() >= 2);
    let real = format!("{prefix}2026-11-12 09.41.12.png");
    assert_eq!(
        capture::recognise_by_name(&real),
        Some(Capture {
            kind: CaptureKind::Still,
            basis: CaptureBasis::OsName,
        })
    );
    // Without the leading mark it is not the name macOS wrote.
    let bare = real.trim_start_matches('\u{200f}');
    assert!(!os_name(bare), "{bare:?}");
}

// 7. The near-document hold exempts every kind of capture evidence, and only
// captures. Two places hold near a document: the walk, which holds what sits
// beside a document it finds, and the gate, which holds what sits beside a
// document the plan sees (one that appeared after the walk). Each is checked.
#[test]
fn captures_beside_a_loose_project_document_are_not_held_but_textures_are() {
    let f = Folder::new("near_document");
    f.file("beat.flp");
    f.file("texture.png");
    #[cfg(target_os = "macos")]
    f.marked("quarterly.png");
    f.file("Bildschirmfoto 2026-11-12 um 09.41.12.png");
    f.file("Bildschirmaufnahme 2026-11-12 um 09.41.12.mov");

    // The walk's hold.
    let walked = f.scan();
    check_near_document(&plan::build(&walked, plan::Request::default()));

    // The gate's hold: the document is in the plan's entries, not in the
    // walk's holds, as when it appears between the walk and the plan.
    let mut late = f.scan();
    for (path, _) in std::mem::take(&mut late.project_holds) {
        let mut entry = etude_core::Entry {
            name: path.file_name().unwrap().to_string_lossy().into_owned(),
            ext: path
                .extension()
                .map(|e| e.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default(),
            size: fs::metadata(&path).unwrap().len(),
            modified: None,
            is_dir: false,
            is_package: false,
            capture: None,
            path,
        };
        // As the scan would have recognised it.
        entry.capture = capture::recognise(&entry, scan::has_screen_capture_mark);
        late.entries.push(entry);
    }
    assert!(late.entries.iter().any(|e| e.name == "beat.flp"));
    check_near_document(&plan::build(&late, plan::Request::default()));
}

fn check_near_document(plan: &Plan) {
    assert_eq!(
        untouched(plan, "texture.png"),
        Some(&Untouched::NearProjectDocument("beat.flp".into())),
        "{:?}",
        plan.untouched
    );
    let mut captures = vec![
        "Bildschirmfoto 2026-11-12 um 09.41.12.png",
        "Bildschirmaufnahme 2026-11-12 um 09.41.12.mov",
    ];
    if cfg!(target_os = "macos") {
        captures.push("quarterly.png");
    }
    for name in captures {
        assert!(
            !matches!(
                untouched(plan, name),
                Some(Untouched::NearProjectDocument(_))
            ),
            "{name} is held near the project document: {:?}",
            plan.untouched
        );
        assert!(
            plan.untouched.iter().any(|(p, _)| p.ends_with(name)) || grouped(plan, name),
            "{name} is accounted for"
        );
    }
}

// 6. The personal-record check runs on captures too.
#[test]
fn a_screenshot_of_a_tax_return_is_held_as_personal_not_grouped() {
    let f = Folder::new("tax");
    f.file("Screenshot of tax_return_2023.png");
    f.file("Screenshot 2026-11-10 at 09.41.12.png");
    f.file("Screenshot 2026-11-11 at 09.41.12.png");
    let plan = f.plan();
    assert!(
        matches!(
            untouched(&plan, "Screenshot of tax_return_2023.png"),
            Some(Untouched::LooksPersonal(_))
        ),
        "{:?}",
        plan.untouched
    );
    assert!(!grouped(&plan, "Screenshot of tax_return_2023.png"));
}

// 8. Elsewhere than macOS nothing is asked and nothing is OsAttribute.
#[cfg(not(target_os = "macos"))]
#[test]
fn off_macos_the_probe_asks_nothing() {
    let f = Folder::new("off_macos");
    let files = [
        f.file("notes.png"),
        f.file("Screenshot 2026-11-12 at 09.41.12.png"),
        f.file("Scherm\u{ad}afbeelding 2026-11-12 om 09.41.12.jpg"),
    ];
    scan::reset_receipt();
    for path in &files {
        assert!(!scan::has_screen_capture_mark(path));
        // The probe the scan uses is never even consulted for a basis.
        let entry = etude_core::scan::Entry {
            path: path.clone(),
            name: path.file_name().unwrap().to_string_lossy().into_owned(),
            ext: path.extension().unwrap().to_string_lossy().into_owned(),
            size: 1,
            modified: None,
            is_dir: false,
            is_package: false,
            capture: None,
        };
        assert_ne!(
            capture::recognise(&entry, scan::has_screen_capture_mark).map(|c| c.basis),
            Some(CaptureBasis::OsAttribute)
        );
    }
    let plan = f.plan();
    assert_eq!(receipt("screen_capture_presence"), None);
    assert!(plan.groups.iter().all(|g| !matches!(
        g.signal,
        plan::Signal::ScreenCapture {
            basis: CaptureBasis::OsAttribute,
            ..
        }
    )));
    for entry in f.scan().entries {
        assert_ne!(
            entry.capture.map(|c| c.basis),
            Some(CaptureBasis::OsAttribute),
            "{entry:?}"
        );
    }
    assert_eq!(receipt("screen_capture_presence"), None);
}

// 9 (source half). The probe asks for presence only; the value is never read.
#[test]
fn the_capture_probe_never_requests_the_value() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let function = |file: &str, signature: &str| -> String {
        let indent = &signature[..signature.len() - signature.trim_start().len()];
        let source = fs::read_to_string(src.join(file)).unwrap();
        let production = source
            .split("\n#[cfg(test)]\nmod ")
            .next()
            .unwrap_or_default()
            .to_string();
        production
            .split(signature)
            .nth(1)
            .unwrap_or_else(|| panic!("{file} defines {signature}"))
            .split(&format!("\n{indent}}}\n"))
            .next()
            .unwrap()
            .to_string()
    };

    let probe = function("scan.rs", "pub fn has_screen_capture_mark(path: &Path)");
    assert_eq!(probe.matches("xattr::presence(").count(), 1, "{probe}");
    assert!(!probe.contains("xattr::read"), "{probe}");
    assert!(!probe.contains("getxattr"), "{probe}");
    assert!(probe.contains("capture::ATTRIBUTE"), "{probe}");

    let presence = function("xattr.rs", "pub fn presence(");
    assert!(presence.contains("platform::length(path, name)"));
    assert!(!presence.contains("read"));

    let length = function("xattr.rs", "    pub fn length(");
    let compact: String = length.split_whitespace().collect();
    assert_eq!(compact.matches("getxattr(").count(), 1);
    assert!(
        compact.contains("std::ptr::null_mut(),0,0,XATTR_NOFOLLOW,"),
        "the probe must pass a null buffer of length zero: {length}"
    );
}

#[cfg(target_os = "macos")]
mod marked {
    use super::*;

    // 4. The attribute counts only on a still extension.
    #[test]
    fn the_attribute_counts_on_stills_only() {
        let f = Folder::new("extension_limit");
        f.marked("minutes.txt");
        f.marked("clip.mov");
        f.marked("Screen Recording 2026-11-12 at 09.41.12.mov");
        f.marked("board.png");
        let out = f.scan();
        assert_eq!(capture_of(&out, "minutes.txt"), None);
        assert_eq!(capture_of(&out, "clip.mov"), None);
        assert_eq!(
            capture_of(&out, "Screen Recording 2026-11-12 at 09.41.12.mov"),
            Some(Capture {
                kind: CaptureKind::Recording,
                basis: CaptureBasis::OsName,
            })
        );
        assert_eq!(
            capture_of(&out, "board.png").map(|c| c.basis),
            Some(CaptureBasis::OsAttribute)
        );
    }

    // 5. The group line reports its weakest file, wherever it sits.
    #[test]
    fn the_group_line_names_the_weakest_basis_not_the_first() {
        // "0001.png" sorts before every other name, so a line built from the
        // first file would claim the attribute.
        let f = Folder::new("weakest_known");
        f.marked("0001.png");
        f.file("Bildschirmfoto 2026-11-12 um 09.41.12.png");
        f.file("Screenshot of the login bug.png");
        let plan = f.plan();
        let shots = plan
            .groups
            .iter()
            .find(|g| g.name == "Screenshots")
            .expect("a Screenshots group");
        assert_eq!(shots.members.len(), 3);
        assert_eq!(shots.signal.describe(), "named \"Screenshot ...\"");

        let f = Folder::new("weakest_os");
        f.marked("0001.png");
        f.marked("0002.png");
        f.file("Bildschirmfoto 2026-11-12 um 09.41.12.png");
        let plan = f.plan();
        let shots = plan
            .groups
            .iter()
            .find(|g| g.name == "Screenshots")
            .expect("a Screenshots group");
        assert_eq!(shots.members.len(), 3);
        assert_eq!(shots.signal.describe(), "named by macOS as screen captures");
    }

    // 6 (attribute half). The attribute does not lift the personal hold.
    #[test]
    fn a_marked_still_with_a_personal_name_is_held() {
        let f = Folder::new("marked_personal");
        f.marked("passport_scan.png");
        f.marked("0001.png");
        f.marked("0002.png");
        let out = f.scan();
        assert_eq!(
            capture_of(&out, "passport_scan.png").map(|c| c.basis),
            Some(CaptureBasis::OsAttribute)
        );
        let plan = plan::build(&out, plan::Request::default());
        assert!(
            matches!(
                untouched(&plan, "passport_scan.png"),
                Some(Untouched::LooksPersonal(_))
            ),
            "{:?}",
            plan.untouched
        );
        assert!(!grouped(&plan, "passport_scan.png"));
    }

    // 9 (receipt half). Planning a folder of stills asks the presence
    // question and opens no file.
    #[test]
    fn planning_stills_records_presence_and_no_open() {
        let f = Folder::new("receipt");
        f.marked("0001.png");
        f.file("Bildschirmfoto 2026-11-12 um 09.41.12.png");
        f.file("holiday.jpg");
        scan::reset_receipt();
        let _ = f.plan();
        assert_eq!(receipt("screen_capture_presence"), Some((3, 3, 0)));
        assert_eq!(receipt("file_open"), None);
    }
}
