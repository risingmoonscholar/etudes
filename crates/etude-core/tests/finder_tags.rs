//! Finder tags: a tagged item is left alone, apply re-checks at move time and
//! after the rename, and the journal records what happened to each item.
//!
//! Tags are the extended attribute com.apple.metadata:_kMDItemUserTags. These
//! tests set it directly; the value is an opaque byte string, because nothing
//! in the tools may decode it.
#![cfg(target_os = "macos")]

use std::ffi::CString;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use etude_core::apply::{self, ApplyFaults};
use etude_core::journal::{EntryState, Journal, Sealer};
use etude_core::plan::{Group, Plan, Signal};
use etude_core::scan::{self, ScanConfig};

unsafe extern "C" {
    fn setxattr(
        path: *const std::ffi::c_char,
        name: *const std::ffi::c_char,
        value: *const std::ffi::c_void,
        size: usize,
        position: u32,
        options: i32,
    ) -> i32;
    fn getxattr(
        path: *const std::ffi::c_char,
        name: *const std::ffi::c_char,
        value: *mut std::ffi::c_void,
        size: usize,
        position: u32,
        options: i32,
    ) -> isize;
}

const TAG: &std::ffi::CStr = c"com.apple.metadata:_kMDItemUserTags";
const COMMENT: &std::ffi::CStr = c"com.apple.metadata:kMDItemFinderComment";

fn set_attr(path: &Path, name: &std::ffi::CStr, value: &[u8]) {
    let path = CString::new(path.as_os_str().as_bytes()).unwrap();
    // SAFETY: valid C strings and a slice that outlives the call.
    let rc = unsafe {
        setxattr(
            path.as_ptr(),
            name.as_ptr(),
            value.as_ptr().cast(),
            value.len(),
            0,
            0,
        )
    };
    assert_eq!(rc, 0, "could not set a test attribute");
}

fn tag(path: &Path) {
    set_attr(path, TAG, b"opaque-tag-bytes");
}

fn read_tag(path: &Path) -> Option<Vec<u8>> {
    let c = CString::new(path.as_os_str().as_bytes()).unwrap();
    let mut buffer = vec![0u8; 256];
    // SAFETY: the buffer owns `len` writable bytes.
    let n = unsafe {
        getxattr(
            c.as_ptr(),
            TAG.as_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            0,
            0,
        )
    };
    (n >= 0).then(|| buffer[..n as usize].to_vec())
}

fn lock() -> MutexGuard<'static, ()> {
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

struct TestSeal;
impl Sealer for TestSeal {
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

/// Fails to seal exactly the progress frames that start with `prefix`.
struct FailingSeal {
    prefix: &'static [u8],
}
impl Sealer for FailingSeal {
    fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>, &'static str> {
        if plaintext.starts_with(self.prefix) {
            return Err("injected journal write failure");
        }
        TestSeal.seal(plaintext)
    }
    fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, &'static str> {
        TestSeal.open(sealed)
    }
}

struct Scene {
    root: PathBuf,
    files: Vec<PathBuf>,
}

