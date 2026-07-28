//! v0.3 acceptance tests.
//!
//! The invariant under test, from docs/sweep/V03-CONTENT.md:
//!
//! > Content findings can only ever widen refusal. They never create or name a
//! > group.
//!
//! These use a stub inspector rather than `etude-read`, so `etude-core` keeps
//! zero dependencies. The real scanners are tested in `etude-read`; what is
//! tested here is that the engine can only *narrow* what it will touch.

use std::fs;
use std::path::{Path, PathBuf};

use etude_core::plan::{self, Inspector};
use etude_core::scan::{self, ScanConfig};
use etude_core::{Category, Untouched};

fn fixture(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("sweep_content_{tag}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fixtures::build(&root).expect("fixture");
    root
}

fn cleanup(root: &Path) {
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(root.parent().unwrap().join("sweep_fixture_outside"));
}

/// Refuses exactly the fixture files with sensitive contents, by reading them.
struct StubInspector {
    reads: Vec<PathBuf>,
}

impl Inspector for StubInspector {
    fn inspect(&mut self, path: &Path, ext: &str) -> Option<Category> {
        if !matches!(ext, "txt" | "md") {
            return None;
        }
        self.reads.push(path.to_path_buf());
        let body = fs::read_to_string(path).ok()?;
        if body.contains("123-45-6789") {
            return Some(Category::Identity);
        }
        if body.contains("4111 1111 1111 1111") {
            return Some(Category::Financial);
        }
        if body.contains("diagnosis") {
            return Some(Category::Medical);
        }
        if body.contains("PRIVATE KEY") {
            return Some(Category::Credential);
        }
        None
    }
}

#[test]
fn inspection_refuses_files_that_metadata_alone_would_have_moved() {
    // The whole point of v0.3: "notes-a.txt" is an innocent name. Without
    // reading it, sweep has no way to know it holds a Social Security number.
    let root = fixture("widens");
    let out = scan::scan(&root, &ScanConfig::default()).expect("scan");

    let without = plan::build(&out);
    let mut insp = StubInspector { reads: Vec::new() };
    let with = plan::build_with(&out, Some(&mut insp));

    for name in fixtures::INNOCENT_NAMES_SENSITIVE_CONTENT {
        let path = root.canonicalize().unwrap().join(name);
        let refused = |p: &plan::Plan| {
            p.untouched
                .iter()
                .any(|(q, u)| *q == path && matches!(u, Untouched::LooksPersonal(_)))
        };
        assert!(
            !refused(&without) || !with.groups.iter().any(|g| g.members.contains(&path)),
            "setup error for {name}"
        );
        assert!(refused(&with), "content inspection did not refuse {name}");
        assert!(
            !with.groups.iter().any(|g| g.members.contains(&path)),
            "{name} was grouped despite sensitive contents"
        );
    }
    cleanup(&root);
}

#[test]
fn inspection_never_creates_or_renames_a_group() {
    // Reading more must not change WHERE anything goes. Group names and
    // membership may only shrink.
    let root = fixture("names");
    let out = scan::scan(&root, &ScanConfig::default()).expect("scan");

    let without = plan::build(&out);
    let mut insp = StubInspector { reads: Vec::new() };
    let with = plan::build_with(&out, Some(&mut insp));

    let names_without: Vec<&String> = without.groups.iter().map(|g| &g.name).collect();
    let names_with: Vec<&String> = with.groups.iter().map(|g| &g.name).collect();

    // Groups may DISAPPEAR — refusing members can drop a group below its
    // minimum size, which is narrowing and therefore allowed. What may never
    // happen is a group appearing, or a name changing.
    for n in &names_with {
        assert!(
            names_without.contains(n),
            "inspection invented a group named {n:?}; content must only narrow"
        );
    }

    for b in &with.groups {
        let a = without
            .groups
            .iter()
            .find(|g| g.name == b.name)
            .expect("group survived inspection but did not exist without it");
        assert!(
            b.members.len() <= a.members.len(),
            "group {} GREW after inspection — content must only widen refusal",
            b.name
        );
        for m in &b.members {
            assert!(a.members.contains(m), "inspection added {m:?} to a group");
        }
    }

    // And prove the narrowing actually happened, so this test cannot pass by
    // the inspector doing nothing at all.
    assert!(
        names_with.len() < names_without.len()
            || with.untouched.len() > without.untouched.len(),
        "inspection changed nothing; the test proves nothing"
    );
    cleanup(&root);
}

#[test]
fn a_harmless_file_with_the_same_name_shape_is_not_refused() {
    // False positives are cheap but not free: refusing everything would make
    // the feature useless while looking safe.
    let root = fixture("control");
    let out = scan::scan(&root, &ScanConfig::default()).expect("scan");
    let mut insp = StubInspector { reads: Vec::new() };
    let p = plan::build_with(&out, Some(&mut insp));

    let control = root.canonicalize().unwrap().join(fixtures::INNOCENT_CONTROL);
    let refused = p
        .untouched
        .iter()
        .any(|(q, u)| *q == control && matches!(u, Untouched::LooksPersonal(_)));
    assert!(!refused, "a harmless file was refused on content");
    cleanup(&root);
}

#[test]
fn a_file_already_refused_by_name_is_never_opened() {
    // Reading it could only confirm a decision already made, so opening it is
    // pure exposure with no benefit.
    let root = fixture("noreopen");
    let out = scan::scan(&root, &ScanConfig::default()).expect("scan");
    let mut insp = StubInspector { reads: Vec::new() };
    let _ = plan::build_with(&out, Some(&mut insp));

    for name in fixtures::SENSITIVE_NAMES {
        let path = root.canonicalize().unwrap().join(name);
        assert!(
            !insp.reads.contains(&path),
            "{name} was refused by name, then opened anyway"
        );
    }
    cleanup(&root);
}

#[test]
fn accounting_still_balances_with_inspection_on() {
    let root = fixture("balance");
    let out = scan::scan(&root, &ScanConfig::default()).expect("scan");
    let mut insp = StubInspector { reads: Vec::new() };
    let p = plan::build_with(&out, Some(&mut insp));

    let grouped: usize = p.groups.iter().map(|g| g.members.len()).sum();
    assert_eq!(
        grouped + p.untouched.len(),
        out.entries.len(),
        "a file vanished from the accounting when inspection was on"
    );
    cleanup(&root);
}
