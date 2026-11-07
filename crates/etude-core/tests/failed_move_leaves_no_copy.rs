//! A move that fails after it made a second name at the destination, and
//! before the source was removed, must take that name back. The caller then
//! says "nothing was moved", and that must be true: no extra name may sit in
//! the destination where undo cannot see it. When the name cannot be taken
//! back, the failure names it instead.
//!
//! The link-then-unlink fallback is forced with the existing fault hooks, and
//! the source's folder is made read-only after the link, so removing the
//! source fails the way it does on a real volume.
#![cfg(target_os = "macos")]
use etude_core::{
    apply::{self, ApplyError, ApplyFaults},
    journal::{Journal, Sealer},
    plan::{Group, Plan, Signal},
    scan::{self, ScanConfig},
};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::Mutex,
};

static ENV: Mutex<()> = Mutex::new(());

struct Fixture(PathBuf, Option<std::ffi::OsString>);
impl Drop for Fixture {
    fn drop(&mut self) {
        for folder in ["root", "root/First"] {
            let _ = fs::set_permissions(self.0.join(folder), fs::Permissions::from_mode(0o755));
        }
        let _ = fs::remove_dir_all(&self.0);
        unsafe {
            match &self.1 {
                Some(value) => std::env::set_var("ETUDE_STATE_DIR", value),
                None => std::env::remove_var("ETUDE_STATE_DIR"),
            }
        }
    }
}

struct Seal;
impl Sealer for Seal {
    fn seal(&self, plain: &[u8]) -> Result<Vec<u8>, &'static str> {
        Ok(plain.iter().map(|b| b ^ 0x5a).collect())
    }
    fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, &'static str> {
        Ok(sealed.iter().map(|b| b ^ 0x5a).collect())
    }
}

fn read_only(path: &Path) {
    fs::set_permissions(path, fs::Permissions::from_mode(0o555)).unwrap();
}

/// Run one apply of two files whose first move fails after its link is made.
/// `lock_destination` also makes the destination folder read-only, so the
/// link cannot be taken back.
fn run(lock_destination: bool) {
    let _lock = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let base = std::env::temp_dir().join(format!(
        "failed-move-no-copy-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let f = Fixture(base, std::env::var_os("ETUDE_STATE_DIR"));
    fs::create_dir_all(f.0.join("root")).unwrap();
    unsafe {
        std::env::set_var("ETUDE_STATE_DIR", f.0.join("state"));
    }
    let root = f.0.join("root").canonicalize().unwrap();
    let sources: Vec<_> = (0..2)
        .map(|i| {
            let p = root.join(format!("file{i}.txt"));
            fs::write(&p, format!("payload {i}")).unwrap();
            p
        })
        .collect();
    let scan = scan::scan(
        &root,
        &ScanConfig {
            depth: 2,
            grace: None,
            ..Default::default()
        },
    )
    .unwrap();
    let plan = Plan::with_groups(
        &scan,
        vec![Group {
            name: "First".into(),
            signal: Signal::Collected { count: 2 },
            members: sources.clone(),
            accepted: true,
        }],
    );
    let (root_hook, destination_hook) = (root.clone(), root.join("First"));
    let mut after_link = |i: usize, _: &Path| {
        if i == 0 {
            read_only(&root_hook);
            if lock_destination {
                read_only(&destination_hook);
            }
        }
    };
    let failure = apply::apply_with_faults_for_tests(
        &plan,
        "verify",
        Some(&Seal),
        ApplyFaults {
            force_link_unlink: true,
            after_hard_link: Some(&mut after_link),
            ..Default::default()
        },
    )
    .expect_err("removing the source must fail");
    for folder in [&root, &root.join("First")] {
        fs::set_permissions(folder, fs::Permissions::from_mode(0o755)).unwrap();
    }
    assert!(
        matches!(&failure.error, ApplyError::Io(e) if e.raw_os_error() == Some(13)),
        "the original error is kept: {:?}",
        failure.error
    );
    assert_eq!(failure.moved, 0, "nothing was moved");
    for (i, source) in sources.iter().enumerate() {
        assert_eq!(fs::read(source).unwrap(), format!("payload {i}").as_bytes());
    }
    let first = root.join("First/file0.txt");
    if lock_destination {
        assert_eq!(
            failure.copy_left.as_deref(),
            Some(first.as_path()),
            "the failure names the second name it could not remove"
        );
        assert_eq!(fs::read(&first).unwrap(), b"payload 0");
    } else {
        assert_eq!(failure.copy_left, None);
        assert!(
            !root.join("First").exists() || fs::read_dir(root.join("First")).unwrap().count() == 0,
            "no second name is left in the destination"
        );
    }
    // Undo of the journal restores nothing: nothing was moved.
    let id = failure.durable_journal_id.expect("journal was written");
    let mut journal = Journal::load_sealed("verify", &id, &Seal).unwrap();
    let undo = apply::undo(&mut journal, &Seal);
    assert!(undo.error.is_none(), "{:?}", undo.error);
    assert_eq!(undo.restored, 0);
}

#[test]
fn a_failed_source_removal_takes_back_the_second_name() {
    run(false);
}

#[test]
fn a_second_name_that_cannot_be_taken_back_is_named() {
    run(true);
}
