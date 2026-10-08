use std::path::PathBuf;
use std::process::{Command, Output};
struct Fixture {
    base: PathBuf,
    root: PathBuf,
    file: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let base = std::env::temp_dir().join(format!(
            "sweep-plan-replay-{}-{}-{sequence}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let root = base.join("selected");
        std::fs::create_dir_all(&root).unwrap();
        for i in 0..4 {
            std::fs::write(
                root.join(format!("Screenshot 2026-01-01 at 10.0{i}.png")),
                b"synthetic payload",
            )
            .unwrap();
        }
        Self {
            file: base.join("approved.plan"),
            base,
            root,
        }
    }
    fn run(&self, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_sweep"))
            .args(arguments)
            .env("ETUDE_STATE_DIR", self.base.join("state"))
            .env("ETUDE_JOURNAL_KEY", "42".repeat(32))
            .env("SWEEP_GRACE_SECS", "0")
            .output()
            .unwrap()
    }
    fn export(&self) -> String {
        let out = self.run(&[
            self.root.to_str().unwrap(),
            "--since",
            "0",
            "--export-plan",
            self.file.to_str().unwrap(),
            "--json",
        ]);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let text = String::from_utf8(out.stdout).unwrap();
        let tail = text
            .split("\"plan_digest\":\"")
            .nth(1)
            .expect("binding digest in exported result");
        let digest = tail.split('"').next().unwrap().to_string();
        assert_eq!(digest.len(), 64);
        digest
    }
    fn replay(&self, digest: &str) -> Output {
        self.run(&[
            "apply",
            "--plan",
            self.file.to_str().unwrap(),
            "--plan-digest",
            digest,
            "--yes",
            "--json",
        ])
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}
#[test]
fn saved_plan_is_actually_replayed_without_replanning_changed_trees() {
    for change in [
        "new_file",
        "changed_file",
        "project_marker",
        "root_replacement",
    ] {
        let fixture = Fixture::new();
        let digest = fixture.export();
        match change {
            "new_file" => std::fs::write(fixture.root.join("later.txt"), b"new").unwrap(),
            "changed_file" => std::fs::write(
                fixture.root.join("Screenshot 2026-01-01 at 10.00.png"),
                b"edited",
            )
            .unwrap(),
            "project_marker" => {
                std::fs::write(fixture.root.join("Cargo.toml"), b"[package]").unwrap()
            }
            _ => {
                std::fs::rename(&fixture.root, fixture.base.join("original")).unwrap();
                std::fs::create_dir(&fixture.root).unwrap();
            }
        }
        let before = std::fs::read_dir(&fixture.root).unwrap().count();
        let out = fixture.replay(&digest);
        assert_eq!(
            out.status.code(),
            Some(2),
            "{change}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(String::from_utf8_lossy(&out.stderr).contains("replan required"));
        assert!(String::from_utf8_lossy(&out.stdout).contains("\"status\":\"refused\""));
        assert_eq!(std::fs::read_dir(&fixture.root).unwrap().count(), before);
        assert!(
            !fixture.base.join("state").exists(),
            "no journal/key state before refusing stale observations"
        );
    }
}
#[test]
fn unchanged_saved_plan_applies_and_undo_preserves_the_fixture() {
    let fixture = Fixture::new();
    let digest = fixture.export();
    let out = fixture.replay(&digest);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read_dir(&fixture.root)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().unwrap().is_file())
            .count(),
        0
    );
    let out = fixture.run(&["undo", fixture.root.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        std::fs::read_dir(&fixture.root)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().unwrap().is_file())
            .count(),
        4
    );
}
#[test]
fn replay_requires_original_digest_and_rejects_configuration_overrides() {
    let fixture = Fixture::new();
    let digest = fixture.export();
    assert_eq!(
        fixture
            .run(&[
                "apply",
                "--plan",
                fixture.file.to_str().unwrap(),
                "--yes",
                "--json"
            ])
            .status
            .code(),
        Some(2)
    );
    assert_eq!(fixture.replay(&"0".repeat(64)).status.code(), Some(2));
    assert_eq!(
        fixture
            .run(&[
                "apply",
                "--plan",
                fixture.file.to_str().unwrap(),
                "--plan-digest",
                &digest,
                "--yes",
                "--since",
                "0",
                "--json"
            ])
            .status
            .code(),
        Some(2)
    );
    assert_eq!(
        fixture
            .run(&[
                "apply",
                fixture.root.to_str().unwrap(),
                "--plan",
                fixture.file.to_str().unwrap(),
                "--plan-digest",
                &digest,
                "--yes",
                "--json"
            ])
            .status
            .code(),
        Some(2)
    );
    assert!(!fixture.base.join("state").exists());
}
