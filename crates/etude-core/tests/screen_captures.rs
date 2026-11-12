//! Screen captures in every language, and screen recordings in their own
//! group.
//!
//! A still is recognised by the attribute macOS writes on it
//! (`com.apple.metadata:kMDItemIsScreenCapture`), then by the name macOS
//! gives it in any language it ships, then by the English names sweep always
//! knew. Recordings are recognised by name only. These tests set the
//! attribute through `etude_core::xattr`, with the value a real
//! `screencapture -x` wrote on a GitHub `macos-15` runner (macOS 15.8.2):
//! <https://github.com/risingmoonscholar/etudes/actions/runs/43233341758>.

use std::fs;
use std::path::PathBuf;

use etude_core::Untouched;
use etude_core::evidence::{CaptureBasis, CaptureKind};
use etude_core::plan::{self, Plan, Signal};
use etude_core::scan::{self, ScanConfig, ScanOutcome};

/// The binary plist `true` that `screencapture -x` wrote as the attribute's
/// value on the runner. Only its presence is ever asked.
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
            std::env::temp_dir().join(format!("etude_captures_{label}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        Folder(root.canonicalize().unwrap())
    }

    fn file(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"synthetic").unwrap();
        path
    }

    /// A file that macOS marked as a screen capture.
    #[cfg(target_os = "macos")]
    fn marked(&self, name: &str) -> PathBuf {
        let path = self.file(name);
        etude_core::xattr::write(&path, etude_core::evidence::capture::ATTRIBUTE, MARK)
            .expect("set the screen-capture attribute");
        path
    }

    fn scan_with(&self, cfg: ScanConfig) -> ScanOutcome {
        scan::scan(&self.0, &cfg).expect("scan")
    }

    fn scan(&self) -> ScanOutcome {
        self.scan_with(ScanConfig {
            grace: None,
            ..ScanConfig::default()
        })
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

fn group<'p>(plan: &'p Plan, name: &str) -> Option<&'p plan::Group> {
    plan.groups.iter().find(|g| g.name == name)
}