impl Drop for Scene {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn scene(label: &str, names: &[&str]) -> Scene {
    let base = std::env::temp_dir().join(format!("etude_tags_{label}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    let root = base.join("root");
    fs::create_dir_all(&root).unwrap();
    let state = base.join("state");
    unsafe { std::env::set_var("ETUDE_STATE_DIR", &state) };
    let root = root.canonicalize().unwrap();
    let files = names
        .iter()
        .map(|name| {
            let path = root.join(name);
            fs::write(&path, format!("contents of {name}")).unwrap();
            path
        })
        .collect();
    Scene { root, files }
}

fn scan_config(include_tagged: bool) -> ScanConfig {
    ScanConfig {
        grace: None,
        include_tagged,
        ..ScanConfig::default()
    }
}

fn plan_of(scene: &Scene, include_tagged: bool) -> Plan {
    let out = scan::scan(&scene.root, &scan_config(include_tagged)).expect("scan");
    let members: Vec<PathBuf> = out.entries.iter().map(|e| e.path.clone()).collect();
    Plan::with_groups(
        &out,
        vec![Group {
            name: "Collected".into(),
            signal: Signal::Collected {
                count: members.len(),
            },
            members,
            accepted: true,
        }],
    )
}

fn reload(report: &apply::ApplyReport) -> Journal {
    Journal::load_sealed("test", &report.journal_id, &TestSeal).expect("journal reloads")
}

fn state_of(journal: &Journal, name: &str) -> EntryState {
    journal
        .entries
        .iter()
        .find(|e| e.from.file_name().is_some_and(|n| n == name))
        .unwrap_or_else(|| panic!("journal has no entry for {name}"))
        .state
}

#[test]
fn default_scan_holds_tagged_items_and_counts_them_without_names() {
    let _g = lock();
    let s = scene("scan", &["a.txt", "b.txt", "c.txt"]);
    tag(&s.files[1]);
    let held = scan::scan(&s.root, &scan_config(false)).unwrap();
    assert_eq!(held.skipped_tagged, 1);
    let names: Vec<_> = held.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["a.txt", "c.txt"]);
    assert!(!held.include_tagged);

    let included = scan::scan(&s.root, &scan_config(true)).unwrap();
    assert_eq!(included.skipped_tagged, 0);
    assert_eq!(included.entries.len(), 3);
    assert!(included.include_tagged);
}

#[test]
fn an_item_tagged_after_the_scan_is_held_at_move_time_and_counted() {
    let _g = lock();
    let s = scene("late", &["a.txt", "b.txt", "c.txt"]);
    let plan = plan_of(&s, false);
    // Between plan and apply.
    tag(&s.files[1]);
    let report =
        apply::apply_with_structured_progress(&plan, "test", Some(&TestSeal), None, |_| {})
            .expect("apply");
    assert_eq!(report.moved, 2);
    assert_eq!(report.held_tagged, 1);
    assert_eq!(report.held_cross_volume, 0);
    assert!(s.files[1].exists(), "the tagged item stayed");
    assert!(read_tag(&s.files[1]).is_some(), "and kept its tag");
    assert!(!s.files[0].exists() && !s.files[2].exists());
    assert!(s.root.join("Collected/a.txt").exists());

    let journal = reload(&report);
    assert_eq!(state_of(&journal, "a.txt"), EntryState::Moved);
    assert_eq!(state_of(&journal, "b.txt"), EntryState::Held);
    assert_eq!(state_of(&journal, "c.txt"), EntryState::Moved);

    // Undo puts back what moved and has nothing to say about the held item.
    let mut journal = journal;
    let undone = apply::undo(&mut journal, &TestSeal);
    assert_eq!(undone.restored, 2);
    assert!(undone.error.is_none());
    assert!(s.files.iter().all(|f| f.exists()));
}

#[test]
fn an_item_tagged_after_the_scan_moves_when_the_plan_included_tagged_items() {
    let _g = lock();
    let s = scene("late_included", &["a.txt", "b.txt"]);
    let plan = plan_of(&s, true);
    tag(&s.files[1]);
    let report =
        apply::apply_with_structured_progress(&plan, "test", Some(&TestSeal), None, |_| {})
            .expect("apply");
    assert_eq!((report.moved, report.held_tagged), (2, 0));
    // The tag travelled with the rename.
    assert_eq!(
        read_tag(&s.root.join("Collected/b.txt")).as_deref(),
        Some(&b"opaque-tag-bytes"[..])
    );
}

#[test]
fn when_every_item_is_held_nothing_moves_and_no_folder_is_left_behind() {
    let _g = lock();
    let s = scene("all_held", &["a.txt", "b.txt"]);
    let plan = plan_of(&s, false);
    tag(&s.files[0]);
    tag(&s.files[1]);
    let report =
        apply::apply_with_structured_progress(&plan, "test", Some(&TestSeal), None, |_| {})
            .expect("apply");
    assert_eq!((report.moved, report.held_tagged), (0, 2));
    assert!(!s.root.join("Collected").exists(), "no empty group folder");
    let journal = reload(&report);
    assert!(journal.entries.iter().all(|e| e.state == EntryState::Held));
}

#[test]
fn an_item_tagged_between_the_check_and_the_rename_is_returned_and_counted() {
    let _g = lock();
    let s = scene("returned", &["a.txt", "b.txt", "c.txt"]);
    let plan = plan_of(&s, false);
    let target = s.files[1].clone();
    let mut tag_it = |_: usize, from: &Path| {
        if from == target {
            tag(from);
        }
    };
    let report = apply::apply_with_faults_for_tests(
        &plan,
        "test",
        Some(&TestSeal),
        ApplyFaults {
            before_rename: Some(&mut tag_it),
            after_rename: None,
            ..Default::default()
        },
    )
    .expect("apply");
    assert_eq!(report.moved, 2, "the others moved");
    assert_eq!(report.held_tagged, 1);
    assert_eq!(report.tagged_not_returned, 0);
    assert!(s.files[1].exists(), "the returned item is at its origin");
    assert!(!s.root.join("Collected/b.txt").exists());
    assert!(
        read_tag(&s.files[1]).is_some(),
        "its tag survived both renames"
    );
    assert!(s.root.join("Collected/c.txt").exists(), "apply carried on");

    let journal = reload(&report);
    assert_eq!(state_of(&journal, "b.txt"), EntryState::Returned);
    assert_eq!(state_of(&journal, "a.txt"), EntryState::Moved);
    assert_eq!(state_of(&journal, "c.txt"), EntryState::Moved);

    let mut journal = journal;
    let undone = apply::undo(&mut journal, &TestSeal);
    assert_eq!(undone.restored, 2);
    assert!(undone.skipped_changed.is_empty() && undone.skipped_missing.is_empty());
    assert!(undone.error.is_none());
}

#[test]
fn an_item_that_cannot_be_returned_stays_counted_and_journaled() {
    let _g = lock();
    let s = scene("stranded", &["a.txt", "b.txt"]);
    let plan = plan_of(&s, false);
    let first = s.files[0].clone();
    let mut tag_it = |_: usize, from: &Path| {
        if from == first {
            tag(from);
        }
    };
    // After the rename, something takes the origin before the return.
    let occupant = first.clone();
    let mut occupy = |_: usize, to: &Path| {
        if to.file_name().is_some_and(|n| n == "a.txt") {
            fs::write(&occupant, b"someone else's file").unwrap();
        }
    };
    let report = apply::apply_with_faults_for_tests(
        &plan,
        "test",
        Some(&TestSeal),
        ApplyFaults {
            before_rename: Some(&mut tag_it),
            after_rename: Some(&mut occupy),
            ..Default::default()
        },
    )
    .expect("apply returns a report, not an error");
    assert_eq!(report.tagged_not_returned, 1);
    assert_eq!(report.held_tagged, 0, "it was not held, it is stranded");
    assert_eq!(report.hold_record_failed, 0);
    assert_eq!(report.moved, 2, "both items are at their destinations");
    assert_eq!(
        fs::read(&first).unwrap(),
        b"someone else's file",
        "the occupant was never overwritten"
    );
    let stranded = s.root.join("Collected/a.txt");
    assert_eq!(fs::read(&stranded).unwrap(), b"contents of a.txt");
    assert!(read_tag(&stranded).is_some());

    let journal = reload(&report);
    assert_eq!(state_of(&journal, "a.txt"), EntryState::Stranded);
    assert!(
        journal
            .entries
            .iter()
            .any(|e| e.is_moved() && e.to == stranded)
    );
    assert_eq!(
        etude_core::journal::Journal::summary(&journal).moved,
        2,
        "undo still sees it as moved"
    );

    // Undo reports the stranded item as unable to go home and does not
    // overwrite the occupant.
    let mut journal = journal;
    let undone = apply::undo(&mut journal, &TestSeal);
    assert_eq!(undone.restored, 1);
    assert_eq!(undone.skipped_changed, vec![stranded.clone()]);
    assert_eq!(fs::read(&first).unwrap(), b"someone else's file");
    assert!(stranded.exists());
}

#[test]
fn a_failed_journal_write_during_the_return_leaves_the_item_home_and_reported_held() {
    let _g = lock();
    let s = scene("return_unrecorded", &["a.txt", "b.txt"]);
    let plan = plan_of(&s, false);
    let first = s.files[0].clone();
    let mut tag_it = |_: usize, from: &Path| {
        if from == first {
            tag(from);
        }
    };
    let report = apply::apply_with_faults_for_tests(
        &plan,
        "test",
        Some(&FailingSeal {
            prefix: b"returned",
        }),
        ApplyFaults {
            before_rename: Some(&mut tag_it),
            after_rename: None,
            ..Default::default()
        },
    )
    .expect("a report, not an error");
    // The report and the filesystem agree: the item is home and counted held.
    assert!(s.files[0].exists(), "the item is at its origin");
    assert!(!s.root.join("Collected/a.txt").exists());
    assert_eq!(report.held_tagged, 1);
    assert_eq!(report.moved, 0);
    assert_eq!(report.hold_record_failed, 1);
    assert_eq!(report.tagged_not_returned, 0);
    // Apply stopped at the journal failure: the next item was not touched.
    assert!(s.files[1].exists());

    // The journal never learned of the return, so it reads Moved. Undo finds
    // the recorded file at the origin and reconciles instead of failing.
    let mut journal = reload(&report);
    assert_eq!(state_of(&journal, "a.txt"), EntryState::Moved);
    let undone = apply::undo(&mut journal, &TestSeal);
    assert!(undone.error.is_none(), "{:?}", undone.error);
    assert_eq!(undone.reconciled, vec![s.files[0].clone()]);
    assert!(s.files[0].exists() && s.files[1].exists());
}

#[test]
fn a_failed_journal_write_after_a_stranded_item_is_reported() {
    let _g = lock();
    let s = scene("stranded_unrecorded", &["a.txt"]);
    let plan = plan_of(&s, false);
    let first = s.files[0].clone();
    let mut tag_it = |_: usize, from: &Path| tag(from);
    let occupant = first.clone();
    let mut occupy = |_: usize, _: &Path| {
        fs::write(&occupant, b"occupant").unwrap();
    };
    let report = apply::apply_with_faults_for_tests(
        &plan,
        "test",
        Some(&FailingSeal {
            prefix: b"stranded",
        }),
        ApplyFaults {
            before_rename: Some(&mut tag_it),
            after_rename: Some(&mut occupy),
            ..Default::default()
        },
    )
    .expect("a report");
    assert_eq!(report.tagged_not_returned, 1);
    assert_eq!(report.hold_record_failed, 1);
    assert_eq!(report.moved, 1);
    assert_eq!(fs::read(&first).unwrap(), b"occupant");
    assert!(s.root.join("Collected/a.txt").exists());
}

#[test]
fn without_a_journal_the_return_still_happens() {
    let _g = lock();
    let s = scene("no_journal", &["a.txt"]);
    let plan = plan_of(&s, false);
    let mut tag_it = |_: usize, from: &Path| tag(from);
    let report = apply::apply_with_faults_for_tests(
        &plan,
        "test",
        None,
        ApplyFaults {
            before_rename: Some(&mut tag_it),
            after_rename: None,
            ..Default::default()
        },
    )
    .expect("apply");
    assert_eq!((report.moved, report.held_tagged), (0, 1));
    assert!(s.files[0].exists());
    assert!(report.journal_path.is_none());
}

#[test]
fn a_tagged_item_in_a_plan_that_included_tagged_items_is_not_returned() {
    let _g = lock();
    let s = scene("included_late", &["a.txt"]);
    let plan = plan_of(&s, true);
    let mut tag_it = |_: usize, from: &Path| tag(from);
    let report = apply::apply_with_faults_for_tests(
        &plan,
        "test",
        Some(&TestSeal),
        ApplyFaults {
            before_rename: Some(&mut tag_it),
            after_rename: None,
            ..Default::default()
        },
    )
    .expect("apply");
    assert_eq!((report.moved, report.held_tagged), (1, 0));
    assert!(read_tag(&s.root.join("Collected/a.txt")).is_some());
}

/// Reading an attribute needs read permission, so a file nobody can read has a
/// tag state nobody can know. Unknown is not untagged: the scan holds it and
/// counts it as unreadable, and apply refuses to move it rather than guess.
#[test]
fn an_item_whose_tag_cannot_be_read_is_held_by_default() {
    use std::os::unix::fs::PermissionsExt;
    let _g = lock();
    let s = scene("unreadable", &["readable.txt", "locked.txt"]);
    let locked = s.files[1].clone();
    let plan = plan_of(&s, true);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

    let held = scan::scan(&s.root, &scan_config(false)).unwrap();
    assert_eq!(held.skipped_unreadable, 1);
    assert_eq!(held.skipped_tagged, 0);
    assert_eq!(held.entries.len(), 1);

    // A plan that names it anyway, made without consulting tags.
    let mut tagless = plan.clone();
    tagless.include_tagged = false;
    let failure =
        apply::apply_with_structured_progress(&tagless, "test", Some(&TestSeal), None, |_| {});
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o644)).unwrap();
    let error = failure.expect_err("an unknowable tag state stops apply");
    assert!(matches!(error, apply::ApplyError::Io(_)), "{error}");
    assert!(locked.exists(), "nothing was moved");
}

