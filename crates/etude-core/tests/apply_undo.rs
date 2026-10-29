//! M5/M6 acceptance tests.
//!
//! These prove the reversibility and resumability claims. Each runs against an
//! isolated synthetic tree with its own state directory, so journals never mix
//! and no real user file is involved.

use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

use etude_core::apply::{self, ApplyError, ProgressPhase};
use etude_core::journal::{Entry, Journal, Method, ValidatedDirectory};
use etude_core::plan::{self, BindingContext, BoundPlan, Plan};
use etude_core::scan::{self, ScanConfig};

/// `ETUDE_STATE_DIR` is process-global, so these tests cannot run concurrently
/// without clobbering each other's journals. Serialise here rather than relying
/// on `--test-threads=1`: a suite that only passes with a special flag is a
/// suite that will be run wrongly.
fn lock() -> MutexGuard<'static, ()> {
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    // A panicking test poisons the mutex; the state is per-test anyway, so
    // recovering keeps one failure from cascading into false failures.
    L.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Test sealer. Not a cipher. It proves the *plumbing* is sealed-only. The
/// real cipher is tested in etude-keep, and the end-to-end pairing is tested in
/// the CLI integration test.
struct TestSeal;
impl etude_core::journal::Sealer for TestSeal {
    fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>, &'static str> {
        let mut v = b"TESTSEAL".to_vec();
        v.extend(plaintext.iter().map(|b| b ^ 0x5a));
        Ok(v)
    }
    fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, &'static str> {
        let body = sealed
            .strip_prefix(b"TESTSEAL".as_slice())
            .ok_or("bad header")?;
        Ok(body.iter().map(|b| b ^ 0x5a).collect())
    }
}

fn context() -> BindingContext {
    BindingContext::new(
        "test",
        env!("CARGO_PKG_VERSION"),
        "fixture-v1",
        "metadata-v1",
    )
}

fn proposal_from_scan(out: &scan::ScanOutcome, groups: Vec<plan::Group>) -> BoundPlan {
    BoundPlan::from_plan(Plan::with_groups(out, groups), context()).expect("bind fixture proposal")
}

fn directory_guard(path: &std::path::Path) -> ValidatedDirectory {
    let metadata = fs::symlink_metadata(path).expect("stat guarded directory");
    #[cfg(unix)]
    let (device, inode) = {
        use std::os::unix::fs::MetadataExt;
        (metadata.dev(), metadata.ino())
    };
    #[cfg(not(unix))]
    let (device, inode) = (0, 0);
    ValidatedDirectory {
        path: path.to_path_buf(),
        device,
        inode,
    }
}

fn manual_proposal(
    root: &std::path::Path,
    groups: Vec<plan::Group>,
    allow_sync: bool,
) -> BoundPlan {
    let out = scan::scan(
        root,
        &ScanConfig {
            depth: 8,
            allow_sync,
            whole_units: false,
            ..Default::default()
        },
    )
    .expect("scan completed fixture");
    proposal_from_scan(&out, groups)
}

