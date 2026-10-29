//! The immutable flag (`chflags uchg`, `UF_IMMUTABLE`): a locked file or
//! folder is held by the scan from the metadata it already has, apply checks
//! the flag again just before each move, a locked destination folder refuses
//! the plan before anything moves, and undo leaves a held item where it was.
#![cfg(target_os = "macos")]

use std::ffi::CString;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use etude_core::apply::{self, ApplyError};
use etude_core::journal::{EntryState, Journal, Sealer};
use etude_core::plan::{self, Group, Plan, Signal};
use etude_core::scan::{self, ScanConfig};

unsafe extern "C" {
    fn chflags(path: *const std::ffi::c_char, flags: u32) -> i32;
}

const UF_IMMUTABLE: u32 = 0x0000_0002;

fn set_flags(path: &Path, flags: u32) {
    let c = CString::new(path.as_os_str().as_bytes()).unwrap();
    // SAFETY: a valid C string; chflags reads it and nothing else.
    let rc = unsafe { chflags(c.as_ptr(), flags) };
    assert_eq!(rc, 0, "could not change flags on {}", path.display());
}

fn lock_path(path: &Path) {
    set_flags(path, UF_IMMUTABLE);
}

/// Clear the flag everywhere under `dir`, so the fixture can be removed.
fn unlock_tree(dir: &Path) {
    let c = CString::new(dir.as_os_str().as_bytes()).unwrap();
    // SAFETY: as above; a failure here only means the path is already gone.
    unsafe { chflags(c.as_ptr(), 0) };
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                unlock_tree(&path);
            } else {
                let c = CString::new(path.as_os_str().as_bytes()).unwrap();
                // SAFETY: as above.
                unsafe { chflags(c.as_ptr(), 0) };
            }
        }
    }
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

struct Scene {
    base: PathBuf,
    root: PathBuf,
    files: Vec<PathBuf>,
}

