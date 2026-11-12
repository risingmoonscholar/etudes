//! Counts are checked after undo as well, so a count mismatch cannot hide a
//! recovery failure. Every injected mutation is confined to the fixture.
#![cfg(target_os = "macos")]
use etude_core::{
    apply::{self, ApplyError, ApplyFaults},
    journal::{Journal, Sealer},
    plan::{Group, Plan, Signal},
    scan::{self, ScanConfig},
};
use std::{
    cell::Cell,
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};
static ENV: Mutex<()> = Mutex::new(());

struct Fixture(PathBuf, Option<std::ffi::OsString>);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
        unsafe {
            match &self.1 {
                Some(value) => std::env::set_var("ETUDE_STATE_DIR", value),
                None => std::env::remove_var("ETUDE_STATE_DIR"),
            }
        }
    }
}
impl Fixture {
    fn new() -> Self {
        let base = std::env::temp_dir().join(format!(
            "verify-counts-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let f = Self(base, std::env::var_os("ETUDE_STATE_DIR"));
        fs::create_dir_all(f.0.join("root")).unwrap();
        unsafe {
            std::env::set_var("ETUDE_STATE_DIR", f.0.join("state"));
        }
        f
    }
}
struct Seal {
    fail: &'static str,
    seen: Cell<usize>,
}
impl Sealer for Seal {
    fn seal(&self, plain: &[u8]) -> Result<Vec<u8>, &'static str> {
        if !self.fail.is_empty() && plain.starts_with(self.fail.as_bytes()) {
            let n = self.seen.get();
            self.seen.set(n + 1);
            // Fail once; recovery writes must work.
            if n == 0 {
                return Err("verify-counts injected seal failure");
            }
        }
        Ok(plain.iter().map(|b| b ^ 0x5a).collect())
    }
    fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, &'static str> {
        Ok(sealed.iter().map(|b| b ^ 0x5a).collect())
    }
}
fn tag(path: &Path) {
    etude_core::xattr::write(path, etude_core::tags::ATTRIBUTE, b"tag").unwrap();
}

