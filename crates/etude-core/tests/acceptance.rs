//! Acceptance tests.
//!
//! These are not unit tests. Each one makes a privacy claim falsifiable. They
//! run against the synthetic fixture tree only. No real user file is read.

use std::fs;
use std::path::PathBuf;

use etude_core::Untouched;
use etude_core::plan;
use etude_core::scan::{self, ScanConfig};

/// Isolated fixture root per test, so tests cannot interfere with each other.
fn fixture(tag: &str) -> (PathBuf, fixtures::Fixture) {
    let root = std::env::temp_dir().join(format!("sweep_fx_{tag}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let f = fixtures::build(&root).expect("fixture build");
    (root, f)
}

fn cleanup(root: &PathBuf) {
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(fixtures::outside_dir(root));
}

#[test]
fn no_sensitive_fixture_is_ever_grouped() {
    // The core promise: "organises the obvious and leaves the private alone."
    let (root, fx) = fixture("sensitive");
    let out = scan::scan(&root, &ScanConfig::default()).expect("scan");
    let p = plan::build(&out);

    for s in &fx.sensitive {
        // scan() canonicalizes, which on macOS rewrites /var to /private/var.
        // Any caller holding pre-scan paths must canonicalize to compare. The
        // same requirement undo will have when it verifies journal entries.
        let s = s.canonicalize().expect("fixture path canonicalizes");

        let in_a_group = p.groups.iter().any(|g| g.members.contains(&s));
        assert!(
            !in_a_group,
            "sensitive file was placed in a group: {}",
            s.display()
        );

        let refused = p
            .untouched
            .iter()
            .any(|(path, u)| *path == s && matches!(u, Untouched::LooksPersonal(_)));
        assert!(
            refused,
            "sensitive file was not recognised as personal: {}",
            s.display()
        );
    }
    cleanup(&root);
}

#[test]
fn package_directory_interior_never_appears_in_a_plan() {
    // Walking into a .photoslibrary or .app is a privacy catastrophe.
    let (root, _fx) = fixture("package");
    let out = scan::scan(
        &root,
        &ScanConfig {
            depth: 8,
            ..Default::default()
        },
    )
    .expect("scan");
    let p = plan::build(&out);

    let leaked: Vec<_> = p
        .groups
        .iter()
        .flat_map(|g| g.members.iter())
        .chain(p.untouched.iter().map(|(path, _)| path))
        .filter(|path| path.to_string_lossy().contains(".app/Contents"))
        .collect();

    assert!(
        leaked.is_empty(),
        "package interior leaked into the plan: {leaked:?}"
    );
    cleanup(&root);
}

#[test]
fn escaping_symlinks_are_never_followed() {
    let (root, _fx) = fixture("symlink");
    let out = scan::scan(
        &root,
        &ScanConfig {
            depth: 8,
            ..Default::default()
        },
    )
    .expect("scan");

    for e in &out.entries {
        let s = e.path.to_string_lossy();
        assert!(
            !s.contains("sweep_fixture_outside"),
            "followed an escaping symlink: {s}"
        );
        assert!(
            !s.contains("/etc/passwd"),
            "followed a symlink to a system file: {s}"
        );
    }
    assert!(
        out.skipped_symlink > 0,
        "fixture symlinks were not seen at all"
    );
    cleanup(&root);
}

#[test]
fn hidden_and_credential_directories_are_not_entered() {
    let (root, _fx) = fixture("hidden");
    let out = scan::scan(
        &root,
        &ScanConfig {
            depth: 8,
            ..Default::default()
        },
    )
    .expect("scan");

    for e in &out.entries {
        let s = e.path.to_string_lossy();
        assert!(!s.contains("/.ssh"), ".ssh was entered: {s}");
        assert!(!s.contains("id_rsa"), "a private key was enumerated: {s}");
    }
    cleanup(&root);
}

#[test]
fn plan_is_deterministic_across_runs() {
    // A plan that reorders between runs cannot be reviewed and re-applied.
    let (root, _fx) = fixture("determinism");
    let cfg = ScanConfig::default();

    let a = plan::build(&scan::scan(&root, &cfg).expect("scan a"));
    let b = plan::build(&scan::scan(&root, &cfg).expect("scan b"));

    let names_a: Vec<_> = a
        .groups
        .iter()
        .map(|g| (&g.name, g.members.len()))
        .collect();
    let names_b: Vec<_> = b
        .groups
        .iter()
        .map(|g| (&g.name, g.members.len()))
        .collect();
    assert_eq!(names_a, names_b, "plan is not deterministic");

    for (ga, gb) in a.groups.iter().zip(b.groups.iter()) {
        assert_eq!(
            ga.members, gb.members,
            "group membership reordered between runs"
        );
    }
    cleanup(&root);
}

#[test]
fn no_group_is_named_after_a_sensitive_category() {
    // The naming rule: sweep never coins a label the filesystem did not contain.
    let (root, _fx) = fixture("naming");
    let out = scan::scan(&root, &ScanConfig::default()).expect("scan");
    let p = plan::build(&out);

    let forbidden = [
        "tax",
        "medical",
        "identity",
        "financial",
        "legal",
        "credential",
        "personal",
    ];
    for g in &p.groups {
        let lower = g.name.to_ascii_lowercase();
        for f in forbidden {
            assert!(
                !lower.contains(f),
                "sweep coined a sensitive group name: {} (contains {f})",
                g.name
            );
        }
    }
    cleanup(&root);
}

#[test]
fn a_file_belongs_to_at_most_one_group() {
    let (root, _fx) = fixture("exclusive");
    let out = scan::scan(&root, &ScanConfig::default()).expect("scan");
    let p = plan::build(&out);

    let mut seen: Vec<&PathBuf> = Vec::new();
    for g in &p.groups {
        for m in &g.members {
            assert!(!seen.contains(&m), "file is in two groups: {}", m.display());
            seen.push(m);
        }
    }
    cleanup(&root);
}

#[test]
fn every_scanned_file_is_either_grouped_or_explained() {
    // No file may silently vanish from the accounting.
    let (root, _fx) = fixture("accounting");
    let out = scan::scan(&root, &ScanConfig::default()).expect("scan");
    let p = plan::build(&out);

    let grouped: usize = p.groups.iter().map(|g| g.members.len()).sum();
    assert_eq!(
        grouped + p.untouched.len(),
        out.entries.len(),
        "accounting does not balance: {} grouped + {} untouched != {} scanned",
        grouped,
        p.untouched.len(),
        out.entries.len()
    );
    cleanup(&root);
}

#[test]
fn the_screenshot_group_is_found_and_correctly_sized() {
    // The wow moment must actually fire on a realistic tree.
    let (root, _fx) = fixture("screenshots");
    let out = scan::scan(&root, &ScanConfig::default()).expect("scan");
    let p = plan::build(&out);

    let shots = p
        .groups
        .iter()
        .find(|g| g.name == "Screenshots")
        .expect("no Screenshots group");
    assert_eq!(shots.members.len(), 34, "screenshot count wrong");
    cleanup(&root);
}

/// A project folder is refused as a scan root, not sorted into type folders.
///
/// The case depth cannot cover. Not descending protects a project that sits
/// INSIDE the folder being swept; it does nothing when the project IS that
/// folder, which is what happens when someone changes into their track and
/// runs the tidy tool. Its bounces and renders are the immediate children
/// then, and without this they move: measured at eight files into Media/,
/// away from the .als that references them by relative path.
#[test]
fn a_project_folder_is_refused_as_a_scan_root() {
    let root = std::env::temp_dir().join(format!("sweep_proj_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("mkdir");
    fs::write(root.join("MyTrack.als"), b"synthetic").expect("write");
    for i in 0..5 {
        fs::write(root.join(format!("bounce_{i}.wav")), b"synthetic").expect("write");
    }

    match scan::scan(&root, &ScanConfig::default()) {
        Err(scan::ScanError::RefusedProjectRoot { marker, .. }) => {
            assert!(
                marker.to_lowercase().contains("als"),
                "the refusal should name the file that made this a project, got {marker:?}"
            );
        }
        Err(other) => panic!("refused for the wrong reason: {other:?}"),
        Ok(out) => panic!(
            "a folder holding MyTrack.als was scanned anyway: {} entries would be grouped",
            out.entries.len()
        ),
    }

    let _ = fs::remove_dir_all(&root);
}

/// The refusal is about the ROOT, not about projects existing anywhere.
///
/// A folder that merely contains project folders is ordinary and must still
/// be sweepable -- refusing it would make the guard useless for the case it
/// was built for, someone tidying the folder their projects live in.
///
/// The fixture holds a Godot project, not the Ableton one it used to. A
/// project.godot marks the project ROOT, so the project is exactly that
/// folder and this parent really is ordinary. An .als does not: it
/// references its samples relative to itself and freely upward, so a folder
/// holding one is NOT safe to sweep, and the companion test below pins that.
/// Using .als here made this test assert something untrue.
#[test]
fn a_folder_containing_projects_is_still_sweepable() {
    let root = std::env::temp_dir().join(format!("sweep_projparent_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("TrackOne")).expect("mkdir");
    fs::write(root.join("TrackOne/project.godot"), b"synthetic").expect("write");
    for i in 0..4 {
        fs::write(root.join(format!("note_{i}.pdf")), b"synthetic").expect("write");
    }

    let out = scan::scan(&root, &ScanConfig::default())
        .expect("a folder that merely contains a project is ordinary");
    assert!(
        out.entries.iter().any(|e| e.ext == "pdf"),
        "the parent folder's own files should still be considered"
    );

    let _ = fs::remove_dir_all(&root);
}

/// A file changed inside the grace window is left alone.
///
/// The case: someone downloads something, starts using it, and runs the tidy
/// tool an hour later. Moving it then is moving something out from under a
/// person mid-task. The window is on mtime, never atime -- Spotlight and any
/// backup agent touch atime just by looking, so an atime window would protect
/// a whole folder forever after one reindex.
#[test]
fn a_file_changed_within_the_grace_window_is_not_grouped() {
    let root = std::env::temp_dir().join(format!("sweep_grace_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("mkdir");
    // Four fresh .pdf files: enough to form a Documents group if nothing held
    // them back.
    for i in 0..4 {
        fs::write(root.join(format!("paper_{i}.pdf")), b"synthetic").expect("write");
    }

    let out = scan::scan(&root, &ScanConfig::default()).expect("scan");
    let p = plan::build(&out);
    assert_eq!(
        p.too_recent(),
        4,
        "four just-written files should all be inside the window"
    );
    assert!(
        p.groups.is_empty(),
        "a group formed from files written seconds ago: {:?}",
        p.groups.iter().map(|g| &g.name).collect::<Vec<_>>()
    );

    // And the window is not a wall: with it off, the same files group.
    let no_grace = ScanConfig {
        grace: None,
        ..Default::default()
    };
    let out2 = scan::scan(&root, &no_grace).expect("scan");
    let p2 = plan::build(&out2);
    assert_eq!(
        p2.groups.len(),
        1,
        "with the window off the same four files should form one group"
    );

    let _ = fs::remove_dir_all(&root);
}

/// A download still in flight is left alone regardless of age.
///
/// Moving one leaves a partial file at a destination the downloader is not
/// writing to, and it never completes. Checked independently of the grace
/// window because a stalled download from last week is still in flight.
#[test]
fn a_download_in_flight_is_never_grouped() {
    let root = std::env::temp_dir().join(format!("sweep_inflight_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("mkdir");
    for name in ["movie.mp4.part", "album.zip.crdownload", "iso.dmg.download"] {
        fs::write(root.join(name), b"partial").expect("write");
    }

    // Grace off, so age cannot be what protects them.
    let cfg = ScanConfig {
        grace: None,
        ..Default::default()
    };
    let out = scan::scan(&root, &cfg).expect("scan");
    let p = plan::build(&out);

    assert_eq!(
        p.in_flight(),
        3,
        "all three in-flight downloads should be recognised as such"
    );
    for g in &p.groups {
        for m in &g.members {
            let n = m.to_string_lossy();
            assert!(
                !n.contains(".part") && !n.contains(".crdownload") && !n.contains(".download"),
                "an in-flight download was grouped into {:?}: {n}",
                g.name
            );
        }
    }

    let _ = fs::remove_dir_all(&root);
}

/// A project nested inside a swept folder is stepped over, not descended into.
///
/// The root check alone was not enough. It protects someone standing IN their
/// project; it does nothing for someone sweeping the folder their projects
/// live in. Verified before this guard existed: a Downloads folder holding a
/// Godot project, scanned at depth 2, produced an Images group of that
/// project's captures while its project.godot sat listed as ungrouped.
/// Applying that plan would have reorganised the project.
///
/// The folder AROUND the project stays ordinary -- its own loose files still
/// group. Only the project is off limits.
#[test]
fn a_project_nested_in_a_swept_folder_is_not_descended_into() {
    let root = std::env::temp_dir().join(format!("sweep_nested_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("ad-astra")).expect("mkdir");
    fs::write(root.join("ad-astra/project.godot"), b"synthetic").expect("write");
    for i in 0..4 {
        fs::write(root.join(format!("ad-astra/capture_{i}.png")), b"x").expect("write");
    }
    for i in 0..3 {
        fs::write(root.join(format!("invoice_{i}.pdf")), b"x").expect("write");
    }

    let cfg = ScanConfig {
        depth: 3,
        grace: None,
        ..Default::default()
    };
    let out = scan::scan(&root, &cfg).expect("the folder AROUND a project is ordinary");

    assert_eq!(
        out.skipped_project, 1,
        "the nested project was not counted as skipped, so nothing would tell \
         the user their project was deliberately left alone"
    );
    for e in &out.entries {
        assert!(
            !e.path.to_string_lossy().contains("ad-astra"),
            "a file from inside the project was scanned at depth 3: {}",
            e.path.display()
        );
    }

    let p = plan::build(&out);
    assert!(
        p.groups.iter().any(|g| g.name == "Documents"),
        "the folder's own loose invoices should still group; only the project is off limits"
    );
    for g in &p.groups {
        for m in &g.members {
            assert!(
                !m.to_string_lossy().contains("capture_"),
                "a project's internal file was grouped into {:?}",
                g.name
            );
        }
    }

    let _ = fs::remove_dir_all(&root);
}