impl Drop for Scene {
    fn drop(&mut self) {
        unlock_tree(&self.base);
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn scene(label: &str, names: &[&str]) -> Scene {
    let base = std::env::temp_dir().join(format!("etude_locked_{label}_{}", std::process::id()));
    unlock_tree(&base);
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
    Scene { base, root, files }
}

fn config(whole_units: bool) -> ScanConfig {
    ScanConfig {
        grace: None,
        whole_units,
        ..ScanConfig::default()
    }
}

fn collected(scene: &Scene) -> Plan {
    let out = scan::scan(&scene.root, &config(false)).expect("scan");
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
fn the_scan_holds_a_locked_file_and_counts_it_without_its_name() {
    let _g = lock();
    let s = scene("scan", &["a.txt", "b.txt", "c.txt"]);
    lock_path(&s.files[1]);
    for whole_units in [false, true] {
        let out = scan::scan(&s.root, &config(whole_units)).unwrap();
        assert_eq!(out.locked_skipped, 1, "whole_units={whole_units}");
        let names: Vec<_> = out.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["a.txt", "c.txt"]);
        let plan = plan::build(&out);
        assert_eq!(plan.locked_skipped, 1);
        assert!(plan.to_json().contains("\"locked_skipped\":1"));
    }
}

#[test]
fn the_scan_holds_a_locked_folder_whole_and_does_not_enter_it() {
    let _g = lock();
    let s = scene("folder", &["a.txt"]);
    let folder = s.root.join("Keep");
    fs::create_dir(&folder).unwrap();
    fs::write(folder.join("inside.txt"), b"inside").unwrap();
    lock_path(&folder);
    let deep = ScanConfig {
        depth: 3,
        ..config(false)
    };
    let out = scan::scan(&s.root, &deep).unwrap();
    assert_eq!(out.locked_skipped, 1);
    let names: Vec<_> = out.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(
        names,
        ["a.txt"],
        "the locked folder's contents are not candidates"
    );
    let units = scan::scan(&s.root, &config(true)).unwrap();
    assert_eq!(units.locked_skipped, 1);
    assert_eq!(
        units.entries.len(),
        1,
        "a locked folder is not a movable unit"
    );
}

#[test]
fn a_file_locked_after_the_scan_is_held_at_move_time_and_undo_leaves_it() {
    let _g = lock();
    let s = scene("late", &["a.txt", "b.txt", "c.txt"]);
    let plan = collected(&s);
    lock_path(&s.files[1]);
    let report =
        apply::apply_with_structured_progress(&plan, "test", Some(&TestSeal), None, |_| {})
            .expect("apply");
    assert_eq!(report.moved, 2);
    assert_eq!(report.held_locked, 1);
    assert_eq!(report.held_tagged, 0);
    assert!(s.files[1].exists(), "the locked item stayed");
    assert!(!s.files[0].exists() && !s.files[2].exists());
    assert!(s.root.join("Collected/a.txt").exists());

    let journal = reload(&report);
    assert_eq!(state_of(&journal, "a.txt"), EntryState::Moved);
    assert_eq!(state_of(&journal, "b.txt"), EntryState::Held);
    assert_eq!(state_of(&journal, "c.txt"), EntryState::Moved);

    let mut journal = journal;
    let undone = apply::undo(&mut journal, &TestSeal);
    assert_eq!(undone.restored, 2);
    assert!(undone.error.is_none());
    assert!(s.files.iter().all(|f| f.exists()));
    assert!(!s.root.join("Collected/b.txt").exists());
}

#[test]
fn a_destination_folder_locked_after_planning_refuses_before_any_move() {
    let _g = lock();
    let s = scene("dest", &["a.txt", "b.txt"]);
    let destination = s.root.join("Collected");
    fs::create_dir(&destination).unwrap();
    let plan = collected(&s);
    lock_path(&destination);
    let failure =
        apply::apply_with_structured_progress(&plan, "test", Some(&TestSeal), None, |_| {})
            .expect_err("a locked destination refuses");
    assert!(
        matches!(failure, ApplyError::DestinationLocked(_)),
        "{failure:?}"
    );
    assert!(s.files.iter().all(|f| f.exists()), "nothing moved");
    assert!(fs::read_dir(&destination).unwrap().next().is_none());
}

#[test]
fn a_destination_folder_locked_at_planning_holds_its_group() {
    let _g = lock();
    let names = [
        "Screenshot 2024-01-01 at 1.png",
        "Screenshot 2024-01-01 at 2.png",
        "Screenshot 2024-01-01 at 3.png",
    ];
    let s = scene("planned", &names);
    let destination = s.root.join("Screenshots");
    fs::create_dir(&destination).unwrap();
    lock_path(&destination);
    let out = scan::scan(&s.root, &config(false)).unwrap();
    let plan = plan::build(&out);
    assert!(
        plan.groups.iter().all(|g| g.name != "Screenshots"),
        "no group moves into a locked folder: {:?}",
        plan.groups
            .iter()
            .map(|g| g.name.clone())
            .collect::<Vec<_>>()
    );
    // The folder itself and the three files it would have received.
    assert_eq!(plan.locked_skipped, 4);
}

#[test]
fn a_tag_plan_never_tags_a_locked_file() {
    let _g = lock();
    let s = scene("tags", &["a.txt", "b.txt", "c.txt"]);
    lock_path(&s.files[1]);
    let out = scan::scan(&s.root, &config(false)).unwrap();
    let specs = plan::validate_tag_maps(&[("txt".into(), "Notes".into())]).unwrap();
    let mut plan = plan::build_tags(&out, &specs, None);
    for group in &mut plan.groups {
        group.accepted = true;
    }
    assert_eq!(plan.locked_skipped, 1);
    assert!(
        plan.groups
            .iter()
            .all(|g| !g.members.iter().any(|m| m == &s.files[1])),
        "the locked file is not in any tag group"
    );
    lock_path(&s.files[2]);
    let report =
        apply::apply_with_structured_progress(&plan, "test", Some(&TestSeal), None, |_| {})
            .expect("apply");
    assert_eq!(report.tagged, 1);
    assert_eq!(report.held_locked, 1);
}