fn names(paths: &[PathBuf]) -> Vec<String> {
    let mut names: Vec<String> = paths
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// The receipt row for `category`: (attempted, observed, failed), or `None`
/// when the category was never recorded.
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

#[test]
fn a_german_screenshot_name_forms_screenshots() {
    let f = Folder::new("german");
    for i in 1..=3 {
        f.file(&format!("Bildschirmfoto 2026-01-0{i} um 09.00.0{i}.png"));
    }
    let plan = f.plan();
    let shots = group(&plan, "Screenshots").expect("a Screenshots group");
    assert_eq!(shots.members.len(), 3);
    assert_eq!(
        shots.signal,
        Signal::ScreenCapture {
            kind: CaptureKind::Still,
            basis: CaptureBasis::OsName,
        }
    );
    assert_eq!(shots.signal.describe(), "named by macOS as screen captures");
    assert!(group(&plan, "Images").is_none(), "{:?}", plan.groups);
}

#[test]
fn recordings_form_their_own_group_apart_from_stills() {
    let f = Folder::new("recordings");
    for i in 1..=3 {
        f.file(&format!(
            "Screen Recording 2026-01-0{i} at 9.00.0{i} AM.mov"
        ));
        f.file(&format!("Screenshot 2026-01-0{i} at 9.00.0{i} AM.png"));
    }
    let plan = f.plan();
    assert_eq!(
        plan.groups
            .iter()
            .map(|g| g.name.as_str())
            .collect::<Vec<_>>(),
        ["Screenshots", "Screen Recordings"]
    );
    let recordings = group(&plan, "Screen Recordings").unwrap();
    assert_eq!(
        names(&recordings.members),
        [
            "Screen Recording 2026-01-01 at 9.00.01 AM.mov",
            "Screen Recording 2026-01-02 at 9.00.02 AM.mov",
            "Screen Recording 2026-01-03 at 9.00.03 AM.mov",
        ]
    );
    assert_eq!(
        recordings.signal,
        Signal::ScreenCapture {
            kind: CaptureKind::Recording,
            basis: CaptureBasis::OsName,
        }
    );
    assert_eq!(
        recordings.signal.describe(),
        "named by macOS as screen recordings"
    );
    let stills = group(&plan, "Screenshots").unwrap();
    assert!(names(&stills.members).iter().all(|n| n.ends_with(".png")));
    assert_eq!(stills.members.len(), 3);
}

#[test]
fn two_recordings_make_no_folder_of_their_own() {
    let f = Folder::new("two_recordings");
    for i in 1..=2 {
        f.file(&format!(
            "Screen Recording 2026-01-0{i} at 9.00.0{i} AM.mov"
        ));
    }
    let plan = f.plan();
    assert!(plan.groups.is_empty(), "{:?}", plan.groups);
    assert_eq!(plan.no_clear_group(), 2);
}

#[test]
fn a_screenshot_named_like_a_tax_form_is_still_held_as_personal() {
    let f = Folder::new("personal");
    f.file("Screenshot of tax_return_2023.png");
    for i in 1..=3 {
        f.file(&format!("Screenshot 2026-01-0{i} at 9.00.0{i} AM.png"));
    }
    let plan = f.plan();
    assert!(plan.untouched.iter().any(|(path, reason)| {
        path.ends_with("Screenshot of tax_return_2023.png")
            && matches!(reason, Untouched::LooksPersonal(_))
    }));
    assert_eq!(group(&plan, "Screenshots").unwrap().members.len(), 3);
}

#[test]
fn a_recording_beside_a_loose_project_document_is_not_held() {
    let f = Folder::new("recording_near_document");
    f.file("mix.flp");
    f.file("kick.wav");
    for i in 1..=3 {
        f.file(&format!(
            "Bildschirmaufnahme 2026-01-0{i} um 09.00.0{i}.mov"
        ));
    }
    let plan = f.plan();
    assert_eq!(group(&plan, "Screen Recordings").unwrap().members.len(), 3);
    assert!(
        plan.untouched
            .iter()
            .any(|(path, reason)| path.ends_with("kick.wav")
                && matches!(reason, Untouched::NearProjectDocument(_)))
    );
}

#[test]
fn screen_recordings_is_a_reserved_destination() {
    let f = Folder::new("reserved");
    let refused = plan::validate_maps(&[("mov".into(), "screen recordings".into())], &f.0);
    assert!(refused.is_err(), "{refused:?}");
}

/// The scan that stash runs asks for no attribute: stash makes no capture
/// groups, and its declaration does not include the read.
#[test]
fn a_scan_without_the_probe_records_no_attribute_read() {
    let f = Folder::new("no_probe");
    f.file("notes.png");
    f.file("Bildschirmfoto 2026-01-01 um 09.00.01.png");
    scan::reset_receipt();
    let out = f.scan_with(ScanConfig {
        grace: None,
        probe_screen_captures: false,
        ..ScanConfig::default()
    });
    assert_eq!(receipt("screen_capture_presence"), None);
    let shot = out
        .entries
        .iter()
        .find(|e| e.name.starts_with("Bildschirmfoto"))
        .unwrap();
    // The names still count.
    assert_eq!(
        shot.capture.map(|c| c.basis),
        Some(CaptureBasis::OsName),
        "{shot:?}"
    );
}

/// Elsewhere than macOS the probe answers "absent" without a call, so no
/// file is ever `OsAttribute`, and nothing is counted. The name table is
/// compiled in, so the OS names still match.
#[cfg(not(target_os = "macos"))]
#[test]
fn elsewhere_than_macos_only_names_count() {
    let f = Folder::new("elsewhere");
    let plain = f.file("notes.png");
    f.file("Bildschirmfoto 2026-01-01 um 09.00.01.png");
    scan::reset_receipt();
    assert!(!scan::has_screen_capture_mark(&plain));
    let out = f.scan();
    assert_eq!(receipt("screen_capture_presence"), None);
    for entry in &out.entries {
        assert_ne!(
            entry.capture.map(|c| c.basis),
            Some(CaptureBasis::OsAttribute)
        );
    }
    assert!(
        out.entries
            .iter()
            .any(|e| e.capture.map(|c| c.basis) == Some(CaptureBasis::OsName))
    );
}

#[cfg(target_os = "macos")]
mod marked {
    use super::*;
    use etude_core::apply::{self, ApplyFaults};

    #[test]
    fn a_marked_still_with_an_unrelated_name_is_an_os_attribute_capture() {
        let f = Folder::new("marked");
        f.marked("notes.png");
        f.marked("notes.txt");
        f.file("plain.png");
        let out = f.scan();
        let capture = |name: &str| out.entries.iter().find(|e| e.name == name).unwrap().capture;
        assert_eq!(
            capture("notes.png").map(|c| (c.kind, c.basis)),
            Some((CaptureKind::Still, CaptureBasis::OsAttribute))
        );
        // The extension limits the evidence: a text file is never a capture.
        assert_eq!(capture("notes.txt"), None);
        assert_eq!(capture("plain.png"), None);
    }

    /// The probe asks for the attribute's length only, and only of files
    /// with a still extension. It opens no file.
    #[test]
    fn the_receipt_counts_the_probe_and_opens_no_file() {
        let f = Folder::new("receipt");
        f.marked("notes.png");
        f.marked("notes.txt");
        f.file("plain.jpg");
        f.file("clip.mov");
        fs::create_dir(f.0.join("shots.png")).unwrap();
        scan::reset_receipt();
        let plan = f.plan();
        assert!(plan.groups.is_empty());
        // notes.png and plain.jpg. Not the text file, the recording or the
        // folder.
        assert_eq!(receipt("screen_capture_presence"), Some((2, 2, 0)));
        assert_eq!(receipt("file_open"), None);
    }

    #[test]
    fn three_marked_stills_form_screenshots_marked_by_macos() {
        let f = Folder::new("three_marked");
        for i in 1..=3 {
            f.marked(&format!("notes{i}.png"));
        }
        let plan = f.plan();
        let shots = group(&plan, "Screenshots").expect("a Screenshots group");
        assert_eq!(shots.members.len(), 3);
        assert_eq!(
            shots.signal.describe(),
            "marked by macOS as screen captures"
        );
        assert!(
            plan.to_json()
                .contains("\"capture_basis\":\"os_attribute\"")
        );
    }

    /// The output must not claim more than its weakest file supports.
    #[test]
    fn a_group_with_mixed_evidence_names_its_weakest() {
        let f = Folder::new("mixed_os");
        f.marked("notes1.png");
        f.marked("notes2.png");
        f.file("Bildschirmfoto 2026-01-01 um 09.00.01.png");
        let plan = f.plan();
        assert_eq!(
            group(&plan, "Screenshots").unwrap().signal.describe(),
            "named by macOS as screen captures"
        );

        let f = Folder::new("mixed_known");
        f.marked("notes1.png");
        f.file("Bildschirmfoto 2026-01-01 um 09.00.01.png");
        f.file("CleanShot 2026-01-01 at 09.00.01.png");
        let plan = f.plan();
        assert_eq!(
            group(&plan, "Screenshots").unwrap().signal.describe(),
            "named \"Screenshot ...\""
        );
    }

    /// A marked capture is exempt from the hold beside a loose project
    /// document, in the scan and in the gate alike, and apply's check before
    /// each move agrees, so the plan applies.
    #[test]
    fn a_marked_capture_beside_a_loose_project_document_is_not_held() {
        let f = Folder::new("marked_near_document");
        f.file("track.flp");
        f.file("cover.png");
        for i in 1..=3 {
            f.marked(&format!("notes{i}.png"));
        }
        let mut plan = f.plan();
        let shots = group(&plan, "Screenshots").expect("a Screenshots group");
        assert_eq!(
            names(&shots.members),
            ["notes1.png", "notes2.png", "notes3.png"]
        );
        let (held, _) = plan.near_document().expect("cover.png is held");
        assert_eq!(held, 1);

        for g in &mut plan.groups {
            g.accepted = true;
        }
        let report =
            apply::apply_with_faults_for_tests(&plan, "sweep", None, ApplyFaults::default())
                .expect("the plan applies");
        assert_eq!(report.moved, 3);
        assert!(f.0.join("Screenshots/notes1.png").exists());
    }

    /// Beside a project document sweep asks only where the answer decides
    /// something. A folder beside the project's folder is held whole, so
    /// nothing in it is asked about; a still beside the project's folder is
    /// asked, because a capture there is not held.
    #[test]
    fn a_project_folder_is_probed_only_where_the_answer_matters() {
        let f = Folder::new("project_probes");
        f.file("project/song.flp");
        f.marked("project/loose.png");
        f.marked("samples/kick.png");
        f.marked("shot.png");
        f.file("cover.png");
        scan::reset_receipt();
        let out = f.scan_with(ScanConfig {
            grace: None,
            depth: 3,
            ..ScanConfig::default()
        });
        // The project folder is stepped over, unread.
        assert_eq!(out.skipped_project, 1);
        // shot.png and cover.png, beside it.
        assert_eq!(receipt("screen_capture_presence"), Some((2, 2, 0)));
        let kept: Vec<&str> = out.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(kept, ["shot.png"]);
        let held = |name: &str| {
            out.project_holds.iter().any(|(path, reason)| {
                path.ends_with(name)
                    && *reason == Untouched::NearProjectDocument("project/song.flp".into())
            })
        };
        assert!(held("samples/kick.png"), "{:?}", out.project_holds);
        assert!(held("cover.png"), "{:?}", out.project_holds);
    }
}
