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
        output.contains("1 item is locked or headed for a locked folder, and stays in place"),
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
    assert_eq!(stashed.status.code(), Some(3), "{output}");
    assert!(output.contains("NOT clear"), "{output}");
    assert!(!output.contains("already clear"), "{output}");
    assert!(output.contains("Nothing was moved"), "{output}");
    assert!(output.contains("chflags nouchg"), "{output}");
    assert!(!output.contains("immutable"), "{output}");

    let json = stash(&s, &[s.root.to_str().unwrap(), "--json"]);
    let output = text(&json);
    assert_eq!(json.status.code(), Some(3), "{output}");
    assert!(output.contains("\"status\":\"refused\""), "{output}");
    assert!(
        output.contains("\"code\":\"permission_denied\""),
        "{output}"
    );
    assert!(output.contains("no_operation_undo_needed"), "{output}");
    assert!(output.contains("\"clear\":false"), "{output}");
    assert!(output.contains("\"locked_skipped\":1"), "{output}");
}

#[test]
fn selecting_only_a_locked_item_prints_no_restore_step() {
    let s = scene("select");
    let file = s.root.join("locked.pdf");
    fs::write(&file, b"locked").unwrap();
    lock(&file);
    let stashed = stash(&s, &["select", file.to_str().unwrap()]);
    let output = text(&stashed);
    assert_eq!(stashed.status.code(), Some(3), "{output}");
    assert!(output.contains("Nothing was moved"), "{output}");
    assert!(!output.contains("stash pop"), "{output}");
    assert!(
        output.contains("1 item is locked and stays in place"),
        "{output}"
    );
    assert!(file.exists());
}

/// A file locked in the holding folder after stash reaches the move denial.
#[test]
fn pop_of_an_item_locked_in_the_holding_folder_says_how_to_unlock_it() {
    let s = scene("pop");
    fs::write(s.root.join("plain.txt"), b"plain").unwrap();
    let root = s.root.to_str().unwrap();
    assert!(stash(&s, &[root]).status.success());
    let holding = fs::read_dir(&s.root)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let held = holding.join("plain.txt");
    lock(&held);

    let popped = stash(&s, &["pop", root]);
    let output = text(&popped);
    assert_eq!(popped.status.code(), Some(3), "{output}");
    assert!(output.contains("1 item is locked"), "{output}");
    assert!(output.contains("chflags nouchg"), "{output}");

    Command::new("chflags")
        .arg("nouchg")
        .arg(&held)
        .status()
        .unwrap();
    assert!(stash(&s, &["pop", root]).status.success());
    assert_eq!(fs::read(s.root.join("plain.txt")).unwrap(), b"plain");
}

/// Run the line a message printed, the way a person pastes it.
fn run_printed(line: &str) -> String {
    let output = Command::new("/bin/sh")
        .arg("-c")
        .arg(line)
        .output()
        .unwrap();
    assert!(output.status.success(), "{line}: {}", text(&output));
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn printed_after<'a>(output: &'a str, words: &str) -> &'a str {
    output
        .split(words)
        .nth(1)
        .unwrap_or_else(|| panic!("no {words:?} in {output}"))
        .lines()
        .next()
        .unwrap()
}

/// `--into` holds nothing yet, so the step must look where the items are, in
/// every folder they came from.
#[test]
fn select_into_prints_a_find_step_for_the_folders_that_hold_the_locked_items() {
    let s = scene("select-into");
    let (one, two, into) = (s.root.join("one"), s.root.join("two"), s.root.join("into"));
    for folder in [&one, &two, &into] {
        fs::create_dir(folder).unwrap();
    }
    let (first, second) = (one.join("first.pdf"), two.join("second.pdf"));
    for file in [&first, &second] {
        fs::write(file, b"locked").unwrap();
        lock(file);
    }
    let plain = one.join("plain.pdf");
    fs::write(&plain, b"plain").unwrap();
    let stashed = stash(
        &s,
        &[
            "select",
            "--into",
            into.to_str().unwrap(),
            plain.to_str().unwrap(),
            first.to_str().unwrap(),
            second.to_str().unwrap(),
        ],
    );
    let output = text(&stashed);
    assert_eq!(stashed.status.code(), Some(3), "{output}");
    let found = run_printed(printed_after(&output, "See which with: "));
    assert!(found.contains("first.pdf"), "{found}");
    assert!(found.contains("second.pdf"), "{found}");
    assert!(!output.contains("first.pdf"), "{output}");
}

#[test]
fn selecting_only_a_locked_item_needs_no_undo() {
    let s = scene("select-mode");
    let file = s.root.join("locked.pdf");
    fs::write(&file, b"locked").unwrap();
    lock(&file);
    let stashed = stash(&s, &["select", file.to_str().unwrap(), "--json"]);
    let output = text(&stashed);
    assert_eq!(stashed.status.code(), Some(3), "{output}");
    assert!(output.contains("no_operation_undo_needed"), "{output}");
    assert!(!output.contains("conditional_journal_restore"), "{output}");
    assert!(output.contains("\"conditions\":[]"), "{output}");
}

/// The printed plan step runs and finishes the job once the lock is cleared.
#[test]
fn a_locked_root_prints_a_stash_command_that_works_after_unlocking() {
    let s = scene("root-step");
    let folder = s.root.join("desk");
    fs::create_dir(&folder).unwrap();
    fs::write(folder.join("plain.txt"), b"plain").unwrap();
    lock(&folder);
    let refused = stash(&s, &[folder.to_str().unwrap()]);
    let output = text(&refused);
    assert_eq!(refused.status.code(), Some(2), "{output}");
    let step = printed_after(&output, "Then plan again with: ");
    assert!(step.starts_with("stash '"), "{step}");
    Command::new("chflags")
        .arg("nouchg")
        .arg(&folder)
        .status()
        .unwrap();
    let again = Command::new("/bin/sh")
        .arg("-c")
        .arg(step.replacen("stash", &format!("'{}'", env!("CARGO_BIN_EXE_stash")), 1))
        .env("ETUDE_STATE_DIR", &s.state)
        .env("ETUDE_JOURNAL_KEY", KEY)
        .output()
        .unwrap();
    assert!(again.status.success(), "{}", text(&again));
    assert!(!folder.join("plain.txt").exists());
}

/// A locked `--into` folder is refused before anything moves, and the same
/// command works once the lock is cleared.
#[test]
fn a_locked_into_folder_is_refused_and_works_after_unlocking() {
    let s = scene("into-step");
    let into = s.root.join("into");
    fs::create_dir(&into).unwrap();
    let file = s.root.join("plain.pdf");
    fs::write(&file, b"plain").unwrap();
    lock(&into);
    let refused = stash(
        &s,
        &[
            "select",
            "--into",
            into.to_str().unwrap(),
            file.to_str().unwrap(),
        ],
    );
    let output = text(&refused);
    assert_ne!(refused.status.code(), Some(0), "{output}");
    assert!(file.exists(), "{output}");
    assert!(!output.contains("panicked"), "{output}");
    assert!(output.contains(into.to_str().unwrap()), "{output}");
    assert!(output.contains("Nothing was moved"), "{output}");
    Command::new("chflags")
        .arg("nouchg")
        .arg(&into)
        .status()
        .unwrap();
    let again = stash(
        &s,
        &[
            "select",
            "--into",
            into.to_str().unwrap(),
            file.to_str().unwrap(),
        ],
    );
    assert!(again.status.success(), "{}", text(&again));
}
