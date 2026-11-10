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
            // A space and a single quote, so every folder step is quoted.
            root: temp(&format!("{tag}-root it's")),
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        self.run_with(KEY, args)
    }

    fn run_with(&self, key: &str, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_sweep"))
            .env("ETUDE_STATE_DIR", &self.state.0)
            .env("ETUDE_JOURNAL_KEY", key)
            .env("HOME", &self.home.0)
            .args(args)
            .output()
            .unwrap()
    }

    fn root(&self) -> &str {
        self.root.0.to_str().unwrap()
    }

    /// The folder step as sweep prints it: the path in single quotes, with
    /// each single quote written as '\''.
    fn folder_step(&self) -> String {
        format!("sweep undo '{}'", self.root().replace('\'', "'\\''"))
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

    fn apply(&self) -> String {
        self.apply_at(KEY, self.root())
    }

    /// Apply a folder with a key and return the undo record id it prints.
    fn apply_at(&self, key: &str, root: &str) -> String {
        // Records are ordered by modification time; keep two applies apart.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let out = self.run_with(key, &["apply", root, "--yes", "--since", "0"]);
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(out.status.success(), "{text}");
        text.lines()
            .find_map(|line| line.strip_prefix("Undo record id: "))
            .unwrap_or_else(|| panic!("no id in {text}"))
            .trim()
            .to_owned()
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

    /// Run a printed `sweep ...` step through /bin/sh exactly as printed, so
    /// its quoting is read the way a shell reads it. Text is stdout then stderr.
    fn step(&self, step: &str) -> (Option<i32>, String) {
        let bin = Path::new(env!("CARGO_BIN_EXE_sweep")).parent().unwrap();
        let out = Command::new("/bin/sh")
            .arg("-c")
            .arg(step)
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .env("ETUDE_STATE_DIR", &self.state.0)
            .env("ETUDE_JOURNAL_KEY", KEY)
            .env("HOME", &self.home.0)
            .output()
            .unwrap();
        (
            out.status.code(),
            format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ),
        )
    }
}

/// The step at the end of the line that starts with `lead`.
fn step_after(out: &str, lead: &str) -> String {
    out.lines()
        .find(|line| line.starts_with(lead))
        .and_then(|line| line.rsplit(", run: ").next())
        .unwrap_or_else(|| panic!("no line starting {lead:?} in {out}"))
        .to_owned()
}

