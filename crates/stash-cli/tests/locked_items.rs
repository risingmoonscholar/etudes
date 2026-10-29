//! stash and the immutable flag: a locked item cannot be moved, so stash
//! leaves it, counts it and says the folder is not clear.
#![cfg(target_os = "macos")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const KEY: &str = "0404040404040404040404040404040404040404040404040404040404040404";

struct Scene {
    base: PathBuf,
    root: PathBuf,
    state: PathBuf,
}

impl Drop for Scene {
    fn drop(&mut self) {
        let _ = Command::new("chflags")
            .args(["-R", "nouchg"])
            .arg(&self.base)
            .status();
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn scene(label: &str) -> Scene {
    let base = std::env::temp_dir().join(format!(
        "stash-locked-{label}-{}-{}",
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
    Scene {
        root: root.canonicalize().unwrap(),
        base,
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

fn stash(scene: &Scene, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_stash"))
        .env("ETUDE_STATE_DIR", &scene.state)
        .env("ETUDE_JOURNAL_KEY", KEY)
        .args(args)
        .output()
        .unwrap()
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

#[test]
fn stash_holds_a_locked_item_and_reports_the_folder_not_clear() {
    let s = scene("held");
    fs::write(s.root.join("plain.txt"), b"plain").unwrap();
    fs::write(s.root.join("other.txt"), b"other").unwrap();
    fs::write(s.root.join("locked.pdf"), b"locked").unwrap();
    lock(&s.root.join("locked.pdf"));
    let root = s.root.to_str().unwrap();

    let stashed = stash(&s, &[root]);
    let output = text(&stashed);
    assert_eq!(stashed.status.code(), Some(3), "{output}");
    assert!(output.contains("Stashed 2 items."), "{output}");
    assert!(
        output.contains("The selected folder is NOT clear."),
        "{output}"
    );
    assert!(
        output.contains("1 locked item was (immutable flag) left in the folder")
            || output.contains("locked item"),
        "{output}"
    );
    assert!(s.root.join("locked.pdf").exists(), "the locked item stayed");
    assert!(!s.root.join("plain.txt").exists());

    let popped = stash(&s, &["pop", root]);
    assert!(popped.status.success(), "{}", text(&popped));
    assert!(s.root.join("plain.txt").exists());
    assert!(s.root.join("other.txt").exists());
    assert!(s.root.join("locked.pdf").exists());
}

#[test]
fn stash_json_reports_locked_items_and_clear_false() {
    let s = scene("json");
    fs::write(s.root.join("plain.txt"), b"plain").unwrap();
    fs::write(s.root.join("locked.pdf"), b"locked").unwrap();
    lock(&s.root.join("locked.pdf"));
    let root = s.root.to_str().unwrap();
    let stashed = stash(&s, &[root, "--json"]);
    let output = text(&stashed);
    assert_eq!(stashed.status.code(), Some(3), "{output}");
    assert!(output.contains("\"clear\":false"), "{output}");
    assert!(output.contains("\"locked_skipped\":1"), "{output}");
    assert!(!output.contains("locked.pdf"), "{output}");
}

#[test]
fn a_folder_holding_only_a_locked_item_is_not_reported_clear() {
    let s = scene("only");
    fs::write(s.root.join("locked.pdf"), b"locked").unwrap();
    lock(&s.root.join("locked.pdf"));
    let stashed = stash(&s, &[s.root.to_str().unwrap()]);
    let output = text(&stashed);
    assert_eq!(stashed.status.code(), Some(1), "{output}");
    assert!(output.contains("NOT clear"), "{output}");
    assert!(!output.contains("already clear"), "{output}");
}
