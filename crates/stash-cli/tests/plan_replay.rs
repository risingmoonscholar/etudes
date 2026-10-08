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
            "stash-plan-replay-{}-{}-{sequence}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let root = base.join("selected");
        std::fs::create_dir_all(&root).unwrap();
        for i in 0..3 {
            std::fs::write(root.join(format!("item-{i}.txt")), b"synthetic payload").unwrap();
        }
        Self {
            file: base.join("approved.plan"),
            base,
            root,
        }
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_stash"))
            .args(args)
            .env("ETUDE_STATE_DIR", self.base.join("state"))
            .env("ETUDE_JOURNAL_KEY", "43".repeat(32))
            .output()
            .unwrap()
    }
    fn export(&self) -> String {
        let out = self.run(&[
            self.root.to_str().unwrap(),
            "--for",
            "3d",
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
        assert_eq!(std::fs::read_dir(&self.root).unwrap().count(), 3);
        let text = String::from_utf8(out.stdout).unwrap();
        text.split("\"plan_digest\":\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap()
            .to_string()
    }
    fn replay(&self, digest: &str) -> Output {
        self.run(&[
            "--plan",
            self.file.to_str().unwrap(),
            "--plan-digest",
            digest,
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
fn stash_rejects_a_changed_tree_instead_of_stashing_a_fresh_plan() {
    let f = Fixture::new();
    let digest = f.export();
    std::fs::write(f.root.join("later.txt"), b"new").unwrap();
    let out = f.replay(&digest);
    assert_eq!(
        out.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("replan required"));
    assert_eq!(std::fs::read_dir(&f.root).unwrap().count(), 4);
    assert!(!f.base.join("state").exists());
}
#[test]
fn stash_replays_exact_original_deadline_and_restores_all_members() {
    let f = Fixture::new();
    let digest = f.export();
    let out = f.replay(&digest);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = f.run(&["pop", f.root.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(std::fs::read_dir(&f.root).unwrap().count(), 3);
}
#[test]
fn stash_cannot_override_an_exported_holding_deadline_or_digest() {
    let f = Fixture::new();
    let digest = f.export();
    assert_eq!(f.replay(&"0".repeat(64)).status.code(), Some(2));
    let out = f.run(&[
        "--plan",
        f.file.to_str().unwrap(),
        "--plan-digest",
        &digest,
        "--for",
        "1w",
        "--json",
    ]);
    assert_eq!(out.status.code(), Some(2));
    assert!(!f.base.join("state").exists());
}
