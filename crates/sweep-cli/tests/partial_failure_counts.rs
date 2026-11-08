//! Exercise the shipped CLI after an actual mid-loop I/O failure. Core fault
//! hooks are not exposed by the production CLI, so a read-only second route
//! supplies the deterministic failure here.
#![cfg(unix)]
use std::{
    fs,
    io::Read,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Command, Output},
};
struct Fixture {
    base: PathBuf,
    key: String,
}
impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let base = std::env::temp_dir().join(format!(
            "sweep-verify-counts-{sequence}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut random = [0u8; 32];
        fs::File::open("/dev/urandom")
            .unwrap()
            .read_exact(&mut random)
            .unwrap();
        let f = Self {
            base,
            key: random.iter().map(|b| format!("{b:02x}")).collect(),
        };
        for dir in ["root/First", "root/Second", "state", "home"] {
            fs::create_dir_all(f.base.join(dir)).unwrap();
        }
        fs::write(f.base.join("state/sweep-existing-profile.toml"), "schema_version = 1\n[[routes]]\nextension = \"pdf\"\nfolder = \"First\"\n[[routes]]\nextension = \"png\"\nfolder = \"Second\"\n").unwrap();
        for name in ["a.pdf", "b.pdf", "e.pdf", "c.png", "d.png", "f.png"] {
            fs::write(f.base.join("root").join(name), format!("payload {name}")).unwrap();
        }
        fs::set_permissions(
            f.base.join("root/Second"),
            fs::Permissions::from_mode(0o555),
        )
        .unwrap();
        f
    }
    fn run(&self, args: &[&str], json: bool) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_sweep"));
        cmd.args(args).arg(self.base.join("root"));
        if json {
            cmd.arg("--json");
        }
        cmd.env("ETUDE_STATE_DIR", self.base.join("state"))
            .env("ETUDE_JOURNAL_KEY", &self.key)
            .env("HOME", self.base.join("home"))
            .env("SWEEP_GRACE_SECS", "0")
            .output()
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::set_permissions(
            self.base.join("root/Second"),
            fs::Permissions::from_mode(0o755),
        );
        let _ = fs::remove_dir_all(&self.base);
    }
}
fn verify(json: bool) {
    let f = Fixture::new();
    let out = f.run(
        &["apply", "--yes", "--scheme", "existing", "--since", "0"],
        json,
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    // This is a fixture precondition, not a skip: privileged execution must
    // not silently turn a failure-path test into success-path coverage.
    assert_eq!(
        out.status.code(),
        Some(3),
        "expected incomplete apply: {stdout}\n{stderr}"
    );
    for name in ["a.pdf", "b.pdf", "e.pdf"] {
        assert!(!f.base.join("root").join(name).exists());
        assert_eq!(
            fs::read(f.base.join("root/First").join(name)).unwrap(),
            format!("payload {name}").as_bytes()
        );
    }
    for name in ["c.png", "d.png", "f.png"] {
        assert_eq!(
            fs::read(f.base.join("root").join(name)).unwrap(),
            format!("payload {name}").as_bytes()
        );
        assert!(!f.base.join("root/Second").join(name).exists());
    }
    // Run undo before asserting reported counts so mismatches still exercise
    // recovery and the independent disk witness.
    let undo = f.run(&["undo"], true);
    let undo_text = String::from_utf8_lossy(&undo.stdout);
    assert_eq!(
        undo.status.code(),
        Some(0),
        "undo: {undo_text}\n{}",
        String::from_utf8_lossy(&undo.stderr)
    );
    for name in ["a.pdf", "b.pdf", "e.pdf", "c.png", "d.png", "f.png"] {
        assert_eq!(
            fs::read(f.base.join("root").join(name)).unwrap(),
            format!("payload {name}").as_bytes()
        );
    }
    assert_eq!(fs::read_dir(f.base.join("root/First")).unwrap().count(), 0);
    assert_eq!(fs::read_dir(f.base.join("root/Second")).unwrap().count(), 0);
    assert!(
        undo_text.contains("\"name\":\"items_restored\",\"count\":3"),
        "undo must restore only three: {undo_text}"
    );
    if json {
        assert!(
            stdout.contains("\"name\":\"items_moved\",\"count\":3"),
            "disk moved 3: {stdout}"
        );
        assert!(stdout.contains("\"status\":\"incomplete\""), "{stdout}");
    } else {
        assert!(
            stderr.contains("3 files were moved before this stopped."),
            "disk moved 3: {stdout}\n{stderr}"
        );
        assert!(!stderr.contains("Nothing was moved"), "{stderr}");
    }
}
#[test]
fn partial_io_human_counts_and_undo() {
    verify(false);
}
#[test]
fn partial_io_json_counts_and_undo() {
    verify(true);
}
/// An apply that stops after moving files into a saved-route folder it
/// created names that folder and says undo removes it; undo then does.
#[test]
fn a_stopped_apply_names_the_folder_it_created_and_undo_removes_it() {
    let f = Fixture::new();
    fs::remove_dir(f.base.join("root/First")).unwrap();
    let out = f.run(
        &["apply", "--yes", "--scheme", "existing", "--since", "0"],
        false,
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(3),
        "expected incomplete apply: {stderr}"
    );
    assert!(
        stderr.contains("3 files were moved before this stopped."),
        "{stderr}"
    );
    assert!(
        stderr.contains("This apply created the folder First; undo removes it once it is empty."),
        "{stderr}"
    );
    assert!(f.base.join("root/First").is_dir());
    let undo = f.run(&["undo"], false);
    assert_eq!(
        undo.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&undo.stderr)
    );
    for name in ["a.pdf", "b.pdf", "e.pdf", "c.png", "d.png", "f.png"] {
        assert_eq!(
            fs::read(f.base.join("root").join(name)).unwrap(),
            format!("payload {name}").as_bytes()
        );
    }
    assert!(
        !f.base.join("root/First").exists(),
        "undo left the folder the stopped apply created"
    );
}
