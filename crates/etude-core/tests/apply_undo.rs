//! M5/M6 acceptance tests.
//!
//! These prove the reversibility and resumability claims. Each runs against an
//! isolated synthetic tree with its own state directory, so journals never mix
//! and no real user file is involved.

use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

use etude_core::apply::{self, ApplyError};
use etude_core::journal::Journal;
use etude_core::plan::{self, Plan};
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

/// Test sealer. Not a cipher — it proves the *plumbing* is sealed-only. The
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

fn setup(tag: &str) -> (PathBuf, fixtures::Fixture, Plan) {
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
    (root, fx, p)
}

fn cleanup(root: &PathBuf) {
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(root.parent().unwrap().join("sweep_fixture_outside"));
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

    apply::apply(&p, "test", Some(&TestSeal), None).expect("apply");

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

    let rep = apply::apply(&p, "test", Some(&TestSeal), None).expect("apply");
    assert_eq!(rep.moved, expected.len());
    for src in &expected {
        assert!(
            !src.exists(),
            "source still present after apply: {}",
            src.display()
        );
    }

    let mut j = Journal::load_sealed("test", &rep.journal_id, &TestSeal).expect("journal loads");
    let ur = apply::undo(&mut j).expect("undo");

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
fn a_failure_mid_apply_leaves_a_journal_describing_exactly_what_happened() {
    let _g = lock();
    // Fault injection, not hope.
    let (root, _fx, p) = setup("resumable");
    const FAIL: usize = 7;

    let err = apply::apply(&p, "test", Some(&TestSeal), Some(FAIL))
        .expect_err("injected failure should propagate");
    assert!(matches!(err, ApplyError::Injected(FAIL)));

    let j = Journal::latest_sealed("test", &TestSeal).expect("journal exists after a crash");
    let done: Vec<_> = j.entries.iter().filter(|e| e.done).collect();
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
    for e in j.entries.iter().filter(|e| !e.done) {
        assert!(e.from.exists(), "journal says not-done but source is gone");
    }
    cleanup(&root);
}

#[test]
fn undo_after_a_partial_apply_restores_only_what_moved() {
    let _g = lock();
    let (root, _fx, p) = setup("partialundo");
    const FAIL: usize = 5;

    let _ = apply::apply(&p, "test", Some(&TestSeal), Some(FAIL));
    let mut j = Journal::latest_sealed("test", &TestSeal).expect("journal");
    let r = apply::undo(&mut j).expect("undo");

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
fn undo_refuses_to_overwrite_a_file_changed_since_apply() {
    let _g = lock();
    // Blind restoration would destroy newer work.
    let (root, _fx, p) = setup("mutated");
    let rep = apply::apply(&p, "test", Some(&TestSeal), None).expect("apply");
    let mut j = Journal::load_sealed("test", &rep.journal_id, &TestSeal).expect("journal");

    let victim = j.entries[0].to.clone();
    fs::write(&victim, b"the user edited this after applying\n").expect("mutate");

    let r = apply::undo(&mut j).expect("undo");

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

    let rep = apply::apply(&p, "test", None, None).expect("apply");

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
    let g = &p.groups[0];
    let dir = root.canonicalize().unwrap().join(&g.name);
    fs::create_dir_all(&dir).expect("mkdir");
    let name = g.members[0].file_name().unwrap();
    fs::write(dir.join(name), b"pre-existing\n").expect("write");

    let err = apply::apply(&p, "test", Some(&TestSeal), None).expect_err("should refuse");
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

    let p = Plan {
        root: root.clone(),
        groups: vec![plan::Group {
            name: "Screenshots".to_string(),
            signal: plan::Signal::Screenshot,
            members: vec![src1.clone(), src2.clone()],
            accepted: true,
        }],
        untouched: Vec::new(),
        scanned: 2,
        skipped_hidden: 0,
        skipped_symlink: 0,
        root_is_synced: false,
    };

    let err = apply::apply(&p, "test", Some(&TestSeal), None).expect_err("should refuse");
    assert!(matches!(err, ApplyError::DestinationCollision(_)));
    assert!(src1.exists(), "first source moved despite the refusal");
    assert!(src2.exists(), "second source moved despite the refusal");
    cleanup(&root);
}

#[test]
fn no_filename_is_readable_in_the_written_journal() {
    // The M7 claim, checked against the bytes on disk rather than the API.
    let _g = lock();
    let (root, _fx, p) = setup("sealed");
    let rep = apply::apply(&p, "test", Some(&TestSeal), None).expect("apply");
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
