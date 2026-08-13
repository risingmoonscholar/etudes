//! M5/M6 acceptance tests.
//!
//! These prove the reversibility and resumability claims. Each runs against an
//! isolated synthetic tree with its own state directory, so journals never mix
//! and no real user file is involved.

use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

use etude_core::apply::{self, ApplyError};
use etude_core::journal::{Entry, Journal, Method};
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
    let ur = apply::undo(&mut j);

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
    let r = apply::undo(&mut j);

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
fn undo_refuses_to_overwrite_a_file_changed_since_apply() {
    let _g = lock();
    // Blind restoration would destroy newer work.
    let (root, _fx, p) = setup("mutated");
    let rep = apply::apply(&p, "test", Some(&TestSeal), None).expect("apply");
    let mut j = Journal::load_sealed("test", &rep.journal_id, &TestSeal).expect("journal");

    let victim = j.entries[0].to.clone();
    fs::write(&victim, b"the user edited this after applying\n").expect("mutate");

    let r = apply::undo(&mut j);

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
        skipped_system: 0,
        skipped_unreadable: 0,
        root_is_synced: false,
        allow_sync: false,
    };

    let err = apply::apply(&p, "test", Some(&TestSeal), None).expect_err("should refuse");
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

    let p = Plan {
        root: root.clone(),
        groups: vec![plan::Group {
            name: "invoice".to_string(),
            signal: plan::Signal::Screenshot,
            members: vec![src1.clone(), src2.clone()],
            accepted: true,
        }],
        untouched: Vec::new(),
        scanned: 2,
        skipped_hidden: 0,
        skipped_symlink: 0,
        skipped_system: 0,
        skipped_unreadable: 0,
        root_is_synced: false,
        allow_sync: false,
    };

    let err = apply::apply(&p, "test", Some(&TestSeal), None).expect_err("should refuse");
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

    let state =
        std::env::temp_dir().join(format!("sweep_state_jid_collision_{}", std::process::id()));
    let _ = fs::remove_dir_all(&state);
    unsafe { std::env::set_var("ETUDE_STATE_DIR", &state) };

    let plan_a = Plan {
        root: root.clone(),
        groups: vec![plan::Group {
            name: "GroupA".to_string(),
            signal: plan::Signal::Screenshot,
            members: vec![src_a.clone()],
            accepted: true,
        }],
        untouched: Vec::new(),
        scanned: 1,
        skipped_hidden: 0,
        skipped_symlink: 0,
        skipped_system: 0,
        skipped_unreadable: 0,
        root_is_synced: false,
        allow_sync: false,
    };
    let plan_b = Plan {
        root: root.clone(),
        groups: vec![plan::Group {
            name: "GroupB".to_string(),
            signal: plan::Signal::Screenshot,
            members: vec![src_b.clone()],
            accepted: true,
        }],
        untouched: Vec::new(),
        scanned: 1,
        skipped_hidden: 0,
        skipped_symlink: 0,
        skipped_system: 0,
        skipped_unreadable: 0,
        root_is_synced: false,
        allow_sync: false,
    };

    let rep_a = apply::apply(&plan_a, "test", Some(&TestSeal), None).expect("apply a");
    let rep_b = apply::apply(&plan_b, "test", Some(&TestSeal), None).expect("apply b");

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
    assert!(j_a.entries[0].done);
    assert!(j_b.entries[0].done);

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
    let p = Plan {
        root: root.clone(),
        groups: vec![plan::Group {
            name: "Dropbox".to_string(),
            signal: plan::Signal::Screenshot,
            members: vec![src.clone()],
            accepted: true,
        }],
        untouched: Vec::new(),
        scanned: 1,
        skipped_hidden: 0,
        skipped_symlink: 0,
        skipped_system: 0,
        skipped_unreadable: 0,
        root_is_synced: false,
        allow_sync: false,
    };

    let err = apply::apply(&p, "test", Some(&TestSeal), None).expect_err("should refuse");
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
    let p = Plan {
        root: out.root.clone(),
        groups: vec![plan::Group {
            name: "Docs".to_string(),
            signal: plan::Signal::Screenshot,
            members: vec![entry_path],
            accepted: true,
        }],
        untouched: Vec::new(),
        scanned: 1,
        skipped_hidden: 0,
        skipped_symlink: 0,
        skipped_system: 0,
        skipped_unreadable: 0,
        root_is_synced: out.root_is_synced,
        allow_sync: out.allow_sync,
    };

    let rep = apply::apply(&p, "test", Some(&TestSeal), None)
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
    let p = Plan {
        root: out.root.clone(),
        groups: vec![plan::Group {
            name: "Dropbox".to_string(),
            signal: plan::Signal::Screenshot,
            members: vec![src.clone()],
            accepted: true,
        }],
        untouched: Vec::new(),
        scanned: 1,
        skipped_hidden: 0,
        skipped_symlink: 0,
        skipped_system: 0,
        skipped_unreadable: 0,
        root_is_synced: out.root_is_synced,
        allow_sync: out.allow_sync,
    };

    let rep = apply::apply(&p, "test", Some(&TestSeal), None)
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
            done: false,
        }],
    };

    let r = apply::undo(&mut j);

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
                done: false,
            },
            Entry {
                from: b_from.clone(),
                to: b_to.clone(),
                method: Method::Rename,
                size: s1,
                mtime_secs: m1,
                inode: i1,
                edge_hash: h1,
                done: false,
            },
        ],
    };

    let r = apply::undo(&mut j);

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
