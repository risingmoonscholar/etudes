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
fn a_failure_mid_apply_leaves_a_journal_describing_exactly_what_happened() {
    let _g = lock();
    // Fault injection, not hope.
    let (root, _fx, p) = setup("resumable");
    const FAIL: usize = 7;

    let err = apply::apply(&p, "test", Some(&TestSeal), Some(FAIL))
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
fn undo_after_a_partial_apply_restores_only_what_moved() {
    let _g = lock();
    let (root, _fx, p) = setup("partialundo");
    const FAIL: usize = 5;

    let _ = apply::apply(&p, "test", Some(&TestSeal), Some(FAIL));
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
fn undo_refuses_to_overwrite_a_file_changed_since_apply() {
    let _g = lock();
    // Blind restoration would destroy newer work.
    let (root, _fx, p) = setup("mutated");
    let rep = apply::apply(&p, "test", Some(&TestSeal), None).expect("apply");
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
    assert!(j_a.entries[0].is_moved());
    assert!(j_b.entries[0].is_moved());

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
            state: etude_core::journal::EntryState::Planned,
        }],
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
            },
        ],
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
        });
    }
    let mut j = Journal {
        id: "resume".into(),
        tool: "test".into(),
        root: root.clone(),
        entries,
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
        }],
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
        });
    }
    let mut j = Journal {
        id: "holes".into(),
        tool: "test".into(),
        root: root.clone(),
        entries,
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

    let rep = apply::apply(&p, "test", Some(&TestSeal), None).expect("apply");
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

    let rep = apply::apply(&p, "test", Some(&TestSeal), None).expect("apply");
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