/// Finder comments are never read. The behavioural half: a comment changes
/// nothing about how an item is treated. The structural half: no production
/// source in the workspace names the comment attribute or lists attributes,
/// so a read whose result was thrown away would also turn this red.
#[test]
fn finder_comments_are_never_read() {
    let _g = lock();
    let s = scene("comments", &["commented.txt", "tagged.txt"]);
    set_attr(
        &s.files[0],
        COMMENT,
        b"a private comment that must not be read",
    );
    tag(&s.files[1]);
    let outcome = scan::scan(&s.root, &scan_config(false)).unwrap();
    assert_eq!(outcome.skipped_tagged, 1, "only the tag decides");
    assert_eq!(outcome.entries.len(), 1);
    assert_eq!(outcome.entries[0].name, "commented.txt");

    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut sources = Vec::new();
    for entry in fs::read_dir(crates).unwrap().flatten() {
        collect_sources(&entry.path().join("src"), &mut sources);
    }
    assert!(sources.len() > 20, "the walk found the workspace sources");
    for (path, text) in sources {
        let production = production(&text);
        for forbidden in ["kMDItemFinderComment", "listxattr", "flistxattr"] {
            assert!(
                !production.contains(forbidden),
                "{} mentions {forbidden}: Finder comments must never be read",
                path.display()
            );
        }
    }
}