/// A file somewhere under `root` but not directly in it.
fn moved_file(root: &Path, name: &str) -> PathBuf {
    for entry in std::fs::read_dir(root).unwrap() {
        let dir = entry.unwrap().path();
        if dir.is_dir() && dir.join(name).is_file() {
            return dir.join(name);
        }
    }
    panic!("{name} was not moved into a folder");
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

/// The raw value of one details field: a number or null.
fn field_in(json: &str, key: &str) -> String {
    let tail = json
        .split(&format!("\"{key}\":"))
        .nth(1)
        .unwrap_or_else(|| panic!("no {key} in {json}"));
    tail.chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect()
}

/// Only the files: undo keeps an empty folder it made while unreadable
/// history might still need it.
fn files(items: Vec<(String, Option<Vec<u8>>)>) -> Vec<(String, Option<Vec<u8>>)> {
    items
        .into_iter()
        .filter(|(_, bytes)| bytes.is_some())
        .collect()
}

fn remaining_in(json: &str) -> usize {
    field_in(json, "remaining_reversible").parse().unwrap()
}

const ONE: &str = "Another apply of this folder can still be undone. To undo it, run: sweep undo '";
const MORE: &str = "more applies of this folder have something left to undo. To undo the next one, run: sweep undo '";

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
    assert!(out.contains(&case.folder_step()), "{out}");
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
    assert!(out.contains(&case.folder_step()), "{out}");
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

#[test]
fn a_partial_undo_does_not_send_the_folder_step_elsewhere() {
    let case = Case::new("partial");
    case.add("document", "pdf");
    let first = case.apply();
    case.add("photo", "jpg");
    case.apply();
    std::fs::remove_file(moved_file(&case.root.0, "photo_1.jpg")).unwrap();
    let (code, out) = case.undo(false);
    assert_eq!(code, Some(3), "{out}");
    assert!(out.contains("Restored 2 of 3 files."), "{out}");
    // The folder step comes back to this record, so the line names the other
    // apply only by its id.
    let line = format!(
        "Another apply of this folder can still be undone. To undo it before this one is finished, run: sweep undo --id {first}"
    );
    assert!(out.contains(&line), "{out}");
    assert!(!out.contains(ONE), "{out}");
    let (code, json) = case.undo(true);
    assert_eq!(code, Some(3), "{json}");
    assert_eq!(remaining_in(&json), 1, "{json}");
    // The id step works as printed.
    let (code, text) = case.step(&format!("sweep undo --id {first}"));
    assert_eq!(code, Some(0), "{text}");
    assert!(text.contains("Restored 3 files."), "{text}");
}

const OTHER_KEY: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

#[test]
fn an_unreadable_record_makes_the_count_unknown_and_says_what_undo_does_next() {
    let case = Case::new("unreadable");
    let elsewhere = Case::new("unreadable-b");
    elsewhere.add("document", "pdf");
    // An older record that the current key does not open.
    case.apply_at(OTHER_KEY, elsewhere.root());
    case.add("document", "pdf");
    let before = tree(&case.root.0);
    case.apply();
    let (code, out) = case.undo(false);
    assert_eq!(code, Some(0), "{out}");
    let line = format!(
        "1 undo record could not be opened, so sweep cannot tell whether another apply of this folder remains. If you run {}, it stops at that record and moves nothing.",
        case.folder_step()
    );
    assert!(out.contains(&line), "{out}");
    assert!(!out.contains(ONE), "{out}");
    assert!(
        !out.contains("--id"),
        "no apply of this folder opens: {out}"
    );
    assert_eq!(files(before.clone()), files(tree(&case.root.0)));
    // What the line says the next undo does is what it does.
    let (code, text) = case.step(&case.folder_step());
    assert_ne!(code, Some(0), "{text}");
    assert!(text.contains("Nothing was moved"), "{text}");
    assert_eq!(files(before), files(tree(&case.root.0)));
}

#[test]
fn an_unreadable_record_reports_null_in_json() {
    let case = Case::new("unreadable-json");
    let elsewhere = Case::new("unreadable-json-b");
    elsewhere.add("document", "pdf");
    case.apply_at(OTHER_KEY, elsewhere.root());
    case.add("document", "pdf");
    case.apply();
    let (code, json) = case.undo(true);
    assert_eq!(code, Some(0), "{json}");
    assert_eq!(field_in(&json, "remaining_reversible"), "null", "{json}");
    assert_eq!(field_in(&json, "records_not_opened"), "1", "{json}");
}

#[test]
fn an_unreadable_record_between_two_applies_gives_the_id_step() {
    let case = Case::new("between");
    let elsewhere = Case::new("between-b");
    case.add("document", "pdf");
    let before = tree(&case.root.0);
    let first = case.apply();
    elsewhere.add("document", "pdf");
    case.apply_at(OTHER_KEY, elsewhere.root());
    case.add("photo", "jpg");
    case.apply();
    let (code, out) = case.undo(false);
    assert_eq!(code, Some(0), "{out}");
    assert!(
        out.contains("1 undo record could not be opened, so sweep cannot tell"),
        "{out}"
    );
    assert!(
        !out.contains(ONE),
        "the folder step stops at that record: {out}"
    );
    let step = step_after(&out, "Another apply of this folder can still be undone.");
    assert_eq!(step, format!("sweep undo --id {first}"));
    let (code, text) = case.step(&step);
    assert_eq!(code, Some(0), "{text}");
    assert!(text.contains("Restored 3 files."), "{text}");
    let mut now = files(tree(&case.root.0));
    now.retain(|(name, _)| !name.starts_with("photo_"));
    assert_eq!(files(before), now, "the first apply is undone");
}

#[test]
fn an_apply_with_only_an_empty_folder_left_says_so() {
    let case = Case::new("empty");
    case.add("document", "pdf");
    let before = tree(&case.root.0);
    let first = case.apply();
    case.add("report", "pdf");
    case.apply();
    // Undo the older apply first. Its folder still holds the newer apply's
    // files, so undo keeps it.
    let (code, out) = case.step(&format!("sweep undo --id {first}"));
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains(ONE), "the newer apply comes next: {out}");
    let (code, out) = case.undo(false);
    assert_eq!(code, Some(0), "{out}");
    let line = format!(
        "Another apply of this folder left an empty folder that undo removes. To remove that folder, run: {}",
        case.folder_step()
    );
    assert!(out.contains(&line), "{out}");
    assert!(!out.contains(ONE), "{out}");
    let (code, out) = case.undo(false);
    assert_eq!(code, Some(0), "{out}");
    assert!(!out.contains("Another apply"), "{out}");
    let mut now = tree(&case.root.0);
    now.retain(|(name, _)| !name.starts_with("report_"));
    assert_eq!(before, now, "the empty folder is gone");
}

