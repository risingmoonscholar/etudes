//! stash acceptance tests.
//!
//! stash's promise is narrower than sweep's and therefore easier to falsify:
//! the folder is empty afterwards, and everything comes back.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use etude_core::apply;
use etude_core::journal::{Journal, Sealer};
use etude_core::plan::{Group, Plan, Signal};
use etude_core::scan::{self, ScanConfig};

/// `ETUDE_STATE_DIR` is process-global; serialise rather than rely on a flag.
fn lock() -> MutexGuard<'static, ()> {
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

struct TestSeal;
impl Sealer for TestSeal {
    fn seal(&self, p: &[u8]) -> Result<Vec<u8>, &'static str> {
        Ok(p.iter().map(|b| b ^ 0x5a).collect())
    }
    fn open(&self, s: &[u8]) -> Result<Vec<u8>, &'static str> {
        Ok(s.iter().map(|b| b ^ 0x5a).collect())
    }
}

fn setup(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("stash_{tag}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fixtures::build(&root).expect("fixture");
    let state = std::env::temp_dir().join(format!("stash_state_{tag}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&state);
    unsafe { std::env::set_var("ETUDE_STATE_DIR", &state) };
    root
}

fn cleanup(root: &Path) {
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(fixtures::outside_dir(root));
}

/// Mirrors what the binary builds: one group, everything in it.
fn stash_plan(root: &Path) -> (Plan, usize) {
    let cfg = ScanConfig {
        depth: 1,
        allow_sync: true,
        whole_units: true,
        ..Default::default()
    };
    let out = scan::scan(root, &cfg).expect("scan");
    let members: Vec<PathBuf> = out.entries.iter().map(|e| e.path.clone()).collect();
    let count = members.len();
    (
        Plan {
            root: out.root.clone(),
            groups: vec![Group {
                name: ".stash-0".into(),
                signal: Signal::Collected { count },
                members,
                accepted: true,
            }],
            untouched: Vec::new(),
            scanned: count,
            skipped_hidden: out.skipped_hidden,
            skipped_symlink: out.skipped_symlink,
            skipped_system: out.skipped_system,
            skipped_project: 0,
            skipped_in_flight: 0,
            skipped_bundle: 0,
            skipped_unreadable: out.skipped_unreadable,
            root_is_synced: out.root_is_synced,
            allow_sync: out.allow_sync,
        },
        count,
    )
}

/// Visible (non-hidden) entries directly in `root`.
fn visible(root: &Path) -> usize {
    fs::read_dir(root)
        .map(|rd| {
            rd.flatten()
                .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
                .count()
        })
        .unwrap_or(0)
}

#[test]
fn the_folder_is_actually_empty_afterwards() {
    // "Your Desktop is clear" has to be literally true, including directories
    // and symlinks. Those are the two things sweep's scanner deliberately
    // does not return. That is the reason whole_units exists.
    let _g = lock();
    let root = setup("clear");
    assert!(visible(&root) > 0, "fixture produced nothing to stash");

    let (plan, count) = stash_plan(&root);
    let r = apply::apply(&plan, "stash", Some(&TestSeal), None).expect("apply");

    assert_eq!(r.moved, count);
    assert_eq!(visible(&root), 0, "items remained visible after stashing");
    cleanup(&root);
}

#[test]
fn everything_comes_back_including_directories_and_symlinks() {
    let _g = lock();
    let root = setup("roundtrip");
    let (plan, count) = stash_plan(&root);
    let before: Vec<PathBuf> = plan.groups[0].members.clone();

    let rep = apply::apply(&plan, "stash", Some(&TestSeal), None).expect("apply");
    let mut j = Journal::load_sealed("stash", &rep.journal_id, &TestSeal).expect("journal");
    let ur = apply::undo(&mut j, &TestSeal);

    assert!(ur.error.is_none(), "unexpected undo error: {:?}", ur.error);
    assert_eq!(ur.restored, count, "not everything was restored");
    assert!(
        ur.skipped_changed.is_empty(),
        "items were wrongly reported as changed: {:?}",
        ur.skipped_changed
    );
    for p in &before {
        assert!(
            p.symlink_metadata().is_ok(),
            "missing after restore: {}",
            p.display()
        );
    }
    cleanup(&root);
}

#[test]
fn a_symlink_is_fingerprinted_by_the_link_not_its_target() {
    // Regression: fingerprint used fs::metadata, which follows. A link pointing
    // at the folder being emptied changed identity when the folder emptied, and
    // undo then refused to restore it.
    let _g = lock();
    let root = setup("symlink");
    let (plan, _) = stash_plan(&root);

    let rep = apply::apply(&plan, "stash", Some(&TestSeal), None).expect("apply");
    let mut j = Journal::load_sealed("stash", &rep.journal_id, &TestSeal).expect("journal");
    let ur = apply::undo(&mut j, &TestSeal);

    assert!(ur.error.is_none(), "unexpected undo error: {:?}", ur.error);
    let self_link = root.canonicalize().unwrap().join("self_link");
    assert!(
        !ur.skipped_changed.iter().any(|p| p.ends_with("self_link")),
        "a symlink was judged changed because its target moved"
    );
    assert!(
        self_link.symlink_metadata().is_ok(),
        "the symlink was not restored"
    );
    cleanup(&root);
}

#[test]
fn stash_moves_the_files_sweep_refuses() {
    // The deliberate divergence from sweep. Clearing a folder for a screen
    // share is pointless if the tax scan stays on the Desktop.
    let _g = lock();
    let root = setup("sensitive");
    let (plan, _) = stash_plan(&root);

    for name in fixtures::SENSITIVE_NAMES {
        let p = root.canonicalize().unwrap().join(name);
        assert!(
            plan.groups[0].members.contains(&p),
            "stash left a sensitive file behind: {name}"
        );
    }

    apply::apply(&plan, "stash", Some(&TestSeal), None).expect("apply");
    for name in fixtures::SENSITIVE_NAMES {
        assert!(
            !root.canonicalize().unwrap().join(name).exists(),
            "{name} was still in the open folder after stashing"
        );
    }
    cleanup(&root);
}

#[test]
fn hidden_items_are_left_where_they_are() {
    // .ssh and friends are never touched, in either tool.
    let _g = lock();
    let root = setup("hidden");
    let (plan, _) = stash_plan(&root);
    apply::apply(&plan, "stash", Some(&TestSeal), None).expect("apply");

    assert!(
        root.join(".ssh").exists(),
        "stash moved a hidden credential directory"
    );
    cleanup(&root);
}
