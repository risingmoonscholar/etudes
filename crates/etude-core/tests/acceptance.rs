//! Acceptance tests from docs/sweep/PLAN.md.
//!
//! These are not unit tests. Each one makes a privacy claim falsifiable. They
//! run against the synthetic fixture tree only — no real user file is read.

use std::fs;
use std::path::PathBuf;

use etude_core::plan;
use etude_core::scan::{self, ScanConfig};
use etude_core::Untouched;

/// Isolated fixture root per test, so tests cannot interfere with each other.
fn fixture(tag: &str) -> (PathBuf, fixtures::Fixture) {
    let root = std::env::temp_dir().join(format!("sweep_fx_{tag}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let f = fixtures::build(&root).expect("fixture build");
    (root, f)
}

fn cleanup(root: &PathBuf) {
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(root.parent().unwrap().join("sweep_fixture_outside"));
}

#[test]
fn no_sensitive_fixture_is_ever_grouped() {
    // The core promise: "organises the obvious and leaves the private alone."
    let (root, fx) = fixture("sensitive");
    let out = scan::scan(&root, &ScanConfig::default()).expect("scan");
    let p = plan::build(&out);

    for s in &fx.sensitive {
        // scan() canonicalizes, which on macOS rewrites /var to /private/var.
        // Any caller holding pre-scan paths must canonicalize to compare — the
        // same requirement undo will have when it verifies journal entries.
        let s = s.canonicalize().expect("fixture path canonicalizes");

        let in_a_group = p.groups.iter().any(|g| g.members.contains(&s));
        assert!(!in_a_group, "sensitive file was placed in a group: {}", s.display());

        let refused = p
            .untouched
            .iter()
            .any(|(path, u)| *path == s && matches!(u, Untouched::LooksPersonal(_)));
        assert!(refused, "sensitive file was not recognised as personal: {}", s.display());
    }
    cleanup(&root);
}

#[test]
fn package_directory_interior_never_appears_in_a_plan() {
    // Walking into a .photoslibrary or .app is a privacy catastrophe.
    let (root, _fx) = fixture("package");
    let out = scan::scan(&root, &ScanConfig { depth: 8, ..Default::default() }).expect("scan");
    let p = plan::build(&out);

    let leaked: Vec<_> = p
        .groups
        .iter()
        .flat_map(|g| g.members.iter())
        .chain(p.untouched.iter().map(|(path, _)| path))
        .filter(|path| path.to_string_lossy().contains(".app/Contents"))
        .collect();

    assert!(leaked.is_empty(), "package interior leaked into the plan: {leaked:?}");
    cleanup(&root);
}

#[test]
fn escaping_symlinks_are_never_followed() {
    let (root, _fx) = fixture("symlink");
    let out = scan::scan(&root, &ScanConfig { depth: 8, ..Default::default() }).expect("scan");

    for e in &out.entries {
        let s = e.path.to_string_lossy();
        assert!(!s.contains("sweep_fixture_outside"), "followed an escaping symlink: {s}");
        assert!(!s.contains("/etc/passwd"), "followed a symlink to a system file: {s}");
    }
    assert!(out.skipped_symlink > 0, "fixture symlinks were not seen at all");
    cleanup(&root);
}

#[test]
fn hidden_and_credential_directories_are_not_entered() {
    let (root, _fx) = fixture("hidden");
    let out = scan::scan(&root, &ScanConfig { depth: 8, ..Default::default() }).expect("scan");

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

    let names_a: Vec<_> = a.groups.iter().map(|g| (&g.name, g.members.len())).collect();
    let names_b: Vec<_> = b.groups.iter().map(|g| (&g.name, g.members.len())).collect();
    assert_eq!(names_a, names_b, "plan is not deterministic");

    for (ga, gb) in a.groups.iter().zip(b.groups.iter()) {
        assert_eq!(ga.members, gb.members, "group membership reordered between runs");
    }
    cleanup(&root);
}

#[test]
fn no_group_is_named_after_a_sensitive_category() {
    // The naming rule: sweep never coins a label the filesystem did not contain.
    let (root, _fx) = fixture("naming");
    let out = scan::scan(&root, &ScanConfig::default()).expect("scan");
    let p = plan::build(&out);

    let forbidden = ["tax", "medical", "identity", "financial", "legal", "credential", "personal"];
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

    let shots = p.groups.iter().find(|g| g.name == "Screenshots").expect("no Screenshots group");
    assert_eq!(shots.members.len(), 34, "screenshot count wrong");
    cleanup(&root);
}