fn verify(case: &str) {
    let _lock = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let f = Fixture::new();
    let root = f.0.join("root").canonicalize().unwrap();
    let sources: Vec<_> = (0..3)
        .map(|i| {
            let p = root.join(format!("file{i}.txt"));
            fs::write(&p, format!("payload {i}")).unwrap();
            p
        })
        .collect();
    let observed = case == "observed_directory";
    let separate = observed
        || matches!(
            case,
            "outside_root" | "blocked_directory" | "appeared_directory" | "created_record"
        );
    if observed {
        fs::create_dir(root.join("Second")).unwrap();
    }
    let scan = scan::scan(
        &root,
        &ScanConfig {
            depth: 2,
            grace: None,
            ..Default::default()
        },
    )
    .unwrap();
    let groups = if separate {
        vec![
            Group {
                name: "First".into(),
                signal: Signal::Collected { count: 1 },
                members: sources[..1].to_vec(),
                accepted: true,
            },
            Group {
                name: "Second".into(),
                signal: Signal::Collected { count: 2 },
                members: sources[1..].to_vec(),
                accepted: true,
            },
        ]
    } else {
        vec![Group {
            name: "First".into(),
            signal: Signal::Collected { count: 3 },
            members: sources.clone(),
            accepted: true,
        }]
    };
    let mut plan = Plan::with_groups(&scan, groups);
    if case == "created_directory" {
        plan.include_tagged = true;
    }
    let fail = match case {
        "done_record" => "done\t1\t",
        "held_record" => "held\t1\n",
        "created_record" => "created-dir\t",
        _ => "",
    };
    let seal = Seal {
        fail,
        seen: Cell::new(0),
    };
    // Allow the first group's directory record, fail the second group's.
    if case == "created_record" {
        seal.seen.set(1);
    }
    let mut before = |i: usize, from: &Path| {
        if i != 1 {
            return;
        }
        match case {
            "ineligible" => {
                fs::rename(from, f.0.join("parked-source")).unwrap();
                fs::create_dir(from).unwrap();
            }
            "rename_io" => {
                fs::write(root.join("First/file1.txt"), b"unrelated occupant").unwrap();
            }
            _ => {}
        }
    };
    let mut after = |i: usize, to: &Path| {
        if case == "destination_tag_read" && i == 1 {
            fs::rename(to, f.0.join("parked-destination")).unwrap();
        }
        if i != 0 {
            return;
        }
        match case {
            "source_tag_read" => {
                fs::rename(&sources[1], f.0.join("parked-source")).unwrap();
            }
            "held_record" => tag(&sources[1]),
            "observed_directory" => {
                fs::rename(root.join("Second"), f.0.join("old-directory")).unwrap();
                fs::create_dir(root.join("Second")).unwrap();
            }
            "created_directory" => {
                fs::rename(root.join("First"), f.0.join("old-directory")).unwrap();
                fs::create_dir(root.join("First")).unwrap();
            }
            "outside_root" => {
                std::os::unix::fs::symlink(&f.0, root.join("Second")).unwrap();
            }
            "blocked_directory" => {
                fs::write(root.join("Second"), b"blocker").unwrap();
            }
            "appeared_directory" => {
                fs::create_dir(root.join("Second")).unwrap();
            }
            "created_record" => seal.seen.set(0),
            _ => {}
        }
    };
    let result = if case == "fail_at" {
        apply::apply_with_structured_progress_outcome(&plan, "verify", Some(&seal), Some(1), |_| {})
    } else {
        apply::apply_with_faults_for_tests(
            &plan,
            "verify",
            Some(&seal),
            ApplyFaults {
                before_rename: Some(&mut before),
                after_rename: Some(&mut after),
                ..Default::default()
            },
        )
    };
    let failure = result.expect_err("must reach the partial-failure path");
    match case {
        "fail_at" => assert!(matches!(failure.error, ApplyError::Injected(1))),
        "observed_directory" | "created_directory" => assert!(matches!(
            failure.error,
            ApplyError::DestinationDirectoryChanged(_)
        )),
        "outside_root" => assert!(matches!(failure.error, ApplyError::StalePlan(_))),
        "blocked_directory" => assert!(matches!(failure.error, ApplyError::DestinationBlocked(_))),
        "done_record" | "held_record" | "created_record" => {
            assert!(matches!(failure.error, ApplyError::Journal(_)))
        }
        _ => assert!(matches!(failure.error, ApplyError::Io(_))),
    }
    // Undo is allowed to see the original tree after the competing mutation
    // has been removed; it must not restore an entry apply never moved.
    match case {
        "source_tag_read" => fs::rename(f.0.join("parked-source"), &sources[1]).unwrap(),
        "ineligible" => {
            fs::remove_dir(&sources[1]).unwrap();
            fs::rename(f.0.join("parked-source"), &sources[1]).unwrap();
        }
        "destination_tag_read" => {
            fs::rename(f.0.join("parked-destination"), root.join("First/file1.txt")).unwrap()
        }
        "observed_directory" | "created_directory" => {
            let name = if observed { "Second" } else { "First" };
            fs::remove_dir(root.join(name)).unwrap();
            fs::rename(f.0.join("old-directory"), root.join(name)).unwrap();
        }
        "outside_root" => fs::remove_file(root.join("Second")).unwrap(),
        _ => {}
    }
    let actual = if matches!(case, "done_record" | "destination_tag_read") {
        2
    } else {
        1
    };
    for (i, source) in sources.iter().enumerate() {
        let dest = root
            .join(if separate && i > 0 { "Second" } else { "First" })
            .join(source.file_name().unwrap());
        if i < actual {
            assert!(!source.exists());
            assert_eq!(fs::read(dest).unwrap(), format!("payload {i}").as_bytes());
        } else {
            assert_eq!(fs::read(source).unwrap(), format!("payload {i}").as_bytes());
            if !(case == "rename_io" && i == 1) {
                assert!(!dest.exists(), "unmoved entry has no destination");
            }
        }
    }
    let id = failure
        .durable_journal_id
        .as_ref()
        .expect("durable recovery handle");
    let mut journal = Journal::load_sealed("verify", id, &seal).unwrap();
    assert_eq!(
        journal.entries.iter().filter(|e| e.is_moved()).count(),
        if case == "done_record" { 1 } else { actual }
    );
    let undo = apply::undo(&mut journal, &seal);
    assert!(undo.error.is_none(), "undo error: {:?}", undo.error);
    assert_eq!(
        undo.restored, actual,
        "undo must restore exactly the physical moves"
    );
    for (i, source) in sources.iter().enumerate() {
        assert_eq!(fs::read(source).unwrap(), format!("payload {i}").as_bytes());
    }
    if case == "rename_io" {
        assert_eq!(
            fs::read(root.join("First/file1.txt")).unwrap(),
            b"unrelated occupant"
        );
    }
    if case == "blocked_directory" {
        assert_eq!(fs::read(root.join("Second")).unwrap(), b"blocker");
    }
    let again = apply::undo(&mut journal, &seal);
    assert!(again.error.is_none());
    assert_eq!(again.restored, 0);
    assert_eq!(
        failure.moved, actual,
        "{case}: reported count must equal files actually moved"
    );
}
macro_rules! cases { ($($name:ident),* $(,)?) => { $(#[test] fn $name() { verify(stringify!($name)); })* }; }
cases!(
    fail_at,
    observed_directory,
    created_directory,
    ineligible,
    rename_io,
    source_tag_read,
    destination_tag_read,
    done_record,
    held_record,
    outside_root,
    blocked_directory,
    appeared_directory,
    created_record
);