/// Everything before the file's test module.
fn production(text: &str) -> &str {
    text.split("\n#[cfg(test)]\nmod ")
        .next()
        .unwrap_or_default()
}

fn collect_sources(dir: &Path, out: &mut Vec<(PathBuf, String)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push((path.clone(), fs::read_to_string(&path).unwrap()));
        }
    }
}

/// The tag probe asks for the attribute's length through a zero-length
/// buffer, so the tag names never enter the process.
#[test]
fn the_tag_probe_never_requests_the_value() {
    let source =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/scan.rs")).unwrap();
    let probe = production(&source)
        .split("pub fn has_finder_tag(path: &Path)")
        .nth(1)
        .unwrap()
        .split("#[cfg(not(target_os")
        .next()
        .unwrap();
    assert_eq!(probe.matches("getxattr(").count(), 1);
    assert!(
        probe.contains("std::ptr::null_mut(),\n            0,"),
        "the probe must pass a null buffer of length zero"
    );
}

/// Nothing but sweep changes behaviour through ScanConfig::default(). The
/// default holds tagged items; the only tool whose contract is to clear a
/// folder sets the opposite explicitly, and no other crate builds a scan.
#[test]
fn the_default_holds_tagged_items_and_only_stash_overrides_it() {
    assert!(!ScanConfig::default().include_tagged);

    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut sources = Vec::new();
    for entry in fs::read_dir(crates).unwrap().flatten() {
        collect_sources(&entry.path().join("src"), &mut sources);
    }
    let mut users: Vec<String> = sources
        .iter()
        .filter(|(path, text)| {
            // Compiled only into scan.rs's test module.
            !path.ends_with("project_document_hold.rs") && production(text).contains("ScanConfig {")
        })
        .map(|(path, _)| {
            path.strip_prefix(crates)
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    users.sort();
    assert_eq!(
        users,
        [
            // Snapshot capture: reads no tags, only the tree layout.
            "etude-core/src/scan.rs",
            // The holder: forwards --include-tagged, defaults to holding.
            "stash-cli/src/main.rs",
            "sweep-cli/src/main.rs",
        ],
        "a new consumer of ScanConfig must decide its tag policy and be named here"
    );
    let stash = fs::read_to_string(crates.join("stash-cli/src/main.rs")).unwrap();
    let production = production(&stash);
    assert_eq!(
        production.matches("ScanConfig {").count(),
        production.matches("include_tagged: true,").count(),
        "every stash scan must set include_tagged explicitly"
    );
}

#[test]
fn a_single_item_tagged_during_rename_leaves_no_empty_group() {
    let _g = lock();
    let s = scene("late_single", &["a.txt"]);
    let plan = plan_of(&s, false);
    let mut tag_it = |_: usize, from: &Path| tag(from);
    let report = apply::apply_with_faults_for_tests(
        &plan,
        "test",
        Some(&TestSeal),
        ApplyFaults {
            before_rename: Some(&mut tag_it),
            after_rename: None,
        },
    )
    .unwrap();
    assert_eq!((report.moved, report.held_tagged), (0, 1));
    assert!(s.files[0].exists());
    assert!(read_tag(&s.files[0]).is_some());
    assert!(
        !s.root.join("Collected").exists(),
        "empty group remains after return"
    );
}
