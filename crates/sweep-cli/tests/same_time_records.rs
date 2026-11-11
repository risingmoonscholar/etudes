//! Two undo records with the same modification time. sweep cannot tell which
//! one is newer, so plain `sweep undo` and `sweep undo FOLDER` refuse and move
//! nothing. The refusal names each record and gives a step that opens only it,
//! and those steps work while the times are still the same. `sweep status`
//! still lists the records, each tie marked, because status is where a person
//! finds the ids. An entry that is not a regular file is not called a tie.
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[path = "../../etude-cli-support/test-support/test_tool.rs"]
mod test_tool;
use test_tool::{keyed_tool, with_test_key};

const KEY: &str = "5a3c9e0f1b2d4c6e8a0b1c2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f7a8b9c0d1e2f";
/// 2027-01-15, a time no apply in this test writes by itself.
const TIED_SECS: u64 = 1_800_000_000;

struct TestDir(PathBuf);
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp(tag: &str) -> TestDir {
    // Tests run in parallel and can share a tag and a clock reading, so a
    // process-wide counter keeps every folder its own.
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "sweep-same-time-{tag}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    TestDir(dir.canonicalize().unwrap())
}

struct Case {
    state: TestDir,
    home: TestDir,
    first: TestDir,
    second: TestDir,
}

impl Case {
    fn new(tag: &str) -> Self {
        Case {
            state: temp(&format!("{tag}-state")),
            home: temp(&format!("{tag}-home")),
            first: temp(&format!("{tag}-first")),
            // A space and a quote, so a folder step must be quoted to work.
            second: temp(&format!("{tag}-second it's")),
        }
    }

    fn command(&self) -> Command {
        let mut command = keyed_tool(env!("CARGO_BIN_EXE_sweep"));
        command
            .env("ETUDE_STATE_DIR", &self.state.0)
            .env("ETUDE_JOURNAL_KEY", KEY)
            .env("HOME", &self.home.0)
            .env_remove("SWEEP_GRACE_SECS");
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }

    /// Run a printed step exactly as a person pastes it. Only this build's
    /// folder is on PATH, so no installed sweep can answer in its place.
    fn shell(&self, step: &str) -> Output {
        let bin = Path::new(env!("CARGO_BIN_EXE_sweep")).parent().unwrap();
        let mut shell = Command::new("/bin/sh");
        with_test_key(&mut shell)
            .env("ETUDE_STATE_DIR", &self.state.0)
            .env("ETUDE_JOURNAL_KEY", KEY)
            .env("HOME", &self.home.0)
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .env_remove("SWEEP_GRACE_SECS")
            .args(["-c", step])
            .output()
            .unwrap_or_else(|error| panic!("{step}: {error}"))
    }

    /// Three old files of one kind, applied: one undo record. Its id.
    fn apply(&self, root: &Path) -> String {
        self.apply_kind(root, "report", "pdf")
    }

    /// Three old files named STEM_N.EXT, applied: one undo record. Its id.
    fn apply_kind(&self, root: &Path, stem: &str, ext: &str) -> String {
        for i in 1..=3 {
            let path = root.join(format!("{stem}_{i}.{ext}"));
            std::fs::write(&path, format!("{stem} {i}")).unwrap();
            let old =
                std::time::SystemTime::now() - std::time::Duration::from_secs(3 * 24 * 60 * 60);
            std::fs::File::open(&path)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(old))
                .unwrap();
        }
        let out = self.run(&["apply", root.to_str().unwrap(), "--yes", "--since", "0"]);
        let text = text(&out);
        assert!(out.status.success(), "{text}");
        text.lines()
            .find_map(|line| line.strip_prefix("Undo record id: "))
            .unwrap_or_else(|| panic!("no id in {text}"))
            .trim()
            .to_owned()
    }

    fn record(&self, id: &str) -> PathBuf {
        self.state.0.join(format!("sweep-{id}.journal"))
    }

    fn set_time(&self, id: &str, secs: u64) {
        std::fs::File::options()
            .write(true)
            .open(self.record(id))
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs)),
            )
            .unwrap();
    }

    fn files_home(&self, root: &Path) -> usize {
        self.kind_home(root, "report", "pdf")
    }

    fn kind_home(&self, root: &Path, stem: &str, ext: &str) -> usize {
        (1..=3)
            .filter(|i| root.join(format!("{stem}_{i}.{ext}")).is_file())
            .count()
    }

    /// Two applied folders whose records carry the same time, sorted ids.
    fn tied() -> (Self, Vec<String>) {
        let case = Case::new("tie");
        let mut ids = vec![case.apply(&case.first.0), case.apply(&case.second.0)];
        for id in &ids {
            case.set_time(id, TIED_SECS);
        }
        ids.sort();
        (case, ids)
    }
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn json(out: &Output) -> serde_like::Value {
    serde_like::parse(&String::from_utf8_lossy(&out.stdout))
}