/// Cut one apply's record back to its first frame, so it has lost every move
/// record while its files sit in the folders it made.
fn cut_record(state: &Path, id: &str) {
    let path = std::fs::read_dir(state)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .find(|path| {
            path.extension().is_some_and(|x| x == "journal")
                && path.file_name().unwrap().to_string_lossy().contains(id)
        })
        .unwrap_or_else(|| panic!("no record for {id}"));
    let raw = std::fs::read(&path).unwrap();
    let base = u32::from_le_bytes(raw[0..4].try_into().unwrap()) as usize;
    std::fs::write(&path, &raw[..4 + base]).unwrap();
}

#[test]
fn a_record_the_next_undo_refuses_is_named_as_refused() {
    let case = Case::new("refused");
    case.add("document", "pdf");
    let first = case.apply();
    cut_record(&case.state.0, &first);
    case.add("photo", "jpg");
    case.apply();
    let (code, out) = case.undo(false);
    assert_eq!(code, Some(0), "{out}");
    let line = format!(
        "Another apply of this folder has files left to put back, but its undo record is missing more than one entry. {} refuses it and moves nothing. It names the folders that hold those files.",
        case.folder_step()
    );
    assert!(out.contains(&line), "{out}");
    assert!(!out.contains(ONE), "{out}");
    let before = tree(&case.root.0);
    // What the line says is what the step does.
    let (code, text) = case.step(&case.folder_step());
    assert_eq!(code, Some(3), "{text}");
    assert!(text.contains("missing more than one entry"), "{text}");
    assert!(text.contains("Documents"), "{text}");
    assert_eq!(before, tree(&case.root.0), "the refused step moved nothing");
}

#[test]
fn an_unopenable_record_older_than_a_live_apply_leaves_the_count_unknown() {
    let case = Case::new("older");
    let elsewhere = Case::new("older-b");
    elsewhere.add("document", "pdf");
    case.apply_at(OTHER_KEY, elsewhere.root());
    case.add("document", "pdf");
    case.apply();
    case.add("photo", "jpg");
    case.apply();
    let (code, out) = case.undo(false);
    assert_eq!(code, Some(0), "{out}");
    // The next folder undo reaches the first apply before the old record.
    assert!(out.contains(ONE), "{out}");
    assert!(out.contains(&case.folder_step()), "{out}");
    assert!(
        out.contains(
            "1 undo record could not be opened, so more applies of this folder may remain."
        ),
        "{out}"
    );
    let (code, json) = case.undo(true);
    assert_eq!(code, Some(0), "{json}");
    assert_eq!(field_in(&json, "remaining_reversible"), "null", "{json}");
    assert_eq!(field_in(&json, "records_not_opened"), "1", "{json}");
}

#[test]
fn more_than_one_unopenable_record_is_counted() {
    let case = Case::new("several");
    let one = Case::new("several-b");
    let two = Case::new("several-c");
    one.add("document", "pdf");
    two.add("document", "pdf");
    case.apply_at(OTHER_KEY, one.root());
    case.apply_at(OTHER_KEY, two.root());
    case.add("document", "pdf");
    case.apply();
    let (code, out) = case.undo(false);
    assert_eq!(code, Some(0), "{out}");
    let line = format!(
        "2 undo records could not be opened, so sweep cannot tell whether another apply of this folder remains. If you run {}, it stops at one of them and moves nothing.",
        case.folder_step()
    );
    assert!(out.contains(&line), "{out}");
    let (code, json) = case.undo(true);
    assert_ne!(
        code,
        Some(0),
        "the folder undo stops at those records: {json}"
    );
}
