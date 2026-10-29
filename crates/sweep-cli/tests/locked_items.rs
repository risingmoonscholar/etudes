//! The immutable flag at the command line: a locked item is held and counted,
//! the count is in the plan JSON and the human summary, and apply holds an
//! item locked after the scan.
#![cfg(target_os = "macos")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const KEY: &str = "0303030303030303030303030303030303030303030303030303030303030303";

struct Scene {
    root: PathBuf,
    state: PathBuf,
}

impl Drop for Scene {
    fn drop(&mut self) {
        let _ = Command::new("chflags")
            .args(["-R", "nouchg"])
            .arg(&self.root)
            .status();
        let _ = fs::remove_dir_all(self.root.parent().unwrap());
    }
}

fn scene(label: &str) -> Scene {
    let base = std::env::temp_dir().join(format!(
        "sweep-locked-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let root = base.join("root");
    let state = base.join("state");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&state).unwrap();
    for n in 1..=5 {
        fs::write(root.join(format!("document-{n}.pdf")), b"x").unwrap();
    }
    Scene {
        root: root.canonicalize().unwrap(),
        state,
    }
}

fn lock(path: &Path) {
    assert!(
        Command::new("chflags")
            .arg("uchg")
            .arg(path)
            .status()
            .unwrap()
            .success()
    );
}

fn sweep(scene: &Scene, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sweep"))
        .env("ETUDE_STATE_DIR", &scene.state)
        .env("ETUDE_JOURNAL_KEY", KEY)
        .args(args)
        .output()
        .unwrap()
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

#[test]
fn a_locked_item_is_counted_without_its_name_and_never_moved() {
    let s = scene("count");
    lock(&s.root.join("document-2.pdf"));
    let root = s.root.to_str().unwrap();

    let text = out(&sweep(&s, &[root, "--since", "0"]));
    assert!(text.contains("1 locked item left alone by flag"), "{text}");

    let json = out(&sweep(&s, &[root, "--since", "0", "--json"]));
    assert!(json.contains("\"locked_skipped\":1"), "{json}");
    assert!(json.contains("\"scanned\":4"), "{json}");
    assert!(!json.contains("document-2.pdf"), "{json}");

    let applied = sweep(&s, &["apply", root, "--since", "0", "--yes"]);
    assert!(applied.status.success(), "{}", out(&applied));
    assert!(
        s.root.join("document-2.pdf").exists(),
        "the locked item stayed"
    );
    assert!(s.root.join("Documents/document-1.pdf").exists());
    let undone = sweep(&s, &["undo", root]);
    assert!(undone.status.success(), "{}", out(&undone));
    assert!(s.root.join("document-1.pdf").exists());
}

/// Locking changes ctime, which the exported plan is bound to, so the replay
/// is refused before apply's own move-time check. Nothing moves either way.
#[test]
fn an_item_locked_after_planning_refuses_the_exported_plan_and_moves_nothing() {
    let s = scene("late");
    let root = s.root.to_str().unwrap();
    let plan = s.state.join("plan.bin");
    let exported = sweep(
        &s,
        &[
            root,
            "--since",
            "0",
            "--export-plan",
            plan.to_str().unwrap(),
            "--json",
        ],
    );
    let exported_json = out(&exported);
    assert!(exported.status.success(), "{exported_json}");
    let digest = exported_json
        .split("\"plan_digest\":\"")
        .nth(1)
        .expect("plan digest")
        .split('"')
        .next()
        .unwrap()
        .to_string();
    lock(&s.root.join("document-3.pdf"));
    let applied = sweep(
        &s,
        &[
            "apply",
            "--plan",
            plan.to_str().unwrap(),
            "--plan-digest",
            &digest,
            "--yes",
            "--json",
        ],
    );
    let json = out(&applied);
    assert_eq!(applied.status.code(), Some(2), "{json}");
    assert!(json.contains("\"code\":\"plan_stale\""), "{json}");
    for n in 1..=5 {
        assert!(
            s.root.join(format!("document-{n}.pdf")).exists(),
            "nothing moved"
        );
    }
}