fn setup(tag: &str) -> (PathBuf, fixtures::Fixture, BoundPlan) {
    let root = std::env::temp_dir().join(format!("sweep_au_{tag}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let fx = fixtures::build(&root).expect("fixture");

    // Per-test state dir so journals cannot collide between tests.
    let state = std::env::temp_dir().join(format!("sweep_state_{tag}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&state);
    unsafe { std::env::set_var("ETUDE_STATE_DIR", &state) };

    let out = scan::scan(&root, &ScanConfig::default()).expect("scan");
    let mut p = plan::build(&out);
    for g in &mut p.groups {
        g.accepted = true;
    }
    let p = BoundPlan::from_plan(p, context()).expect("bind fixture plan");
    (root, fx, p)
}

fn cleanup(root: &PathBuf) {
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(fixtures::outside_dir(root));
}

#[test]
fn sensitive_files_survive_a_full_apply_untouched() {
    let _g = lock();
    // The strongest claim in the product, checked against the filesystem rather
    // than against the plan.
    let (root, fx, p) = setup("sensitive");

    let before: Vec<(PathBuf, u64)> = fx
        .sensitive
        .iter()
        .map(|s| {
            let c = s.canonicalize().expect("canon");
            let len = fs::metadata(&c).expect("meta").len();
            (c, len)
        })
        .collect();

    apply::apply(&p, &context(), Some(&TestSeal), None).expect("apply");

    for (path, len) in before {
        assert!(
            path.exists(),
            "a sensitive file was moved away from {}",
            path.display()
        );
        assert_eq!(
            fs::metadata(&path).expect("meta").len(),
            len,
            "sensitive file was modified"
        );
    }
    cleanup(&root);
}

#[test]
fn apply_then_undo_restores_every_path() {
    let _g = lock();
    let (root, _fx, p) = setup("roundtrip");

    let expected: Vec<PathBuf> = p
        .groups
        .iter()
        .flat_map(|g| g.members.iter().cloned())
        .collect();
    assert!(!expected.is_empty(), "fixture produced no moves to test");

    let rep = apply::apply(&p, &context(), Some(&TestSeal), None).expect("apply");
    assert_eq!(rep.moved, expected.len());
    for src in &expected {
        assert!(
            !src.exists(),
            "source still present after apply: {}",
            src.display()
        );
    }

    let mut j = Journal::load_sealed("test", &rep.journal_id, &TestSeal).expect("journal loads");
    let ur = apply::undo(&mut j, &TestSeal);

    assert!(ur.error.is_none(), "unexpected undo error: {:?}", ur.error);
    assert_eq!(
        ur.restored,
        expected.len(),
        "undo did not restore every file"
    );
    assert!(
        ur.skipped_changed.is_empty(),
        "undo skipped unchanged files"
    );
    for src in &expected {
        assert!(src.exists(), "file not restored: {}", src.display());
    }
    cleanup(&root);
}

#[test]
fn undo_keeps_a_preexisting_empty_destination_directory() {
    let _g = lock();
    let root = std::env::temp_dir().join(format!("sweep_au_existing_empty_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("Existing")).expect("create user folder");
    let sources: Vec<_> = (0..3)
        .map(|i| {
            let path = root.join(format!("file{i}.qoi"));
            fs::write(&path, b"fixture").expect("write source");
            path
        })
        .collect();
    let state =
        std::env::temp_dir().join(format!("sweep_state_existing_empty_{}", std::process::id()));
    let _ = fs::remove_dir_all(&state);
    unsafe { std::env::set_var("ETUDE_STATE_DIR", &state) };
    let out = scan::scan(&root, &ScanConfig::default()).expect("scan user folder");
    let p = proposal_from_scan(
        &out,
        vec![plan::Group {
            name: "Existing".into(),
            signal: plan::Signal::Collected {
                count: sources.len(),
            },
            members: sources,
            accepted: true,
        }],
    );
    let report = apply::apply(&p, &context(), Some(&TestSeal), None).expect("apply");
    let mut journal = Journal::load_sealed("test", &report.journal_id, &TestSeal).expect("journal");
    let undo = apply::undo(&mut journal, &TestSeal);
    assert!(undo.error.is_none(), "undo error: {:?}", undo.error);
    assert!(
        root.join("Existing").is_dir(),
        "undo removed the pre-existing empty folder"
    );
    cleanup(&root);
    let _ = fs::remove_dir_all(state);
}

#[cfg(unix)]
#[test]
fn undo_removes_only_the_empty_destination_directory_it_created() {
    let _g = lock();
    let root = std::env::temp_dir().join(format!("sweep_au_created_dir_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create root");
    let source = root.join("file.qoi");
    fs::write(&source, b"fixture").expect("write source");
    let state =
        std::env::temp_dir().join(format!("sweep_state_created_dir_{}", std::process::id()));
    let _ = fs::remove_dir_all(&state);
    unsafe { std::env::set_var("ETUDE_STATE_DIR", &state) };
    let out = scan::scan(&root, &ScanConfig::default()).expect("scan source");
    let p = proposal_from_scan(
        &out,
        vec![plan::Group {
            name: "NewFolder".into(),
            signal: plan::Signal::Collected { count: 1 },
            members: vec![source],
            accepted: true,
        }],
    );
    let report = apply::apply(&p, &context(), Some(&TestSeal), None).expect("apply");
    let destination = root.join("NewFolder");
    assert!(destination.is_dir(), "apply did not create its destination");
    let mut journal = Journal::load_sealed("test", &report.journal_id, &TestSeal).expect("journal");
    let undo = apply::undo(&mut journal, &TestSeal);
    assert!(undo.error.is_none(), "undo error: {:?}", undo.error);
    assert!(
        !destination.exists(),
        "undo left its empty destination behind"
    );
    cleanup(&root);
    let _ = fs::remove_dir_all(state);
}

/// Journal state of an undo killed after its last file restore: the file is
/// home and recorded as reversed, and the folder apply created is still there.
/// Built directly, with no timing, from a real apply.
fn journal_with_every_file_reversed(tag: &str) -> (PathBuf, PathBuf, PathBuf, Journal) {
    let root = std::env::temp_dir().join(format!("sweep_au_{tag}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create root");
    let source = root.join("file.qoi");
    fs::write(&source, b"fixture").expect("write source");
    let state = std::env::temp_dir().join(format!("sweep_state_{tag}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&state);
    unsafe { std::env::set_var("ETUDE_STATE_DIR", &state) };
    let out = scan::scan(&root, &ScanConfig::default()).expect("scan source");
    let p = proposal_from_scan(
        &out,
        vec![plan::Group {
            name: "NewFolder".into(),
            signal: plan::Signal::Collected { count: 1 },
            members: vec![source.clone()],
            accepted: true,
        }],
    );
    let report = apply::apply(&p, &context(), Some(&TestSeal), None).expect("apply");
    let journal = Journal::load_sealed("test", &report.journal_id, &TestSeal).expect("journal");
    assert!(root.join("NewFolder").is_dir(), "apply made no folder");
    assert_eq!(
        journal.created_directories.len(),
        1,
        "apply did not record the folder it created"
    );
    for (i, e) in journal.entries.iter().enumerate() {
        fs::rename(&e.to, &e.from).expect("put file home");
        journal
            .record_undone(i, &TestSeal)
            .expect("record reversal");
    }
    let journal = Journal::load_sealed("test", &journal.id, &TestSeal).expect("reload journal");
    assert!(
        journal
            .entries
            .iter()
            .all(|e| e.state == etude_core::journal::EntryState::Reversed),
        "fixture must have every file entry reversed"
    );
    (root.clone(), root.join("NewFolder"), state, journal)
}

#[cfg(unix)]
#[test]
fn second_undo_removes_the_created_folder_when_every_file_is_already_back() {
    let _g = lock();
    let (root, folder, state, mut journal) = journal_with_every_file_reversed("second_undo");
    assert!(
        apply::has_undo_work(&journal, &TestSeal),
        "undo reports nothing to do while the folder it must remove is still there"
    );
    let report = apply::undo(&mut journal, &TestSeal);
    assert!(report.error.is_none(), "undo error: {:?}", report.error);
    assert!(!folder.exists(), "second undo left the created folder");
    assert_eq!(fs::read(root.join("file.qoi")).unwrap(), b"fixture");
    assert!(
        !apply::has_undo_work(&journal, &TestSeal),
        "undo still has work after the folder is gone"
    );
    cleanup(&root);
    let _ = fs::remove_dir_all(state);
}

#[cfg(unix)]
#[test]
fn second_undo_leaves_a_created_folder_that_is_not_empty() {
    let _g = lock();
    let (root, folder, state, mut journal) = journal_with_every_file_reversed("not_empty");
    fs::write(folder.join("user-file"), b"theirs").expect("user adds a file");
    assert!(
        !apply::has_undo_work(&journal, &TestSeal),
        "a folder that cannot be removed must not count as undo work"
    );
    let report = apply::undo(&mut journal, &TestSeal);
    assert!(report.error.is_none(), "undo error: {:?}", report.error);
    assert_eq!(fs::read(folder.join("user-file")).unwrap(), b"theirs");
    cleanup(&root);
    let _ = fs::remove_dir_all(state);
}

#[cfg(unix)]
#[test]
fn second_undo_leaves_a_folder_the_journal_does_not_record_as_created() {
    let _g = lock();
    let (root, folder, state, mut journal) = journal_with_every_file_reversed("not_recorded");
    journal.created_directories.clear();
    assert!(!apply::has_undo_work(&journal, &TestSeal));
    let report = apply::undo(&mut journal, &TestSeal);
    assert!(report.error.is_none(), "undo error: {:?}", report.error);
    assert!(folder.is_dir(), "undo removed a folder it did not create");
    cleanup(&root);
    let _ = fs::remove_dir_all(state);
}

#[cfg(unix)]
#[test]
fn second_undo_leaves_a_created_folder_that_was_swapped_for_another() {
    let _g = lock();
    let (root, folder, state, mut journal) = journal_with_every_file_reversed("swapped");
    let reserved = root.join("reserved replacement");
    fs::create_dir(&reserved).expect("reserve a distinct directory");
    fs::remove_dir(&folder).expect("remove original");
    fs::rename(&reserved, &folder).expect("install replacement");
    assert!(!apply::has_undo_work(&journal, &TestSeal));
    let report = apply::undo(&mut journal, &TestSeal);
    assert!(report.error.is_none(), "undo error: {:?}", report.error);
    assert!(folder.is_dir(), "undo removed a replacement folder");
    cleanup(&root);
    let _ = fs::remove_dir_all(state);
}

#[cfg(unix)]
#[test]
fn second_undo_leaves_a_created_folder_that_was_swapped_for_a_link() {
    let _g = lock();
    let (root, folder, state, mut journal) = journal_with_every_file_reversed("linked");
    let elsewhere = root.join("elsewhere");
    fs::create_dir(&elsewhere).expect("create link target");
    fs::remove_dir(&folder).expect("remove original");
    std::os::unix::fs::symlink(&elsewhere, &folder).expect("install link");
    assert!(!apply::has_undo_work(&journal, &TestSeal));
    let report = apply::undo(&mut journal, &TestSeal);
    assert!(report.error.is_none(), "undo error: {:?}", report.error);
    assert!(
        fs::symlink_metadata(&folder)
            .expect("link still there")
            .file_type()
            .is_symlink()
    );
    assert!(elsewhere.is_dir());
    cleanup(&root);
    let _ = fs::remove_dir_all(state);
}

#[cfg(unix)]
#[test]
fn undo_does_not_remove_a_replacement_for_its_created_directory() {
    let _g = lock();
    let root = std::env::temp_dir().join(format!("sweep_au_replaced_dir_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create root");
    let source = root.join("file.qoi");
    fs::write(&source, b"fixture").expect("write source");
    let state =
        std::env::temp_dir().join(format!("sweep_state_replaced_dir_{}", std::process::id()));
    let _ = fs::remove_dir_all(&state);
    unsafe { std::env::set_var("ETUDE_STATE_DIR", &state) };
    let out = scan::scan(&root, &ScanConfig::default()).expect("scan source");
    let p = proposal_from_scan(
        &out,
        vec![plan::Group {
            name: "NewFolder".into(),
            signal: plan::Signal::Collected { count: 1 },
            members: vec![source],
            accepted: true,
        }],
    );
    let report = apply::apply(&p, &context(), Some(&TestSeal), None).expect("apply");
    let destination = root.join("NewFolder");
    let mut journal = Journal::load_sealed("test", &report.journal_id, &TestSeal).expect("journal");
    let mut replaced = false;
    let undo = apply::undo_with_progress(&mut journal, &TestSeal, |progress| {
        if progress.completed == progress.total && !replaced {
            // Reserve a different inode while the original directory still
            // exists. Removing and recreating the same path can immediately
            // reuse its inode on filesystems such as ext4, which would make
            // an inode-identity guard unable to distinguish the replacement.
            let replacement = root.join("reserved replacement");
            fs::create_dir(&replacement).expect("reserve replacement directory");
            let original = fs::symlink_metadata(&destination).expect("stat original directory");
            let reserved = fs::symlink_metadata(&replacement).expect("stat reserved directory");
            use std::os::unix::fs::MetadataExt;
            assert_ne!(
                (original.dev(), original.ino()),
                (reserved.dev(), reserved.ino()),
                "replacement must be a distinct directory object"
            );
            fs::remove_dir(&destination).expect("remove original empty destination");
            fs::rename(&replacement, &destination).expect("install reserved replacement");
            replaced = true;
        }
    });
    assert!(undo.error.is_none(), "undo error: {:?}", undo.error);
    assert!(replaced, "undo never reached the replacement point");
    assert!(
        destination.is_dir(),
        "undo removed the replacement directory"
    );
    cleanup(&root);
    let _ = fs::remove_dir_all(state);
}

#[test]
fn apply_refuses_a_destination_directory_created_after_planning() {
    let _g = lock();
    let root =
        std::env::temp_dir().join(format!("sweep_au_destination_race_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create root");
    let source = root.join("file.qoi");
    fs::write(&source, b"fixture").expect("write source");
    let state = std::env::temp_dir().join(format!(
        "sweep_state_destination_race_{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&state);
    unsafe { std::env::set_var("ETUDE_STATE_DIR", &state) };

    let out = scan::scan(&root, &ScanConfig::default()).expect("scan source");
    let p = Plan::with_groups(
        &out,
        vec![plan::Group {
            name: "NewFolder".into(),
            signal: plan::Signal::Collected { count: 1 },
            members: vec![source.clone()],
            accepted: true,
        }],
    );
    let destination = root.join("NewFolder");
    let mut injected = false;
    let result =
        apply::apply_with_structured_progress(&p, "test", Some(&TestSeal), None, |progress| {
            if progress.phase == ProgressPhase::Planning && progress.planned == 1 && !injected {
                // Another apply can win the mkdir after this plan has been
                // checked but before this apply reaches its first move.
                fs::create_dir(&destination).expect("competing apply created destination");
                injected = true;
            }
        });

    assert!(injected, "the competing mkdir was not injected");
    assert!(
        result.is_err(),
        "apply adopted a competing destination folder"
    );
    assert!(
        source.exists(),
        "apply moved a source into the competing folder"
    );
    assert!(destination.is_dir(), "apply removed the competing folder");
    assert_eq!(
        fs::read_dir(&destination)
            .expect("read competing folder")
            .count(),
        0,
        "the competing folder must remain untouched"
    );
    cleanup(&root);
    let _ = fs::remove_dir_all(state);
}

#[cfg(unix)]
#[test]
fn apply_refuses_a_destination_directory_replaced_after_planning() {
    use std::os::unix::fs::MetadataExt;

    let _g = lock();
    let root = std::env::temp_dir().join(format!(
        "sweep_au_destination_replaced_{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create root");
    let source = root.join("file.qoi");
    fs::write(&source, b"fixture").expect("write source");
    let destination = root.join("Reading");
    fs::create_dir(&destination).expect("create planned destination");
    let state = std::env::temp_dir().join(format!(
        "sweep_state_destination_replaced_{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&state);
    unsafe { std::env::set_var("ETUDE_STATE_DIR", &state) };

    let out = scan::scan(&root, &ScanConfig::default()).expect("scan source and destination");
    let p = Plan::with_groups(
        &out,
        vec![plan::Group {
            name: "Reading".into(),
            signal: plan::Signal::Mapped {
                ext: "qoi".into(),
                created: false,
            },
            members: vec![source.clone()],
            accepted: true,
        }],
    );
    let bound = BoundPlan::from_plan(p, context()).expect("bind fixture plan");
    let reserved = root.join("reserved replacement");
    let mut replaced = false;
    let result = apply::apply_bound_with_structured_progress(
        &bound,
        &context(),
        Some(&TestSeal),
        None,
        |progress| {
            if progress.phase == ProgressPhase::Planning && !replaced {
                // Reserve while Reading still exists so the filesystem cannot
                // recycle its inode, then install the replacement at the same
                // path after bound-plan validation but before any move.
                fs::create_dir(&reserved).expect("reserve replacement directory");
                let original = fs::symlink_metadata(&destination).expect("stat original");
                let replacement = fs::symlink_metadata(&reserved).expect("stat replacement");
                assert_ne!(
                    (original.dev(), original.ino()),
                    (replacement.dev(), replacement.ino())
                );
                fs::remove_dir(&destination).expect("remove original destination");
                fs::rename(&reserved, &destination).expect("install replacement");
                replaced = true;
            }
        },
    );

    assert!(replaced, "planning callback did not run");
    assert!(
        matches!(result, Err(ApplyError::DestinationDirectoryChanged(_))),
        "apply did not give the named refusal for a replacement destination: {result:?}"
    );
    assert!(source.exists(), "apply moved the source before refusing");
    assert_eq!(fs::read_dir(&destination).unwrap().count(), 0);
    cleanup(&root);
    let _ = fs::remove_dir_all(state);
}

#[cfg(unix)]
#[test]
fn apply_refuses_a_planned_destination_replaced_by_an_outside_symlink() {
    use std::os::unix::fs::symlink;

    let _g = lock();
    let root = std::env::temp_dir().join(format!(
        "sweep_au_destination_symlink_{}",
        std::process::id()
    ));
    let outside = root.with_file_name(format!(
        "sweep_au_destination_symlink_outside_{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&outside);
    fs::create_dir_all(&root).expect("create root");
    fs::create_dir(&outside).expect("create outside folder");
    let source = root.join("file.qoi");
    fs::write(&source, b"fixture").expect("write source");
    let destination = root.join("Reading");
    fs::create_dir(&destination).expect("create planned destination");
    let state = std::env::temp_dir().join(format!(
        "sweep_state_destination_symlink_{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&state);
    unsafe { std::env::set_var("ETUDE_STATE_DIR", &state) };

    let out = scan::scan(&root, &ScanConfig::default()).expect("scan source and destination");
    let p = Plan::with_groups(
        &out,
        vec![plan::Group {
            name: "Reading".into(),
            signal: plan::Signal::Mapped {
                ext: "qoi".into(),
                created: false,
            },
            members: vec![source.clone()],
            accepted: true,
        }],
    );
    let bound = BoundPlan::from_plan(p, context()).expect("bind fixture plan");
    let mut swapped = false;
    let result = apply::apply_bound_with_structured_progress(
        &bound,
        &context(),
        Some(&TestSeal),
        None,
        |progress| {
            if progress.phase == ProgressPhase::Planning && !swapped {
                // This is the source-review probe: validation has completed,
                // but the first move has not started.
                fs::remove_dir(&destination).expect("remove planned destination");
                symlink(&outside, &destination).expect("install outside symlink");
                swapped = true;
            }
        },
    );

    assert!(swapped, "planning callback did not run");
    assert!(
        matches!(result, Err(ApplyError::DestinationDirectoryChanged(_))),
        "apply did not give the named refusal for an outside symlink: {result:?}"
    );
    assert!(
        source.exists(),
        "apply moved the source through the symlink"
    );
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
    cleanup(&root);
    let _ = fs::remove_dir_all(outside);
    let _ = fs::remove_dir_all(state);
}

#[cfg(unix)]
#[test]
fn undo_stops_when_its_source_folder_is_replaced() {
    use std::os::unix::fs::MetadataExt;

    let _g = lock();
    let root =
        std::env::temp_dir().join(format!("sweep_au_undo_folder_swap_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create root");
    let sources: Vec<_> = (0..2)
        .map(|i| {
            let path = root.join(format!("file{i}.qoi"));
            fs::write(&path, b"fixture").expect("write source");
            path
        })
        .collect();
    let state = std::env::temp_dir().join(format!(
        "sweep_state_undo_folder_swap_{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&state);
    unsafe { std::env::set_var("ETUDE_STATE_DIR", &state) };
    let out = scan::scan(&root, &ScanConfig::default()).expect("scan sources");
    let p = proposal_from_scan(
        &out,
        vec![plan::Group {
            name: "Reading".into(),
            signal: plan::Signal::Collected {
                count: sources.len(),
            },
            members: sources.clone(),
            accepted: true,
        }],
    );
    let report = apply::apply(&p, &context(), Some(&TestSeal), None).expect("apply");
    let mut journal = Journal::load_sealed("test", &report.journal_id, &TestSeal).expect("journal");
    let displaced = root.with_file_name(format!("sweep_au_undo_displaced_{}", std::process::id()));
    let replacement = root.with_file_name(format!("sweep_au_undo_reserved_{}", std::process::id()));
    let mut swapped = false;
    let undo = apply::undo_with_progress(&mut journal, &TestSeal, |progress| {
        if progress.completed == 1 && !swapped {
            fs::create_dir(&replacement).expect("reserve replacement root");
            let original = fs::symlink_metadata(&root).expect("stat original root");
            let reserved = fs::symlink_metadata(&replacement).expect("stat replacement root");
            assert_ne!(
                (original.dev(), original.ino()),
                (reserved.dev(), reserved.ino())
            );
            fs::rename(&root, &displaced).expect("displace original root");
            fs::rename(&replacement, &root).expect("install replacement root");
            swapped = true;
        }
    });

    assert!(swapped, "undo did not reach the injected swap point");
    assert_eq!(
        undo.restored, 1,
        "undo moved files after the folder changed"
    );
    assert!(matches!(
        undo.error,
        Some(ApplyError::DestinationDirectoryChanged(_))
    ));
    assert!(displaced.join("file1.qoi").exists());
    assert!(displaced.join("Reading/file0.qoi").exists());
    assert!(
        !root.join("Reading").exists(),
        "replacement root was altered"
    );
    assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
    cleanup(&displaced);
    cleanup(&root);
    let _ = fs::remove_dir_all(state);
}

#[cfg(unix)]
#[test]
fn undo_recovery_refuses_a_replaced_source_folder() {
    use std::os::unix::fs::MetadataExt;

    let _g = lock();
    let root = std::env::temp_dir().join(format!(
        "sweep_au_undo_recovery_swap_{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create root");
    let source = root.join("file.qoi");
    fs::write(&source, b"fixture").expect("write source");
    let state = std::env::temp_dir().join(format!(
        "sweep_state_undo_recovery_swap_{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&state);
    unsafe { std::env::set_var("ETUDE_STATE_DIR", &state) };
    let out = scan::scan(&root, &ScanConfig::default()).expect("scan source");
    let p = proposal_from_scan(
        &out,
        vec![plan::Group {
            name: "Reading".into(),
            signal: plan::Signal::Collected { count: 1 },
            members: vec![source],
            accepted: true,
        }],
    );
    let report = apply::apply(&p, &context(), Some(&TestSeal), None).expect("apply");
    let mut journal = Journal::load_sealed("test", &report.journal_id, &TestSeal).expect("journal");
    // Simulate a process killed after the move but before the done frame.
    journal.entries[0].state = etude_core::journal::EntryState::Planned;
    journal
        .save_sealed(&TestSeal)
        .expect("write interrupted journal");

    let displaced = root.with_file_name(format!(
        "sweep_au_undo_recovery_displaced_{}",
        std::process::id()
    ));
    let replacement = root.with_file_name(format!(
        "sweep_au_undo_recovery_reserved_{}",
        std::process::id()
    ));
    fs::create_dir(&replacement).expect("reserve replacement root");
    let original_meta = fs::symlink_metadata(&root).expect("stat original root");
    let replacement_meta = fs::symlink_metadata(&replacement).expect("stat replacement root");
    assert_ne!(
        (original_meta.dev(), original_meta.ino()),
        (replacement_meta.dev(), replacement_meta.ino())
    );
    fs::rename(&root, &displaced).expect("displace original root");
    fs::rename(&replacement, &root).expect("install replacement root");

    let undo = apply::undo(&mut journal, &TestSeal);
    assert!(matches!(
        undo.error,
        Some(ApplyError::DestinationDirectoryChanged(_))
    ));
    assert!(displaced.join("Reading/file.qoi").exists());
    assert!(!root.join("file.qoi").exists());
    assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
    cleanup(&displaced);
    cleanup(&root);
    let _ = fs::remove_dir_all(state);
}

#[cfg(unix)]
#[test]
fn no_journal_apply_moves_a_source_that_cannot_be_opened_for_reading() {
    use std::os::unix::fs::PermissionsExt;

    let _g = lock();
    let root = std::env::temp_dir().join(format!("sweep_au_no_read_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create root");
    let source = root.join("selected.qoi");
    fs::write(&source, b"private fixture").expect("write source");
    fs::set_permissions(&source, fs::Permissions::from_mode(0o000)).expect("make unreadable");
    // Reading an extended attribute needs read permission, so the tag check
    // cannot run on this file. This test is about apply not opening the file,
    // so the scan does not consult tags; the held-by-default counterpart is
    // `an_item_whose_tag_cannot_be_read_is_held_by_default`.
    let out = scan::scan(
        &root,
        &ScanConfig {
            include_tagged: true,
            ..ScanConfig::default()
        },
    )
    .expect("scan source");
    let p = proposal_from_scan(
        &out,
        vec![plan::Group {
            name: "Collected".into(),
            signal: plan::Signal::Collected { count: 1 },
            members: vec![source.clone()],
            accepted: true,
        }],
    );
    let report = apply::apply(&p, &context(), None, None).expect("no-journal apply");
    assert_eq!(report.moved, 1);
    assert!(!source.exists(), "source was not moved");
    assert!(
        root.join("Collected/selected.qoi").exists(),
        "destination missing"
    );
    cleanup(&root);
}

#[test]
fn a_failure_mid_apply_leaves_a_journal_describing_exactly_what_happened() {
    let _g = lock();
    // Fault injection, not hope.
    let (root, _fx, p) = setup("resumable");
    const FAIL: usize = 7;

    let err = apply::apply(&p, &context(), Some(&TestSeal), Some(FAIL))
        .expect_err("injected failure should propagate");
    assert!(matches!(err, ApplyError::Injected(FAIL)));

    let j = Journal::latest_sealed("test", &TestSeal).expect("journal exists after a crash");
    let done: Vec<_> = j.entries.iter().filter(|e| e.is_moved()).collect();
    assert_eq!(
        done.len(),
        FAIL,
        "journal claims a different number of moves than happened"
    );

    for e in &done {
        assert!(e.to.exists(), "journal says done but destination is absent");
        assert!(
            !e.from.exists(),
            "journal says done but source is still there"
        );
    }
    for e in j.entries.iter().filter(|e| !e.is_moved()) {
        assert!(e.from.exists(), "journal says not-done but source is gone");
    }
    cleanup(&root);
}

#[test]
fn outcome_reports_the_durable_journal_id_after_apply_failure() {
    let _g = lock();
    let (root, _fx, p) = setup("outcome_failure");
    let failure = apply::apply_with_outcome(&p, &context(), Some(&TestSeal), Some(0))
        .expect_err("injected failure should propagate");
    assert!(matches!(failure.error, ApplyError::Injected(0)));
    let id = failure
        .durable_journal_id
        .expect("saved journal id should accompany the failure");
    let journal = Journal::latest_sealed("test", &TestSeal).expect("saved journal exists");
    assert_eq!(journal.id, id);
    assert!(journal.entries.iter().all(|entry| !entry.is_moved()));
    cleanup(&root);
}

#[test]
fn outcome_has_no_durable_journal_id_when_journaling_is_disabled() {
    let _g = lock();
    let (root, _fx, p) = setup("outcome_no_journal");
    let failure = apply::apply_with_outcome(&p, &context(), None, Some(0))
        .expect_err("injected failure should propagate");
    assert!(matches!(failure.error, ApplyError::Injected(0)));
    assert_eq!(failure.durable_journal_id, None);
    cleanup(&root);
}

#[test]
fn live_stale_outcome_reports_the_saved_empty_journal_id() {
    let _g = lock();
    let root = std::env::temp_dir().join(format!(
        "sweep_au_live_stale_outcome_{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    let state = std::env::temp_dir().join(format!(
        "sweep_state_live_stale_outcome_{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&state);
    fs::create_dir_all(&root).expect("create root");
    unsafe { std::env::set_var("ETUDE_STATE_DIR", &state) };
    for index in 0..4 {
        fs::write(
            root.join(format!("Screenshot 2026-01-01 at 10.0{index}.png")),
            b"synthetic payload",
        )
        .expect("write fixture source");
    }
    let out = scan::scan_for_live_apply(
        &root,
        &ScanConfig {
            grace: None,
            ..Default::default()
        },
    )
    .expect("live scan");
    let mut proposal = plan::build(&out);
    for group in &mut proposal.groups {
        group.accepted = true;
    }
    let bound = BoundPlan::from_live_plan(proposal, context()).expect("bind live plan");
    let source = bound.groups[0].members[0].clone();
    fs::write(root.join("Cargo.toml"), b"synthetic marker")
        .expect("invalidate live snapshot with a new marker");

    let failure = apply::apply_bound_with_structured_progress_outcome(
        &bound,
        &context(),
        Some(&TestSeal),
        None,
        |_| {},
    )
    .expect_err("stale live plan must refuse");
    assert!(matches!(failure.error, ApplyError::StalePlan(_)));
    let id = failure
        .durable_journal_id
        .expect("saved stale-plan journal id should accompany refusal");
    let journal = Journal::latest_sealed("test", &TestSeal).expect("empty journal exists");
    assert_eq!(journal.id, id);
    assert!(journal.entries.is_empty());
    assert!(source.exists(), "stale apply moved a source");
    cleanup(&root);
    let _ = fs::remove_dir_all(state);
}

#[test]
fn undo_after_a_partial_apply_restores_only_what_moved() {
    let _g = lock();
    let (root, _fx, p) = setup("partialundo");
    const FAIL: usize = 5;

    let _ = apply::apply(&p, &context(), Some(&TestSeal), Some(FAIL));
    let mut j = Journal::latest_sealed("test", &TestSeal).expect("journal");
    let r = apply::undo(&mut j, &TestSeal);

    assert!(r.error.is_none(), "unexpected undo error: {:?}", r.error);
    assert_eq!(
        r.restored, FAIL,
        "undo restored a different count than was applied"
    );
    for e in &j.entries {
        assert!(
            e.from.exists(),
            "file missing after partial undo: {}",
            e.from.display()
        );
    }
    cleanup(&root);
}

#[test]
fn undo_skips_a_file_whose_only_difference_is_its_contents() {
    let _g = lock();
    // The sibling test above edits a file freely, which changes size, mtime and
    // hash together -- so it passes whether undo skips on ANY difference or
    // only on ALL of them. This one changes exactly one field: same byte
    // count, same modification time, different bytes. Undo must still refuse.
    //
    // Mutation testing found the gap: weakening the skip condition from "any
    // field differs" to "every field differs" left the whole suite green,
    // which would let a restore overwrite edited work.
    let (root, _fx, p) = setup("hash-only");
    let rep = apply::apply(&p, &context(), Some(&TestSeal), None).expect("apply");
    let mut j = Journal::load_sealed("test", &rep.journal_id, &TestSeal).expect("journal");

    let victim = j.entries[0].to.clone();
    let before = fs::metadata(&victim).expect("stat");
    let original = fs::read(&victim).expect("read");
    // Same length, different content: only the hash moves.
    let mut edited = original.clone();
    let last = edited.len() - 1;
    edited[last] = edited[last].wrapping_add(1);
    fs::write(&victim, &edited).expect("edit");
    // Put the modification time back, so mtime matches the journal too.
    let f = fs::File::options().write(true).open(&victim).expect("open");
    f.set_times(fs::FileTimes::new().set_modified(before.modified().expect("mtime")))
        .expect("restore mtime");
    drop(f);

    let after = fs::metadata(&victim).expect("stat");
    assert_eq!(after.len(), before.len(), "fixture changed the size");

    let r = apply::undo(&mut j, &TestSeal);

    assert!(r.error.is_none(), "unexpected undo error: {:?}", r.error);
    assert!(
        r.skipped_changed.contains(&victim),
        "undo did not skip a file whose contents changed under an unchanged size and time"
    );
    assert_eq!(
        fs::read(&victim).expect("read"),
        edited,
        "undo overwrote edited content it should have skipped"
    );
    cleanup(&root);
}

#[test]
fn undo_refuses_to_overwrite_a_file_changed_since_apply() {
    let _g = lock();
    // Blind restoration would destroy newer work.
    let (root, _fx, p) = setup("mutated");
    let rep = apply::apply(&p, &context(), Some(&TestSeal), None).expect("apply");
    let mut j = Journal::load_sealed("test", &rep.journal_id, &TestSeal).expect("journal");

    let victim = j.entries[0].to.clone();
    fs::write(&victim, b"the user edited this after applying\n").expect("mutate");

    let r = apply::undo(&mut j, &TestSeal);

    assert!(r.error.is_none(), "unexpected undo error: {:?}", r.error);
    assert!(
        r.skipped_changed.contains(&victim),
        "undo did not report the changed file as skipped"
    );
    assert!(victim.exists(), "undo moved a file it should have skipped");
    let text = fs::read_to_string(&victim).expect("read");
    assert!(
        text.contains("the user edited"),
        "undo overwrote newer content"
    );
    cleanup(&root);
}

#[test]
fn no_journal_mode_writes_nothing_and_still_moves() {
    let _g = lock();
    let (root, _fx, p) = setup("nojournal");
    let expected: usize = p.groups.iter().map(|g| g.members.len()).sum();

    let rep = apply::apply(&p, &context(), None, None).expect("apply");

    assert_eq!(rep.moved, expected);
    assert!(
        rep.journal_path.is_none(),
        "no-journal mode reported a journal path"
    );
    assert!(
        Journal::latest_sealed("test", &TestSeal).is_err(),
        "no-journal mode still wrote a journal"
    );
    cleanup(&root);
}

#[test]
fn apply_refuses_when_a_destination_already_exists() {
    let _g = lock();
    let (root, _fx, p) = setup("collision");
    // Pre-create a colliding destination.
    let groups = p.groups.clone();
    let g = &groups[0];
    let dir = root.canonicalize().unwrap().join(&g.name);
    fs::create_dir_all(&dir).expect("mkdir");
    let name = g.members[0].file_name().unwrap();
    fs::write(dir.join(name), b"pre-existing\n").expect("write");
    let p = manual_proposal(&root, groups.clone(), false);

    let err = apply::apply(&p, &context(), Some(&TestSeal), None).expect_err("should refuse");
    assert!(matches!(err, ApplyError::DestinationExists(_)));

    // And nothing moved.
    for m in &g.members {
        assert!(
            m.exists(),
            "a file moved despite the refusal: {}",
            m.display()
        );
    }
    cleanup(&root);
}

#[test]
fn apply_refuses_when_two_planned_destinations_collide() {
    let _g = lock();
    let root = std::env::temp_dir().join(format!("sweep_au_plan_collision_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let sub1 = root.join("sub1");
    let sub2 = root.join("sub2");
    fs::create_dir_all(&sub1).expect("mkdir sub1");
    fs::create_dir_all(&sub2).expect("mkdir sub2");
    let src1 = sub1.join("Screenshot 1.png");
    let src2 = sub2.join("Screenshot 1.png");
    fs::write(&src1, b"first\n").expect("write first");
    fs::write(&src2, b"second\n").expect("write second");

    let state =
        std::env::temp_dir().join(format!("sweep_state_plan_collision_{}", std::process::id()));
    let _ = fs::remove_dir_all(&state);
    unsafe { std::env::set_var("ETUDE_STATE_DIR", &state) };

    let p = manual_proposal(
        &root,
        vec![plan::Group {
            name: "Screenshots".to_string(),
            signal: plan::Signal::Screenshot,
            members: vec![src1.clone(), src2.clone()],
            accepted: true,
        }],
        false,
    );

    let err = apply::apply(&p, &context(), Some(&TestSeal), None).expect_err("should refuse");
    assert!(matches!(err, ApplyError::DestinationCollision(_)));
    assert!(src1.exists(), "first source moved despite the refusal");
    assert!(src2.exists(), "second source moved despite the refusal");
    cleanup(&root);
}

/// Issue #9. Two sources whose visible names are the same string of
/// characters, but encoded as two different Unicode normalization forms:
/// "café" as NFC (single precomposed U+00E9) and as NFD (`e` followed by the
/// combining acute accent U+0301). Byte-distinct in Rust, but the same
/// directory entry on APFS. The ASCII-case sibling of this test
/// (`apply_refuses_when_two_planned_destinations_collide`) gets a clean
/// pre-flight `DestinationCollision` and zero files moved. This one must get
/// the same treatment, not a partial apply where the first file actually
/// moves and the second fails against the filesystem mid-run.
#[test]
// APFS treats NFC and NFD spellings as one directory entry; ext4 does not,
// so on Linux these are genuinely two files and the refusal correctly does not
// happen. Gated rather than deleted: the property is real where the filesystem
// is.
#[cfg(target_os = "macos")]
fn apply_refuses_when_two_planned_destinations_are_nfc_nfd_of_same_name() {
    let _g = lock();
    let root =
        std::env::temp_dir().join(format!("sweep_au_nfc_nfd_collision_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let sub1 = root.join("sub1");
    let sub2 = root.join("sub2");
    fs::create_dir_all(&sub1).expect("mkdir sub1");
    fs::create_dir_all(&sub2).expect("mkdir sub2");

    // NFC: "caf" + U+00E9 (LATIN SMALL LETTER E WITH ACUTE), one code point.
    let nfc_name = "invoice_caf\u{00e9}.pdf";
    // NFD: "cafe" + U+0301 (COMBINING ACUTE ACCENT), two code points.
    let nfd_name = "invoice_cafe\u{0301}.pdf";
    assert_ne!(
        nfc_name.as_bytes(),
        nfd_name.as_bytes(),
        "fixture broken: the two forms must be byte-distinct going in"
    );

    let src1 = sub1.join(nfc_name);
    let src2 = sub2.join(nfd_name);
    fs::write(&src1, b"first\n").expect("write first");
    fs::write(&src2, b"second\n").expect("write second");

    let state = std::env::temp_dir().join(format!(
        "sweep_state_nfc_nfd_collision_{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&state);
    unsafe { std::env::set_var("ETUDE_STATE_DIR", &state) };

    let p = manual_proposal(
        &root,
        vec![plan::Group {
            name: "invoice".to_string(),
            signal: plan::Signal::Screenshot,
            members: vec![src1.clone(), src2.clone()],
            accepted: true,
        }],
        false,
    );

    let err = apply::apply(&p, &context(), Some(&TestSeal), None).expect_err("should refuse");
    assert!(
        matches!(err, ApplyError::DestinationCollision(_)),
        "expected a clean pre-flight DestinationCollision, got: {err:?}"
    );
    assert!(
        src1.exists(),
        "NFC source moved despite the refusal (partial apply)"
    );
    assert!(
        src2.exists(),
        "NFD source moved despite the refusal (partial apply)"
    );
    cleanup(&root);
}

#[test]
fn two_applies_same_root_same_second_get_distinct_journal_ids() {
    let _g = lock();
    // Wall-clock seconds alone collide; both journals must survive.
    let root = std::env::temp_dir().join(format!("sweep_au_jid_collision_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let sub_a = root.join("a");
    let sub_b = root.join("b");
    fs::create_dir_all(&sub_a).expect("mkdir a");
    fs::create_dir_all(&sub_b).expect("mkdir b");
    let src_a = sub_a.join("alpha.txt");
    let src_b = sub_b.join("beta.txt");
    fs::write(&src_a, b"alpha\n").expect("write a");
    fs::write(&src_b, b"beta\n").expect("write b");
    let src_a = src_a.canonicalize().expect("canonical a");
    let src_b = src_b.canonicalize().expect("canonical b");

    let state =
        std::env::temp_dir().join(format!("sweep_state_jid_collision_{}", std::process::id()));
    let _ = fs::remove_dir_all(&state);
    unsafe { std::env::set_var("ETUDE_STATE_DIR", &state) };

    let plan_a = manual_proposal(
        &root,
        vec![plan::Group {
            name: "GroupA".to_string(),
            signal: plan::Signal::Screenshot,
            members: vec![src_a.clone()],
            accepted: true,
        }],
        false,
    );
    let rep_a = apply::apply(&plan_a, &context(), Some(&TestSeal), None).expect("apply a");
    let plan_b = manual_proposal(
        &root,
        vec![plan::Group {
            name: "GroupB".to_string(),
            signal: plan::Signal::Screenshot,
            members: vec![src_b.clone()],
            accepted: true,
        }],
        false,
    );

    let rep_b = apply::apply(&plan_b, &context(), Some(&TestSeal), None).expect("apply b");

    assert_ne!(
        rep_a.journal_id, rep_b.journal_id,
        "same-root same-tool applies collided on journal_id"
    );
    assert_ne!(
        rep_a.journal_path, rep_b.journal_path,
        "same-root same-tool applies shared a journal path"
    );

    let j_a = Journal::load_sealed("test", &rep_a.journal_id, &TestSeal).expect("load a");
    let j_b = Journal::load_sealed("test", &rep_b.journal_id, &TestSeal).expect("load b");
    assert_eq!(j_a.entries.len(), 1);
    assert_eq!(j_b.entries.len(), 1);
    assert_eq!(j_a.entries[0].from, src_a);
    assert_eq!(j_b.entries[0].from, src_b);
    assert!(j_a.entries[0].is_moved());
    assert!(j_b.entries[0].is_moved());

    cleanup(&root);
}

#[test]
fn no_filename_is_readable_in_the_written_journal() {
    // The M7 claim, checked against the bytes on disk rather than the API.
    let _g = lock();
    let (root, _fx, p) = setup("sealed");
    let rep = apply::apply(&p, &context(), Some(&TestSeal), None).expect("apply");
    let raw = fs::read(rep.journal_path.expect("path")).expect("read");
    let hay = String::from_utf8_lossy(&raw);

    for name in fixtures::all_names() {
        assert!(
            !hay.contains(&name),
            "journal leaked a filename in the clear: {name}"
        );
    }
    assert!(
        !hay.contains("/Users"),
        "journal leaked a path prefix in the clear"
    );
    cleanup(&root);
}

// --- Defect #2: --allow-sync must reach apply, not just plan ---------------
//
// `apply()`'s destination-sync guard used to check `is_synced(&dest_dir)`
// unconditionally, never consulting whether the flag that let `scan()`
// proceed into a synced root was ever granted. Since a group's destination
// is always `plan.root.join(&g.name)`, that meant: the moment a root needed
// `--allow-sync` to be scanned, applying to it could never succeed, with or
// without the flag. `Plan::allow_sync` (set at plan-build time, derived from
// `scan.root_is_synced`, which is itself only true when the scan was granted
// the flag) is what closes that gap.

#[test]
fn apply_refuses_synced_destination_when_allow_sync_was_never_granted() {
    let _g = lock();
    let root = std::env::temp_dir().join(format!("sweep_au_sync_denied_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("mkdir root");
    let src = root.join("deck_notes.pdf");
    fs::write(&src, b"x\n").expect("write src");

    let state =
        std::env::temp_dir().join(format!("sweep_state_sync_denied_{}", std::process::id()));
    let _ = fs::remove_dir_all(&state);
    unsafe { std::env::set_var("ETUDE_STATE_DIR", &state) };

    // The destination directory NAME is itself a sync marker ("Dropbox"), so
    // is_synced(dest) is true even though this plan was never granted
    // allow_sync. This is today's default-refusal behaviour and it must not
    // change: no flag, no unlock.
    let p = manual_proposal(
        &root,
        vec![plan::Group {
            name: "Dropbox".to_string(),
            signal: plan::Signal::Screenshot,
            members: vec![src.clone()],
            accepted: true,
        }],
        false,
    );

    let err = apply::apply(&p, &context(), Some(&TestSeal), None).expect_err("should refuse");
    assert!(matches!(err, ApplyError::DestinationIsSynced(_)));
    assert!(src.exists(), "source moved despite the refusal");
    cleanup(&root);
}

#[test]
fn allow_sync_granted_at_scan_time_actually_reaches_apply() {
    let _g = lock();
    // Real repro shape: the root itself lives inside a synced tree
    // (.../Dropbox/Projects), scanned with --allow-sync. This is exactly what
    // stress/scenarios/20-allow-sync-apply-illusion.sh does against the CLI.
    let base = std::env::temp_dir().join(format!("sweep_au_sync_ok_{}", std::process::id()));
    let root = base.join("Dropbox").join("Projects");
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&root).expect("mkdir root");
    let src = root.join("deck_notes.pdf");
    fs::write(&src, b"x\n").expect("write src");

    let state = std::env::temp_dir().join(format!("sweep_state_sync_ok_{}", std::process::id()));
    let _ = fs::remove_dir_all(&state);
    unsafe { std::env::set_var("ETUDE_STATE_DIR", &state) };

    let out = scan::scan(
        &root,
        &ScanConfig {
            allow_sync: true,
            ..Default::default()
        },
    )
    .expect("scan with allow_sync");
    assert!(out.root_is_synced, "fixture root should be flagged synced");

    let entry_path = out.entries[0].path.clone();
    let entry_name = out.entries[0].name.clone();
    let p = proposal_from_scan(
        &out,
        vec![plan::Group {
            name: "Docs".to_string(),
            signal: plan::Signal::Screenshot,
            members: vec![entry_path],
            accepted: true,
        }],
    );

    let rep = apply::apply(&p, &context(), Some(&TestSeal), None)
        .expect("allow_sync granted at scan time should let apply proceed");
    assert_eq!(rep.moved, 1);
    assert!(out.root.join("Docs").join(&entry_name).exists());

    let _ = fs::remove_dir_all(&base);
}

#[test]
fn allow_sync_granted_on_an_unsynced_root_still_covers_a_destination_that_looks_synced() {
    let _g = lock();
    // `Plan::allow_sync` must carry the FLAG that was passed to scan(), not
    // something derived from whether the root itself turned out to be
    // synced. If it were derived from root_is_synced, this scenario would
    // wrongly refuse: the root here is an ordinary, non-synced folder, but
    // `sweep review` lets a user rename a group to any name they choose
    // (see review.rs's "rename escape hatch"), and nothing stops that name
    // from colliding with a sync marker like "Dropbox". A user who already
    // passed --allow-sync should not be refused later over a coincidence in
    // a destination folder NAME.
    let root = std::env::temp_dir().join(format!(
        "sweep_au_sync_flag_survives_rename_{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("mkdir root");
    let src = root.join("deck_notes.pdf");
    fs::write(&src, b"x\n").expect("write src");

    let state = std::env::temp_dir().join(format!(
        "sweep_state_sync_flag_survives_rename_{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&state);
    unsafe { std::env::set_var("ETUDE_STATE_DIR", &state) };

    // --allow-sync passed even though this root is not actually synced.
    let out = scan::scan(
        &root,
        &ScanConfig {
            allow_sync: true,
            ..Default::default()
        },
    )
    .expect("scan");
    assert!(!out.root_is_synced, "fixture root is not synced");
    assert!(out.allow_sync, "the flag itself must still be recorded");

    // The user renamed this group to "Dropbox". That is coincidence, not
    // intent to touch real Dropbox. The destination now matches a sync marker.
    let p = proposal_from_scan(
        &out,
        vec![plan::Group {
            name: "Dropbox".to_string(),
            signal: plan::Signal::Screenshot,
            members: vec![src.clone()],
            accepted: true,
        }],
    );

    let rep = apply::apply(&p, &context(), Some(&TestSeal), None)
        .expect("the --allow-sync flag granted at scan time must still cover this");
    assert_eq!(rep.moved, 1);
    assert!(root.join("Dropbox").join("deck_notes.pdf").exists());

    cleanup(&root);
}

#[test]
fn undo_collapses_a_half_move_instead_of_leaving_a_duplicate() {
    // Issue #5's crash shape, which on macOS can now only arise from the
    // link+unlink FALLBACK path (renamex_np unsupported) or a journal written
    // before the atomic move landed: two syscalls, a signal between them, the
    // file at BOTH paths with the same inode. Undo used to walk away from it.
    //
    // Same inode at both paths is not ambiguity. It is proof they are one
    // file, and that the unlink half never ran. Removing the destination link
    // finishes the reversal rather than abandoning it.
    let _g = lock();
    let root = std::env::temp_dir().join(format!("sweep_halfmove_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let state = root.join("state");
    fs::create_dir_all(&state).expect("mkdir state");
    unsafe { std::env::set_var("ETUDE_STATE_DIR", &state) };
    let group = root.join("Screenshots");
    fs::create_dir_all(&group).expect("mkdir");
    let from = root.join("Screenshot 2026-01-01 at 9.00.00 AM.png");
    fs::write(&from, b"one file, two names").expect("write");

    let to = group.join("Screenshot 2026-01-01 at 9.00.00 AM.png");
    fs::hard_link(&from, &to).expect("link");
    // Deliberately do NOT unlink `from`: this is the crash window.

    // Fingerprint it exactly the way apply would have, so undo sees a journal
    // that matches reality rather than one hand-built to pass.
    let (size, mtime, inode, edge) = etude_core::journal::fingerprint(&to).expect("fingerprint");

    let mut j = Journal {
        id: "halfmove".into(),
        tool: "test".into(),
        root: root.clone(),
        entries: vec![Entry {
            from: from.clone(),
            to: to.clone(),
            method: Method::Rename,
            size,
            mtime_secs: mtime,
            inode,
            edge_hash: edge,
            // The crash shape. `done` is set only AFTER move_one returns, so a
            // signal between the link and the unlink leaves it false. A review
            // caught the first version of this test setting it true, which is
            // the user-hard-linked-something case and not this bug at all.
            state: etude_core::journal::EntryState::Planned,

            tag: None,
        }],
        created_directories: Vec::new(),
        validated_directories: vec![directory_guard(&root)],
        progress_tail_damaged: false,
    };

    let r = apply::undo(&mut j, &TestSeal);

    assert!(
        !to.exists(),
        "the duplicate at the destination survived undo: one file is still \
         reachable by two names and nothing tracks it"
    );
    assert!(from.exists(), "the file must remain at its origin");
    assert_eq!(
        r.healed.len(),
        1,
        "a collapsed half-move is healed, not restored: nothing moved, an extra \
         name was removed"
    );
    assert_eq!(r.restored, 0, "nothing was moved back");

    cleanup(&root);
}

#[test]
fn a_users_own_hard_link_is_never_deleted_by_recovery() {
    // The case that blocked the first version of this fix. A user hard-links a
    // file into a planned destination themselves; same (dev, ino) at both
    // names is then true for a pair sweep never touched. Recovery is scoped to
    // the successor of the last done entry, so an entry beyond it must never
    // have inference applied — and this asserts exactly that.
    let _g = lock();
    let root = std::env::temp_dir().join(format!("sweep_userlink_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let state = root.join("state");
    fs::create_dir_all(&state).expect("mkdir state");
    unsafe { std::env::set_var("ETUDE_STATE_DIR", &state) };
    let group = root.join("Screenshots");
    fs::create_dir_all(&group).expect("mkdir");

    // Entry 0: never started (not done, nothing moved).
    let a_from = root.join("a.png");
    fs::write(&a_from, b"entry zero").expect("write");

    // Entry 1: ALSO not done, and the user has hard-linked from->to themselves.
    let b_from = root.join("b.png");
    let b_to = group.join("b.png");
    fs::write(&b_from, b"the user's own link").expect("write");
    fs::hard_link(&b_from, &b_to).expect("user's link");

    let fp = |p: &std::path::Path| etude_core::journal::fingerprint(p).expect("fp");
    let (s0, m0, i0, h0) = fp(&a_from);
    let (s1, m1, i1, h1) = fp(&b_from);

    let mut j = Journal {
        id: "userlink".into(),
        tool: "test".into(),
        root: root.clone(),
        entries: vec![
            Entry {
                from: a_from.clone(),
                to: group.join("a.png"),
                method: Method::Rename,
                size: s0,
                mtime_secs: m0,
                inode: i0,
                edge_hash: h0,
                state: etude_core::journal::EntryState::Planned,

                tag: None,
            },
            Entry {
                from: b_from.clone(),
                to: b_to.clone(),
                method: Method::Rename,
                size: s1,
                mtime_secs: m1,
                inode: i1,
                edge_hash: h1,
                state: etude_core::journal::EntryState::Planned,

                tag: None,
            },
        ],
        created_directories: Vec::new(),
        validated_directories: Vec::new(),
        progress_tail_damaged: false,
    };

    let r = apply::undo(&mut j, &TestSeal);

    // Entry 1 is past the successor (entry 0), so recovery must not look at
    // it. The user's link survives.
    assert!(
        b_to.exists(),
        "recovery deleted a hard link the user made themselves: the exact \
         failure a review blocked the first version of this fix for"
    );
    assert!(b_from.exists());
    assert!(r.healed.is_empty(), "nothing here was sweep's to heal");

    let _ = fs::remove_dir_all(&root);
    unsafe { std::env::remove_var("ETUDE_STATE_DIR") };
}

#[test]
fn a_plain_rename_never_replaces_this() {
    // The comment on rename_excl warns that a future tidy-up replacing it with
    // fs::rename would silently clobber on collision. This is the test that
    // comment promises: move_one must REFUSE an existing destination, never
    // overwrite it. fs::rename overwrites, so swapping it in turns this red.
    let _g = lock();
    let root = std::env::temp_dir().join(format!("sweep_noclobber_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("mkdir");

    let from = root.join("mover.txt");
    let to = root.join("occupied.txt");
    fs::write(&from, b"wants to move").expect("write");
    fs::write(&to, b"already living here").expect("write");

    let err = apply::move_one_for_tests(&from, &to)
        .expect_err("moving onto an existing file must be refused");
    assert_eq!(err.raw_os_error(), Some(17), "EEXIST, not a silent replace");
    assert_eq!(
        fs::read(&to).expect("read"),
        b"already living here",
        "the occupant was replaced"
    );
    assert!(
        from.exists(),
        "the source must be untouched by a refused move"
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_killed_undo_resumes_where_it_stopped_instead_of_starting_over() {
    // Issue #7. Undo used to hold every reversal in memory and write the
    // journal once at the end, so a kill partway through left the file on
    // disk claiming all entries were still at their destinations. The next
    // run walked all of them again, reported the same restored count again,
    // and the journal never converged to "nothing left to undo".
    let _g = lock();
    let root = std::env::temp_dir().join(format!("sweep_resume_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let state = root.join("state");
    fs::create_dir_all(&state).expect("mkdir state");
    unsafe { std::env::set_var("ETUDE_STATE_DIR", &state) };

    let sealer = TestSeal;
    let group = root.join("Screenshots");
    fs::create_dir_all(&group).expect("mkdir group");

    // Build the post-apply state directly, the way the other tests here do:
    // files at their destinations, entries fingerprinted from what is really
    // on disk rather than hand-written to pass.
    let mut entries = Vec::new();
    for n in 0..4 {
        let from = root.join(format!("f{n}.png"));
        let to = group.join(format!("f{n}.png"));
        fs::write(&to, format!("file {n}")).expect("write");
        let (size, mtime, inode, edge) =
            etude_core::journal::fingerprint(&to).expect("fingerprint");
        entries.push(Entry {
            from,
            to,
            method: Method::Rename,
            size,
            mtime_secs: mtime,
            inode,
            edge_hash: edge,
            state: etude_core::journal::EntryState::Moved,

            tag: None,
        });
    }
    let mut j = Journal {
        id: "resume".into(),
        tool: "test".into(),
        root: root.clone(),
        entries,
        created_directories: Vec::new(),
        validated_directories: vec![directory_guard(&root)],
        progress_tail_damaged: false,
    };
    j.save_sealed(&sealer).expect("save");

    // Undo the top two, persisting as it goes, then stop as a kill would.
    for i in (2..4).rev() {
        let e = j.entries[i].clone();
        fs::rename(&e.to, &e.from).expect("manual reverse");
        j.entries[i].state = etude_core::journal::EntryState::Reversed;
        j.record_undone(i, &sealer).expect("record");
    }

    // Reload from disk. This is the whole point: what the killed run left
    // behind has to say two are already home.
    let reloaded = Journal::load_sealed("test", &j.id, &sealer).expect("reload");
    assert_eq!(
        reloaded.entries.iter().filter(|e| e.is_moved()).count(),
        2,
        "the journal on disk still claims four files are at their destinations, \
         so a resumed undo will walk the two that are already home"
    );

    // Resume. It must restore exactly the two that are left.
    let mut j2 = reloaded;
    let r = apply::undo(&mut j2, &sealer);
    assert!(r.error.is_none(), "resumed undo errored: {:?}", r.error);
    assert_eq!(r.restored, 2, "a resumed undo must only do the work left");
    assert_eq!(
        r.already_reversed, 2,
        "and must recognise the finished work"
    );

    // Converged: everything home, and a third run finds nothing.
    for n in 0..4 {
        assert!(root.join(format!("f{n}.png")).exists(), "f{n} is not home");
    }
    let mut j3 = Journal::load_sealed("test", &j2.id, &sealer).expect("reload again");
    let r3 = apply::undo(&mut j3, &sealer);
    assert_eq!(r3.restored, 0, "a third run must find nothing left to do");
    assert_eq!(r3.already_reversed, 4);

    let _ = fs::remove_dir_all(&root);
    unsafe { std::env::remove_var("ETUDE_STATE_DIR") };
}

#[test]
fn a_reversal_survives_being_saved_and_reloaded() {
    // A review caught Reversed encoding as 0 in the base frame and reloading
    // as Planned. That put an entry undo had already finished back in reach of
    // apply's crash recovery, which is the exact collapse the three states
    // exist to prevent: a user who hard-linked the old destination back would
    // have had that name deleted.
    let _g = lock();
    let root = std::env::temp_dir().join(format!("sweep_roundtrip_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let state = root.join("state");
    fs::create_dir_all(&state).expect("mkdir state");
    unsafe { std::env::set_var("ETUDE_STATE_DIR", &state) };
    let sealer = TestSeal;

    let from = root.join("a.png");
    let to = root.join("Screenshots").join("a.png");
    fs::create_dir_all(to.parent().expect("parent")).expect("mkdir");
    fs::write(&from, b"home already").expect("write");
    let (size, mtime, inode, edge) = etude_core::journal::fingerprint(&from).expect("fp");

    let j = Journal {
        id: "roundtrip".into(),
        tool: "test".into(),
        root: root.clone(),
        entries: vec![Entry {
            from: from.clone(),
            to: to.clone(),
            method: Method::Rename,
            size,
            mtime_secs: mtime,
            inode,
            edge_hash: edge,
            state: etude_core::journal::EntryState::Reversed,

            tag: None,
        }],
        created_directories: Vec::new(),
        validated_directories: Vec::new(),
        progress_tail_damaged: false,
    };
    j.save_sealed(&sealer).expect("save");

    let back = Journal::load_sealed("test", &j.id, &sealer).expect("reload");
    assert_eq!(
        back.entries[0].state,
        etude_core::journal::EntryState::Reversed,
        "a reversal came back as something else, so undo's finished work is \
         indistinguishable from work apply never started"
    );

    // And the consequence that makes it matter: recovery must not touch it.
    let mut j2 = back;
    fs::hard_link(&from, &to).expect("user links the old destination back");
    let r = apply::undo(&mut j2, &sealer);
    assert!(
        to.exists(),
        "recovery deleted a name the user made, because a reloaded reversal \
         looked like an entry apply never got to"
    );
    assert!(r.healed.is_empty());

    let _ = fs::remove_dir_all(&root);
    unsafe { std::env::remove_var("ETUDE_STATE_DIR") };
}

#[test]
fn a_skip_between_two_reversals_still_reloads() {
    // A review caught the replay cursor demanding consecutive indices. Undo
    // walks backwards but writes no frame for an entry it skips as changed or
    // missing, so a real run produces holes: reverse 2, skip 1, reverse 0.
    // The strict version rejected that journal as damaged, which would have
    // broken resume in exactly the runs where the frames matter.
    let _g = lock();
    let root = std::env::temp_dir().join(format!("sweep_holes_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let state = root.join("state");
    fs::create_dir_all(&state).expect("mkdir state");
    unsafe { std::env::set_var("ETUDE_STATE_DIR", &state) };
    let sealer = TestSeal;
    let group = root.join("Screenshots");
    fs::create_dir_all(&group).expect("mkdir");

    let mut entries = Vec::new();
    for n in 0..3 {
        let to = group.join(format!("f{n}.png"));
        fs::write(&to, format!("file {n}")).expect("write");
        let (size, mtime, inode, edge) =
            etude_core::journal::fingerprint(&to).expect("fingerprint");
        entries.push(Entry {
            from: root.join(format!("f{n}.png")),
            to,
            method: Method::Rename,
            size,
            mtime_secs: mtime,
            inode,
            edge_hash: edge,
            state: etude_core::journal::EntryState::Moved,

            tag: None,
        });
    }
    let mut j = Journal {
        id: "holes".into(),
        tool: "test".into(),
        root: root.clone(),
        entries,
        created_directories: Vec::new(),
        validated_directories: vec![directory_guard(&root)],
        progress_tail_damaged: false,
    };
    j.save_sealed(&sealer).expect("save");

    // Reverse 2 and 0, leaving 1 alone the way a skip would.
    for i in [2usize, 0usize] {
        let e = j.entries[i].clone();
        fs::rename(&e.to, &e.from).expect("reverse");
        j.entries[i].state = etude_core::journal::EntryState::Reversed;
        j.record_undone(i, &sealer).expect("record");
    }

    let back = Journal::load_sealed("test", &j.id, &sealer)
        .expect("a journal with a skip between two reversals must still load");
    assert_eq!(
        back.entries[2].state,
        etude_core::journal::EntryState::Reversed
    );
    assert_eq!(
        back.entries[1].state,
        etude_core::journal::EntryState::Moved
    );
    assert_eq!(
        back.entries[0].state,
        etude_core::journal::EntryState::Reversed
    );

    let _ = fs::remove_dir_all(&root);
    unsafe { std::env::remove_var("ETUDE_STATE_DIR") };
}

#[test]
fn selective_undo_records_holes_and_preserves_unselected_entries() {
    let _g = lock();
    let root = std::env::temp_dir().join(format!("selective_undo_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let state = root.join("state");
    let destination = root.join("holding");
    fs::create_dir_all(&state).expect("mkdir state");
    fs::create_dir_all(&destination).expect("mkdir destination");
    unsafe { std::env::set_var("ETUDE_STATE_DIR", &state) };
    let sealer = TestSeal;
    let mut entries = Vec::new();
    for index in 0..3 {
        let from = root.join(format!("source-{index}"));
        let to = destination.join(format!("item-{index}"));
        fs::write(&to, format!("synthetic item {index}")).expect("write held item");
        let (size, mtime_secs, inode, edge_hash) =
            etude_core::journal::fingerprint(&to).expect("fingerprint");
        entries.push(Entry {
            from,
            to,
            method: Method::Rename,
            size,
            mtime_secs,
            inode,
            edge_hash,
            state: etude_core::journal::EntryState::Moved,

            tag: None,
        });
    }
    let mut journal = Journal {
        id: "selective".into(),
        tool: "test".into(),
        root: root.clone(),
        entries,
        created_directories: Vec::new(),
        validated_directories: vec![directory_guard(&root)],
        progress_tail_damaged: false,
    };
    journal.save_sealed(&sealer).expect("save base journal");

    // Restore entries 2 and 0, leaving entry 1 held. This is the same sparse
    // progress shape as the skip-between-reversals regression below.
    for index in [2usize, 0] {
        let report = apply::undo_entry(&mut journal, index, &sealer).expect("valid index");
        assert_eq!(report.restored, 1);
    }
    assert!(journal.entries[0].from.exists());
    assert!(!journal.entries[1].from.exists());
    assert!(journal.entries[1].to.exists());
    assert!(journal.entries[2].from.exists());

    let reloaded = Journal::load_sealed("test", &journal.id, &sealer)
        .expect("selective reversal frames must reload");
    use etude_core::journal::EntryState::{Moved, Reversed};
    assert_eq!(
        reloaded
            .entries
            .iter()
            .map(|entry| entry.state)
            .collect::<Vec<_>>(),
        vec![Reversed, Moved, Reversed]
    );
    let _ = fs::remove_dir_all(&root);
    unsafe { std::env::remove_var("ETUDE_STATE_DIR") };
}

/// A journal whose last progress frame was cut mid-write still restores every
/// file, including the one whose frame never landed.
///
/// This is the end-to-end half of `a_torn_tail_keeps_the_complete_frames_and_says_it_was_torn`
/// in `journal.rs`, which can assert what the journal says but has no
/// filesystem to check it against.
///
/// The failure it pins: CI produced a journal with 143 complete frames and a
/// 4-byte tail -- a length prefix whose body the crash beat. Refusing the whole
/// journal over those 4 bytes left every file the 143 frames described sitting
/// at its destination, and `undo` reported only that it could not open the
/// journal. Replaying the complete frames and stopping at the tear recovers
/// them; the entry whose frame was cut is the first `Planned` one, which is
/// exactly what successor-entry recovery is for.
#[test]
fn a_torn_final_frame_still_lets_undo_restore_everything() {
    let _g = lock();
    let (root, _fx, p) = setup("torn_tail");

    let rep = apply::apply(&p, &context(), Some(&TestSeal), None).expect("apply");
    let moved: Vec<PathBuf> = Journal::load_sealed("test", &rep.journal_id, &TestSeal)
        .expect("journal loads")
        .entries
        .iter()
        .map(|e| e.to.clone())
        .collect();
    assert!(
        moved.len() > 2,
        "test setup: need several moves for a tear to matter, got {}",
        moved.len()
    );
    for to in &moved {
        assert!(
            to.exists(),
            "test setup: {} should be at its destination",
            to.display()
        );
    }

    // Cut the file the way the crash did: append a length prefix announcing a
    // record whose body never arrives. Four bytes, exactly what CI showed.
    let jpath = Journal::load_sealed("test", &rep.journal_id, &TestSeal)
        .expect("journal loads")
        .path();
    let mut raw = fs::read(&jpath).expect("read journal");
    raw.extend_from_slice(&4144u32.to_le_bytes());
    fs::write(&jpath, &raw).expect("write torn journal");

    let mut j = Journal::load_sealed("test", &rep.journal_id, &TestSeal)
        .expect("a torn tail must not void the journal");
    assert!(
        j.progress_tail_damaged,
        "the tear has to be visible on the journal, or the loss is silent"
    );

    let report = apply::undo(&mut j, &TestSeal);
    assert!(report.error.is_none(), "undo errored: {:?}", report.error);

    for to in &moved {
        assert!(
            !to.exists(),
            "{} was still at its destination after undo. A 4-byte tail stranded it",
            to.display()
        );
    }
    for e in &j.entries {
        assert!(
            e.from.exists(),
            "{} never came home after undo",
            e.from.display()
        );
    }

    cleanup(&root);
}

/// A journal missing more than one record does nothing, rather than restoring
/// what it can reach and calling that success.
///
/// The distinction this pins: ONE entry that looks moved-but-unrecorded is a
/// crash between a move and its record, and successor-entry recovery handles
/// it. SEVERAL means the journal itself lost records, and recovery only ever
/// reaches the first -- so proceeding restores a few, strands the rest, and
/// exits 0.
///
/// That half-restore is what an earlier version of the torn-tail fix did:
/// measured at 3 restored and 17 stranded, silently. Refusing is worse at
/// recovering and better at not lying, and the files are all still at their
/// destinations either way.
#[test]
fn a_journal_missing_several_records_restores_nothing() {
    let _g = lock();
    let (root, _fx, p) = setup("deep_trunc");

    let rep = apply::apply(&p, &context(), Some(&TestSeal), None).expect("apply");
    let full = Journal::load_sealed("test", &rep.journal_id, &TestSeal).expect("loads");
    assert!(
        full.entries.len() > 3,
        "test setup: need several moves, got {}",
        full.entries.len()
    );

    // Cut back to the base frame plus nothing: every entry reads Planned while
    // every file is at its destination. A truncation on a frame boundary, so
    // the tail does not even register as torn.
    let jpath = full.path();
    let raw = fs::read(&jpath).expect("read");
    let base_len = u32::from_le_bytes(raw[0..4].try_into().unwrap()) as usize;
    fs::write(&jpath, &raw[..4 + base_len]).expect("truncate");

    let mut j = Journal::load_sealed("test", &rep.journal_id, &TestSeal).expect("still loads");
    let before: Vec<PathBuf> = j.entries.iter().map(|e| e.to.clone()).collect();

    assert_eq!(
        apply::next_undo_journal(std::slice::from_ref(&j), &TestSeal),
        Some(0)
    );
    let report = apply::undo(&mut j, &TestSeal);

    assert!(
        report.unrecorded_moves > 1,
        "should have noticed several unrecorded moves, saw {}",
        report.unrecorded_moves
    );
    assert_eq!(
        report.restored, 0,
        "restored {} files from a journal it could not fully reverse. Partial is \
         the failure here: the rest are stranded and the exit code says success",
        report.restored
    );
    for to in &before {
        assert!(
            to.exists(),
            "{} was moved despite the refusal. Nothing should have been touched",
            to.display()
        );
    }

    cleanup(&root);
}

/// Files sharing a word are separated by what they are, not collected by the
/// word they mention.
///
/// The fixture's five acme_* files span five extensions -- psd, fig, pdf, md,
/// sketch -- and share nothing else. A shared-token rule grouped all five and
/// named the folder "acme"; on the first real Downloads folder that rule met
/// it made a folder called "apple" out of a receipt, an agreement, a script
/// and an export. Frequency is not category.
///
/// Two things are pinned. No group may be NAMED from a word in the filenames,
/// and the five acme files may never all land in one group again -- they are
/// five different kinds of file and belong wherever their kind belongs, which
/// for most of them is nowhere, since one .psd does not make a group.
#[test]
fn files_sharing_a_word_are_split_by_what_they_are() {
    let _g = lock();
    let (root, _fx, p) = setup("no_token");

    for g in &p.groups {
        assert!(
            !g.name.eq_ignore_ascii_case("acme") && !g.name.eq_ignore_ascii_case("notes"),
            "a group named {:?} exists. Something is naming groups from words \
             in filenames again",
            g.name
        );
    }

    let acme_in = |g: &etude_core::plan::Group| {
        g.members
            .iter()
            .filter(|m| m.to_string_lossy().to_lowercase().contains("acme_"))
            .count()
    };
    let total: usize = p.groups.iter().map(acme_in).sum();
    let biggest = p.groups.iter().map(acme_in).max().unwrap_or(0);
    assert!(
        biggest < 5,
        "all five acme_* files landed in one group again ({biggest} of them). \
         They are five different kinds of file; only a rule reading the word \
         they share could put them together"
    );
    assert!(
        total < 5 || biggest < total,
        "the acme files were collected rather than separated"
    );

    cleanup(&root);
}

#[test]
fn unbound_apply_refuses_destinations_outside_its_root_before_moving() {
    let _guard = lock();
    let base =
        std::env::temp_dir().join(format!("etude_destination_bounds_{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    let root = base.join("root");
    let outside = base.join("outside");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir(&outside).unwrap();
    let source = root.join("file.txt");
    fs::write(&source, b"original").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, root.join("alias")).unwrap();
    let out = scan::scan(&root, &ScanConfig::default()).unwrap();
    let mut names = vec![
        "../escaped".to_string(),
        "nested/../escaped".into(),
        outside.to_string_lossy().into_owned(),
        "".into(),
        "nested//child".into(),
        "nested/".into(),
    ];
    #[cfg(unix)]
    names.push("alias/child".into());
    for name in names {
        let p = Plan::with_groups(
            &out,
            vec![plan::Group {
                name: name.clone(),
                signal: plan::Signal::Collected { count: 1 },
                members: vec![source.clone()],
                accepted: true,
            }],
        );
        let result = apply::apply_with_structured_progress(&p, "test", None, None, |_| {});
        let error = result.expect_err("unsafe destination must be refused");
        assert!(
            matches!(error, ApplyError::StalePlan(_)),
            "wrong refusal for {name:?}: {error}"
        );
        assert!(
            error
                .to_string()
                .contains("destination must stay inside the plan root")
        );
        assert_eq!(
            fs::read(&source).unwrap(),
            b"original",
            "source moved for {name:?}"
        );
        assert!(
            !base.join("escaped").exists(),
            "created escaping destination"
        );
        assert_eq!(
            fs::read_dir(&outside).unwrap().count(),
            0,
            "changed outside root"
        );
    }
    fs::remove_dir_all(base).unwrap();
}

/// Build the stopped A / completed B ending without processes or timing.
fn contended_journals(tag: &str, count: usize) -> (PathBuf, Vec<Journal>) {
    let root = std::env::temp_dir().join(format!("sweep_contention_{tag}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("group")).unwrap();
    unsafe { std::env::set_var("ETUDE_STATE_DIR", root.join("state")) };
    let mut a = Journal {
        id: "a".into(),
        tool: "test".into(),
        root: root.clone(),
        validated_directories: vec![directory_guard(&root)],
        ..Journal::default()
    };
    for i in 0..count {
        let from = root.join(format!("file-{i}"));
        let to = root.join("group").join(format!("file-{i}"));
        fs::write(&from, format!("original bytes {i}\n")).unwrap();
        let (size, mtime_secs, inode, edge_hash) = etude_core::journal::fingerprint(&from).unwrap();
        a.entries.push(Entry {
            from: from.clone(),
            to: to.clone(),
            method: Method::Rename,
            size,
            mtime_secs,
            inode,
            edge_hash,
            state: etude_core::journal::EntryState::Planned,

            tag: None,
        });
        fs::rename(from, to).unwrap();
    }
    a.entries[0].state = etude_core::journal::EntryState::Moved;
    let mut b = Journal {
        id: "b".into(),
        tool: "test".into(),
        root: root.clone(),
        entries: a.entries[1..].to_vec(),
        validated_directories: vec![directory_guard(&root)],
        ..Journal::default()
    };
    for entry in &mut b.entries {
        entry.state = etude_core::journal::EntryState::Moved;
    }
    b.save_sealed(&TestSeal).unwrap();
    a.save_sealed(&TestSeal).unwrap();
    // Explicit order stands for recency, independent of timestamp resolution.
    (root, vec![a, b])
}

#[test]
fn newest_stopped_apply_does_not_block_the_journal_that_finished_its_files() {
    let _g = lock();
    let (root, mut journals) = contended_journals("recovery", 5);
    assert_eq!(apply::unrecorded_moves(&journals[0]), 4);
    for _ in 0..2 {
        let index = apply::next_undo_journal(&journals, &TestSeal).unwrap();
        let report = apply::undo(&mut journals[index], &TestSeal);
        assert_eq!(
            report.unrecorded_moves, 0,
            "contention was mistaken for lost records"
        );
        assert!(report.error.is_none(), "{:?}", report.error);
        assert!(report.skipped_changed.is_empty());
        journals[index] = Journal::load_sealed("test", &journals[index].id, &TestSeal).unwrap();
    }
    assert!(apply::next_undo_journal(&journals, &TestSeal).is_none());
    for i in 0..5 {
        assert_eq!(
            fs::read(root.join(format!("file-{i}"))).unwrap(),
            format!("original bytes {i}\n").as_bytes()
        );
    }
    assert_eq!(fs::read_dir(root.join("group")).unwrap().count(), 0);
    cleanup(&root);
}

#[test]
fn a_single_contended_destination_is_restored_by_its_recorded_owner() {
    let _g = lock();
    for copied in [false, true] {
        let (root, mut journals) = contended_journals(if copied { "copy" } else { "single" }, 2);
        if copied {
            // Model the inode change from copy-unlink without needing a second
            // mounted volume. The original observation remains in both journals.
            let entry = &mut journals[1].entries[0];
            let metadata = fs::metadata(&entry.to).unwrap();
            let temporary = root.join("copied");
            fs::copy(&entry.to, &temporary).unwrap();
            fs::File::options()
                .write(true)
                .open(&temporary)
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(metadata.modified().unwrap()))
                .unwrap();
            fs::remove_file(&entry.to).unwrap();
            fs::rename(temporary, &entry.to).unwrap();
            entry.method = Method::CopyUnlink;
            assert_ne!(
                etude_core::journal::fingerprint(&entry.to).unwrap().2,
                entry.inode
            );
            journals[1].save_sealed(&TestSeal).unwrap();
        }
        assert_eq!(apply::next_undo_journal(&journals, &TestSeal), Some(1));
        let report = apply::undo(&mut journals[1], &TestSeal);
        assert_eq!(report.restored, 1);
        assert_eq!(apply::next_undo_journal(&journals, &TestSeal), Some(0));
        assert_eq!(apply::undo(&mut journals[0], &TestSeal).restored, 1);
        for i in 0..2 {
            assert_eq!(
                fs::read(root.join(format!("file-{i}"))).unwrap(),
                format!("original bytes {i}\n").as_bytes()
            );
        }
        cleanup(&root);
    }
}

#[test]
fn unjournalled_moves_and_unproven_owners_still_refuse() {
    let _g = lock();
    let (root, mut journals) = contended_journals("unproven", 5);
    let before = fs::read(journals[0].path()).unwrap();
    // The files exist, but the process that moved them left no journal.
    assert_eq!(apply::next_undo_journal(&journals[..1], &TestSeal), Some(0));
    assert_eq!(apply::undo(&mut journals[0], &TestSeal).unrecorded_moves, 4);
    // A journal that merely planned the same paths proves no completed move.
    for entry in &mut journals[1].entries {
        entry.state = etude_core::journal::EntryState::Planned;
    }
    assert_eq!(apply::next_undo_journal(&journals, &TestSeal), Some(0));
    // A Moved record with the wrong fingerprint is also not an owner.
    for entry in &mut journals[1].entries {
        entry.state = etude_core::journal::EntryState::Moved;
        entry.edge_hash ^= 1;
    }
    assert_eq!(apply::next_undo_journal(&journals, &TestSeal), Some(0));
    assert_eq!(apply::undo(&mut journals[0], &TestSeal).unrecorded_moves, 4);
    assert_eq!(fs::read(journals[0].path()).unwrap(), before);
    for i in 0..5 {
        assert!(!root.join(format!("file-{i}")).exists());
        assert_eq!(
            fs::read(root.join("group").join(format!("file-{i}"))).unwrap(),
            format!("original bytes {i}\n").as_bytes()
        );
    }
    cleanup(&root);
}

#[cfg(unix)]
#[test]
fn another_live_journal_keeps_its_origin_directory() {
    let _g = lock();
    let (root, folder, state, mut journal) = journal_with_every_file_reversed("live_origin");
    let mut other = Journal {
        id: "live-origin".into(),
        tool: "stash".into(),
        root: root.clone(),
        validated_directories: vec![directory_guard(&folder)],
        ..Default::default()
    };
    let file = folder.join("held.txt");
    fs::write(&file, b"held bytes").unwrap();
    let held = root.join("holding.txt");
    let (size, mtime_secs, inode, edge_hash) = etude_core::journal::fingerprint(&file).unwrap();
    fs::rename(&file, &held).unwrap();
    other.entries.push(Entry {
        from: file.clone(),
        to: held,
        method: Method::Rename,
        size,
        mtime_secs,
        inode,
        edge_hash,
        state: etude_core::journal::EntryState::Moved,
        tag: None,
    });
    other.save_sealed(&TestSeal).unwrap();
    let report = apply::undo(&mut journal, &TestSeal);
    assert!(report.error.is_none());
    assert!(folder.is_dir(), "live origin folder must remain");
    assert!(!apply::has_undo_work(&journal, &TestSeal));
    let restored = apply::undo(&mut other, &TestSeal);
    assert_eq!(restored.restored, 1);
    assert_eq!(fs::read(file).unwrap(), b"held bytes");
    cleanup(&root);
    let _ = fs::remove_dir_all(state);
}