/// A small reader for the result envelope, so this test needs no new crate.
mod serde_like {
    use std::process::{Command, Stdio};

    pub struct Value(String);

    pub fn parse(text: &str) -> Value {
        Value(text.to_owned())
    }

    impl Value {
        /// Evaluate a Python expression over the parsed value `v`.
        pub fn get(&self, expression: &str) -> String {
            let mut child = Command::new("python3")
                .args([
                    "-c",
                    "import json,sys; v=json.load(sys.stdin); print(json.dumps(eval(sys.argv[1])))",
                    expression,
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            use std::io::Write;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(self.0.as_bytes())
                .unwrap();
            let out = child.wait_with_output().unwrap();
            assert!(out.status.success(), "{}", self.0);
            String::from_utf8(out.stdout).unwrap().trim().to_owned()
        }
    }
}

#[test]
fn a_tie_names_each_record_moves_nothing_and_has_its_own_code() {
    let (case, ids) = Case::tied();
    let expected = format!(
        "sweep: 2 undo records that can still be undone have the same time, so sweep cannot tell which one is newer. Nothing was moved.\n\
         To undo one of them, run: sweep undo --id {a}\n\
         Or run: sweep undo --id {b}\n\
         To see the folder of a record, run: sweep status --id {a}\n",
        a = ids[0],
        b = ids[1]
    );
    let out = case.run(&["undo"]);
    assert_eq!(out.status.code(), Some(3), "{}", text(&out));
    assert_eq!(String::from_utf8_lossy(&out.stderr), expected);
    assert!(!text(&out).contains("fix this one"));
    assert_eq!(case.files_home(&case.first.0), 0);
    assert_eq!(case.files_home(&case.second.0), 0);

    let out = case.run(&["undo", "--json"]);
    assert_eq!(out.status.code(), Some(3), "{}", text(&out));
    let value = json(&out);
    assert_eq!(value.get("v['status']"), r#""refused""#);
    assert_eq!(
        value.get("v['reason']"),
        r#"{"code": "journal_times_tied", "retry": "after_change", "subject": {"kind": "journal"}}"#
    );
    assert_eq!(
        value.get("[r['operation_id'] for r in v['details']['same_time_records']]"),
        format!(r#"["{}", "{}"]"#, ids[0], ids[1])
    );
    assert_eq!(
        value.get("[r['command'] for r in v['details']['same_time_records']]"),
        format!(
            r#"["sweep undo --id {}", "sweep undo --id {}"]"#,
            ids[0], ids[1]
        )
    );
}

/// `sweep undo FOLDER` chooses only among that folder's records, so a record
/// of another folder with the same time does not stop it.
#[test]
fn a_folder_undo_is_not_stopped_by_another_folders_record() {
    let (case, _) = Case::tied();
    let out = case.run(&["undo", case.first.0.to_str().unwrap(), "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert_eq!(json(&out).get("v['reason']['code']"), r#""done""#);
    assert_eq!(case.files_home(&case.first.0), 3);
    assert_eq!(case.files_home(&case.second.0), 0);
}

/// Two applies of one folder with the same time: the folder undo and the plain
/// undo both stop, name only that folder's records, and each step works.
#[test]
fn a_same_folder_tie_names_that_folders_records_and_each_step_works() {
    let case = Case::new("same-folder");
    let root = case.first.0.clone();
    let mut ids = vec![
        case.apply_kind(&root, "report", "pdf"),
        case.apply_kind(&root, "photo", "jpg"),
    ];
    let other = case.apply(&case.second.0);
    case.set_time(&other, TIED_SECS - 10);
    for id in &ids {
        case.set_time(id, TIED_SECS);
    }
    ids.sort();
    let out = case.run(&["undo", root.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(3), "{}", text(&out));
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert_eq!(
        stderr,
        format!(
            "sweep: 2 undo records of this folder that can still be undone have the same time, so sweep cannot tell which one is newer. Nothing was moved.\n\
             To undo one of them, run: sweep undo --id {a}\n\
             Or run: sweep undo --id {b}\n\
             To see the folder of a record, run: sweep status --id {a}\n",
            a = ids[0],
            b = ids[1]
        )
    );
    assert!(!stderr.contains(&other), "{stderr}");
    let value = json(&case.run(&["undo", root.to_str().unwrap(), "--json"]));
    assert_eq!(
        value.get("[r['operation_id'] for r in v['details']['same_time_records']]"),
        format!(r#"["{}", "{}"]"#, ids[0], ids[1])
    );
    let steps: Vec<String> = stderr
        .lines()
        .filter_map(|line| {
            line.split_once("run: sweep undo")
                .map(|(_, rest)| format!("sweep undo{rest}"))
        })
        .collect();
    assert_eq!(steps.len(), 2, "{stderr}");
    for step in &steps {
        for id in &ids {
            case.set_time(id, TIED_SECS);
        }
        let out = case.shell(step);
        assert!(out.status.success(), "{step}: {}", text(&out));
        assert!(
            text(&out).contains("Restored 3 files."),
            "{step}: {}",
            text(&out)
        );
    }
    assert_eq!(case.kind_home(&root, "report", "pdf"), 3);
    assert_eq!(case.kind_home(&root, "photo", "jpg"), 3);
    assert_eq!(case.files_home(&case.second.0), 0);
}

/// Finished records never block a choice. Both tied records finished: plain
/// undo reaches the older live apply. One finished and one live: plain undo
/// takes the live one. Neither is refused.
#[test]
fn finished_records_with_the_same_time_do_not_block_undo() {
    let case = Case::new("finished");
    let third = temp("finished-third");
    let older = case.apply(&third.0);
    let a = case.apply(&case.first.0);
    let b = case.apply(&case.second.0);
    for id in [&a, &b] {
        let out = case.run(&["undo", "--id", id]);
        assert!(out.status.success(), "{}", text(&out));
    }
    case.set_time(&older, TIED_SECS - 10);
    case.set_time(&a, TIED_SECS);
    case.set_time(&b, TIED_SECS);
    let out = case.run(&["undo", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let value = json(&out);
    assert_eq!(value.get("v['reason']['code']"), r#""done""#);
    assert_eq!(
        value.get("v['recovery']['operation']"),
        format!(r#""{older}""#)
    );
    assert_eq!(case.files_home(&third.0), 3);

    // Every record finished: nothing to undo, not a tie.
    case.set_time(&older, TIED_SECS);
    case.set_time(&a, TIED_SECS);
    case.set_time(&b, TIED_SECS);
    let out = case.run(&["undo"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(text(&out).contains("Nothing to undo"), "{}", text(&out));
    assert!(!text(&out).contains("same time"), "{}", text(&out));

    // One finished, one live, same time: the live one is undone.
    let live = case.apply(&case.first.0);
    case.set_time(&live, TIED_SECS + 5);
    case.set_time(&b, TIED_SECS + 5);
    let out = case.run(&["undo", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let value = json(&out);
    assert_eq!(
        value.get("v['recovery']['operation']"),
        format!(r#""{live}""#)
    );
    assert!(!value.get("v['details']").contains("same_time_records"));
    assert_eq!(case.files_home(&case.first.0), 3);
}

/// Every printed step works when pasted, while the times are still the same.
#[test]
fn each_printed_step_works_through_the_shell_while_the_times_tie() {
    let (case, ids) = Case::tied();
    let out = case.run(&["undo"]);
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let steps: Vec<String> = stderr
        .lines()
        .filter_map(|line| line.split_once("run: ").map(|(_, step)| step.to_owned()))
        .collect();
    assert_eq!(steps.len(), 3, "{stderr}");

    // The status step names the folder of that record.
    let status = case.shell(&steps[2]);
    assert!(status.status.success(), "{}", text(&status));
    assert!(text(&status).contains(&ids[0]), "{}", text(&status));

    for (step, id) in steps[..2].iter().zip(&ids) {
        // Each undo rewrites its record, so tie the times again first.
        for id in &ids {
            case.set_time(id, TIED_SECS);
        }
        let before = (
            case.files_home(&case.first.0),
            case.files_home(&case.second.0),
        );
        let out = case.shell(step);
        assert!(out.status.success(), "{step}: {}", text(&out));
        assert!(
            text(&out).contains("Restored 3 files."),
            "{step}: {}",
            text(&out)
        );
        let after = (
            case.files_home(&case.first.0),
            case.files_home(&case.second.0),
        );
        assert_eq!(after.0 + after.1, before.0 + before.1 + 3, "{step}");
        assert!(step.ends_with(id.as_str()));
    }
    assert_eq!(case.files_home(&case.first.0), 3);
    assert_eq!(case.files_home(&case.second.0), 3);
}

/// Status chooses nothing, so it lists tied records, each marked, and
/// `status --id` reaches one of them.
#[test]
fn status_lists_tied_records_with_their_ids() {
    let (case, ids) = Case::tied();
    let first = case.first.0.to_str().unwrap();
    let out = case.run(&["status", first]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let shown = text(&out);
    let mine = shown
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().next())
        .unwrap()
        .to_owned();
    let other = ids.iter().find(|id| **id != mine).unwrap();
    assert!(
        shown.contains(&format!(
            "  This record has the same time as {other}. sweep cannot tell which one is newer. Its --id step still works."
        )),
        "{shown}"
    );
    let value = json(&case.run(&["status", first, "--json"]));
    assert_eq!(value.get("v['status']"), r#""done""#);
    assert_eq!(
        value.get("v['details']['same_time_as']"),
        format!(r#"["{other}"]"#)
    );
    for id in &ids {
        let out = case.run(&["status", "--id", id, "--json"]);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out));
        assert_eq!(
            json(&out).get("v['details']['operation_id']"),
            format!(r#""{id}""#)
        );
    }
    // Once the times differ, no row carries the field.
    case.set_time(&ids[0], TIED_SECS + 1);
    let value = json(&case.run(&["status", first, "--json"]));
    assert_eq!(value.get("'same_time_as' in v['details']"), "false");
}

/// Without a tie, plain undo still takes the record written last, by its
/// time, whatever the ids say.
#[test]
fn without_a_tie_plain_undo_still_takes_the_newest_time() {
    let case = Case::new("order");
    let first = case.apply(&case.first.0);
    let second = case.apply(&case.second.0);
    // The record applied first is now the newer one, by one second.
    case.set_time(&second, TIED_SECS);
    case.set_time(&first, TIED_SECS + 1);
    let out = case.run(&["undo"]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(case.files_home(&case.first.0), 3);
    assert_eq!(case.files_home(&case.second.0), 0);
}

/// A folder named like a record is not a tie: its own words, its own step,
/// and the step works.
#[test]
fn an_entry_that_is_not_a_file_gets_its_own_message_and_step() {
    let case = Case::new("odd");
    case.apply(&case.first.0);
    let odd = case.state.0.join("sweep-odd.journal");
    std::fs::create_dir(&odd).unwrap();
    let out = case.run(&["undo"]);
    assert_eq!(out.status.code(), Some(3), "{}", text(&out));
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        stderr.contains("sweep-odd.journal in it is not a regular file, so it is not an undo record. Nothing was moved."),
        "{stderr}"
    );
    assert!(!stderr.contains("same time"), "{stderr}");
    assert!(!stderr.contains("fix this one"), "{stderr}");
    let value = json(&case.run(&["undo", "--json"]));
    assert_eq!(value.get("v['reason']['code']"), r#""journal_unreadable""#);
    let step = stderr
        .lines()
        .find_map(|line| {
            line.split_once("reads it: ")
                .map(|(_, step)| step.to_owned())
        })
        .unwrap_or_else(|| panic!("no step in {stderr}"));
    assert!(step.starts_with("mv -i "), "{step}");
    let moved = case.shell(&step);
    assert!(moved.status.success(), "{step}: {}", text(&moved));
    assert!(!odd.exists());
    let out = case.run(&["undo"]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(case.files_home(&case.first.0), 3);
}
