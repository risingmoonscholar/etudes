//! Each apply of a folder is its own operation with its own undo. After one
//! undo, sweep says when another apply of the same folder can still be undone,
//! so one undo does not read as if the folder were back as it was.
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

struct TestDir(PathBuf);
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp(tag: &str) -> TestDir {
    let dir = std::env::temp_dir().join(format!(
        "sweep-undo-remaining-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    TestDir(dir.canonicalize().unwrap())
}

struct Case {
    state: TestDir,
    home: TestDir,
    root: TestDir,
}

impl Case {
    fn new(tag: &str) -> Self {
        Case {
            state: temp(&format!("{tag}-state")),
            home: temp(&format!("{tag}-home")),
            root: temp(&format!("{tag}-root")),
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_sweep"))
            .env("ETUDE_STATE_DIR", &self.state.0)
            .env("ETUDE_JOURNAL_KEY", KEY)
            .env("HOME", &self.home.0)
            .args(args)
            .output()
            .unwrap()
    }

    fn root(&self) -> &str {
        self.root.0.to_str().unwrap()
    }

    /// Write three old files of one kind, so a plain apply groups them.
    fn add(&self, stem: &str, ext: &str) {
        for i in 1..=3 {
            let path = self.root.0.join(format!("{stem}_{i}.{ext}"));
            std::fs::write(&path, format!("{stem}-{i}")).unwrap();
            let old =
                std::time::SystemTime::now() - std::time::Duration::from_secs(3 * 24 * 60 * 60);
            std::fs::File::open(&path)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(old))
                .unwrap();
        }
    }

    fn apply(&self) {
        // Records are ordered by modification time; keep two applies apart.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let out = self.run(&["apply", self.root(), "--yes", "--since", "0"]);
        assert!(
            out.status.success(),
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn undo(&self, json: bool) -> (Option<i32>, String) {
        let mut args = vec!["undo", self.root()];
        if json {
            args.push("--json");
        }
        let out = self.run(&args);
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
        )
    }

    fn undo_newest(&self) -> (Option<i32>, String) {
        let out = self.run(&["undo"]);
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
        )
    }
}

fn tree(root: &Path) -> Vec<(String, Option<Vec<u8>>)> {
    let mut items = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path.strip_prefix(root).unwrap().display().to_string();
            if path.is_dir() {
                stack.push(path.clone());
                items.push((name, None));
            } else {
                items.push((name, Some(std::fs::read(&path).unwrap())));
            }
        }
    }
    items.sort();
    items
}

fn remaining_in(json: &str) -> usize {
    let tail = json
        .split("\"remaining_reversible\":")
        .nth(1)
        .unwrap_or_else(|| panic!("no remaining_reversible in {json}"));
    tail.chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .unwrap()
}

const ONE: &str = "Another apply of this folder can still be undone. To undo it, run: sweep undo '";
const MORE: &str =
    "more applies of this folder can still be undone. Run this once for each: sweep undo '";

#[test]
fn one_apply_says_nothing_remains() {
    let case = Case::new("one");
    case.add("document", "pdf");
    let before = tree(&case.root.0);
    case.apply();
    let (code, out) = case.undo(false);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("Restored 3 files."), "{out}");
    assert!(!out.contains("can still be undone"), "{out}");
    assert_eq!(before, tree(&case.root.0));
}

#[test]
fn two_applies_of_one_folder_say_the_second_remains() {
    let case = Case::new("two");
    case.add("document", "pdf");
    case.apply();
    case.add("report", "pdf");
    let before_second = tree(&case.root.0);
    case.apply();
    // The folder is back as it was only after two undos. The first says so.
    let (code, out) = case.undo(false);
    assert_eq!(code, Some(0), "{out}");
    let restored = out.find("Restored 3 files.").expect(&out);
    let line = out.find(ONE).unwrap_or_else(|| panic!("{out}"));
    assert!(line > restored, "the line follows the count: {out}");
    assert!(
        out.contains(&format!("sweep undo '{}'", case.root())),
        "{out}"
    );
    assert_eq!(before_second, tree(&case.root.0));

    let (code, out) = case.undo(false);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("Restored 3 files."), "{out}");
    assert!(!out.contains("can still be undone"), "{out}");
    let (code, _) = case.undo(false);
    assert_eq!(code, Some(1));
}

#[test]
fn three_applies_give_the_count_in_text_and_json() {
    let case = Case::new("three");
    case.add("document", "pdf");
    case.apply();
    case.add("photo", "jpg");
    case.apply();
    case.add("report", "pdf");
    case.apply();
    let (code, json) = case.undo(true);
    assert_eq!(code, Some(0), "{json}");
    assert_eq!(remaining_in(&json), 2, "{json}");
    let (code, out) = case.undo(false);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains(ONE), "{out}");
    let (code, json) = case.undo(true);
    assert_eq!(code, Some(0), "{json}");
    assert_eq!(remaining_in(&json), 0, "{json}");
}

#[test]
fn two_remaining_are_counted_in_text() {
    let case = Case::new("count");
    case.add("document", "pdf");
    case.apply();
    case.add("photo", "jpg");
    case.apply();
    case.add("report", "pdf");
    case.apply();
    let (code, out) = case.undo(false);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains(&format!("2 {MORE}")), "{out}");
}

#[test]
fn undo_with_no_folder_says_what_remains_for_the_folder_it_undid() {
    let case = Case::new("newest");
    case.add("document", "pdf");
    case.apply();
    case.add("report", "pdf");
    case.apply();
    let (code, out) = case.undo_newest();
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains(ONE), "{out}");
    assert!(
        out.contains(&format!("sweep undo '{}'", case.root())),
        "{out}"
    );
    let (code, out) = case.undo_newest();
    assert_eq!(code, Some(0), "{out}");
    assert!(!out.contains("can still be undone"), "{out}");
}

#[test]
fn an_apply_of_another_folder_is_not_counted() {
    let case = Case::new("other");
    let elsewhere = Case::new("other-b");
    case.add("document", "pdf");
    case.apply();
    // The same state folder, a different root.
    elsewhere.add("document", "pdf");
    std::thread::sleep(std::time::Duration::from_millis(20));
    let out = case.run(&["apply", elsewhere.root(), "--yes", "--since", "0"]);
    assert!(out.status.success());
    let (code, json) = case.undo(true);
    assert_eq!(code, Some(0), "{json}");
    assert_eq!(remaining_in(&json), 0, "{json}");
}
