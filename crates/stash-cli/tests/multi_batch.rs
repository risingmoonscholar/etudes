use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "stash-multi-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("tree")).unwrap();
        fs::create_dir_all(root.join("holding")).unwrap();
        Self(root)
    }
    fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join("tree").join(name);
        fs::write(&path, bytes).unwrap();
        path
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stash"));
        command
            .env("ETUDE_STATE_DIR", self.0.join("state"))
            .env("ETUDE_JOURNAL_KEY", "56".repeat(32))
            .current_dir(self.0.join("tree"));
        command
    }
    fn stash(&self, duration: Option<&str>) -> String {
        let mut command = self.command();
        command.arg(self.0.join("tree")).arg("--json");
        if let Some(duration) = duration {
            command.args(["--for", duration]);
        }
        id(&command.output().unwrap())
    }
    fn select(&self, path: &Path) -> String {
        id(&self
            .command()
            .arg("select")
            .arg(path)
            .arg("--into")
            .arg(self.0.join("holding"))
            .arg("--json")
            .output()
            .unwrap())
    }
    fn pop(&self, id: &str) -> Output {
        self.command()
            .args(["pop", "--id", id, "--json"])
            .output()
            .unwrap()
    }
    fn journals(&self) -> Vec<PathBuf> {
        fs::read_dir(self.0.join("state"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "journal"))
            .collect()
    }
    fn snapshot(&self) -> String {
        let out=Command::new("python3").args(["-c","import pathlib,hashlib,sys,json; p=pathlib.Path(sys.argv[1]); print(json.dumps([(str(x.relative_to(p)),hashlib.sha256(x.read_bytes()).hexdigest()) for x in sorted(p.rglob('*')) if x.is_file()]))"]).arg(&self.0).output().unwrap();
        assert!(out.status.success());
        String::from_utf8(out.stdout).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn json_query(output: &Output, expression: &str) -> String {
    let mut child = Command::new("python3")
        .args([
            "-c",
            "import json,sys; value=json.load(sys.stdin); print(eval(sys.argv[1]))",
            expression,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&output.stdout)
        .unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    String::from_utf8(result.stdout).unwrap().trim().into()
}
fn id(output: &Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let id = json_query(output, "value['details']['stash_id']");
    assert_ne!(id, "None");
    assert_ne!(id, json_query(output, "value['operation_id']"));
    id
}
fn success(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
#[test]
fn independent_same_folder_batches_restore_exactly_one() {
    let f = Fixture::new();
    let first = f.file("first.txt", b"first synthetic");
    let a = f.stash(None);
    let second = f.file("second.txt", b"second synthetic");
    let b = f.stash(None);
    assert_ne!(a, b);
    assert!(!first.exists() && !second.exists());
    success(&f.pop(&a));
    assert_eq!(fs::read(&first).unwrap(), b"first synthetic");
    assert!(!second.exists());
    success(&f.pop(&b));
    assert_eq!(fs::read(&second).unwrap(), b"second synthetic");
    assert_eq!(f.pop(&a).status.code(), Some(1));
}
#[test]
fn path_pop_refuses_ambiguous_batches_without_mutation() {
    let f = Fixture::new();
    f.file("one.txt", b"one");
    f.stash(None);
    f.file("two.txt", b"two");
    f.stash(None);
    let before = f.snapshot();
    let out = f.command().args(["pop", "--json"]).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("multiple live batches"));
    assert_eq!(before, f.snapshot());
}
#[test]
fn explicit_missing_and_malformed_ids_never_fall_back() {
    let f = Fixture::new();
    f.file("one.txt", b"one");
    f.stash(None);
    let before = f.snapshot();
    assert_eq!(f.pop("missing-id").status.code(), Some(1));
    for bad in ["../elsewhere", "/absolute", "bad/id", ""] {
        assert_eq!(f.pop(bad).status.code(), Some(2));
    }
    assert_eq!(before, f.snapshot());
}
#[test]
fn latest_uses_creation_order_not_mutated_journal_mtime() {
    let f = Fixture::new();
    let first = f.file("first.txt", b"first");
    let a = f.stash(None);
    std::thread::sleep(std::time::Duration::from_millis(2));
    let second = f.file("second.txt", b"second");
    let b = f.stash(None);
    let oldest = f.0.join("state").join(format!("stash-{a}.journal"));
    let out = Command::new("python3")
        .args([
            "-c",
            "import os,sys,time; os.utime(sys.argv[1],(time.time()+1000,time.time()+1000))",
        ])
        .arg(oldest)
        .output()
        .unwrap();
    assert!(out.status.success());
    success(
        &f.command()
            .args(["pop", "--latest", "--json"])
            .output()
            .unwrap(),
    );
    assert!(!first.exists());
    assert_eq!(fs::read(&second).unwrap(), b"second");
    assert_eq!(f.pop(&b).status.code(), Some(1));
    success(
        &f.command()
            .args(["pop", "--latest", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(fs::read(&first).unwrap(), b"first");
}
#[test]
fn deadline_and_status_are_information_until_explicit_pop() {
    let f = Fixture::new();
    let source = f.file("due.txt", b"due synthetic");
    let batch = f.stash(Some("1d"));
    let before = f.snapshot();
    let status = f
        .command()
        .args(["status", "--all", "--json"])
        .output()
        .unwrap();
    success(&status);
    assert_eq!(
        json_query(&status, "value['details']['stashes'][0]['id']"),
        batch
    );
    assert_eq!(
        json_query(&status, "value['details']['stashes'][0]['stashed']"),
        "1"
    );
    assert_eq!(
        json_query(&status, "value['details']['stashes'][0]['overdue']"),
        "False"
    );
    assert_eq!(
        f.command()
            .args(["pop", "--id", &batch, "--if-due", "--json"])
            .output()
            .unwrap()
            .status
            .code(),
        Some(2)
    );
    assert_eq!(before, f.snapshot());
    assert!(!source.exists());
    success(&f.pop(&batch));
    assert_eq!(fs::read(&source).unwrap(), b"due synthetic");
}
#[test]
fn selected_multi_parent_operations_keep_independent_ids() {
    let f = Fixture::new();
    let one = f.file("one.txt", b"one");
    let two = f.file("two.txt", b"two");
    let a = f.select(&one);
    let b = f.select(&two);
    success(&f.pop(&a));
    assert_eq!(fs::read(&one).unwrap(), b"one");
    assert!(!two.exists());
    success(&f.command().args(["pop", &b, "--json"]).output().unwrap());
    assert_eq!(fs::read(&two).unwrap(), b"two");
}
#[test]
fn swapped_authenticated_journal_refuses_before_any_restore() {
    let f = Fixture::new();
    let one = f.file("one.txt", b"one");
    let a = f.stash(None);
    let two = f.file("two.txt", b"two");
    let b = f.stash(None);
    fs::copy(
        f.0.join("state").join(format!("stash-{a}.journal")),
        f.0.join("state").join(format!("stash-{b}.journal")),
    )
    .unwrap();
    let before = f.snapshot();
    let output = f.pop(&b);
    assert_eq!(output.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&output.stderr).contains("identity"));
    assert_eq!(before, f.snapshot());
    assert!(!one.exists() && !two.exists());
}
#[test]
fn latest_refuses_unreadable_history_and_exact_id_still_works() {
    let f = Fixture::new();
    let source = f.file("one.txt", b"one");
    let a = f.stash(None);
    fs::write(
        f.0.join("state").join("stash-damaged.journal"),
        b"synthetic damaged journal",
    )
    .unwrap();
    let before = f.snapshot();
    assert_eq!(
        f.command()
            .args(["pop", "--latest", "--json"])
            .output()
            .unwrap()
            .status
            .code(),
        Some(3)
    );
    assert_eq!(before, f.snapshot());
    success(&f.pop(&a));
    assert_eq!(fs::read(source).unwrap(), b"one");
}
#[test]
fn status_has_one_row_per_batch_and_default_paths_are_redacted() {
    let f = Fixture::new();
    f.file("one.txt", b"one");
    let a = f.stash(None);
    f.file("two.txt", b"two");
    let b = f.stash(None);
    let before = f.snapshot();
    let output = f
        .command()
        .args(["status", "--all", "--json"])
        .output()
        .unwrap();
    success(&output);
    assert_eq!(json_query(&output, "len(value['details']['stashes'])"), "2");
    let ids = json_query(
        &output,
        "sorted(row['id'] for row in value['details']['stashes'])",
    );
    assert!(ids.contains(&a) && ids.contains(&b));
    assert!(!String::from_utf8_lossy(&output.stdout).contains(f.0.to_str().unwrap()));
    assert_eq!(before, f.snapshot());
    assert_eq!(f.journals().len(), 2);
}
fn rebind(f: &Fixture, old: &str, new: &str, legacy: bool) {
    let path = f.0.join("state").join(format!("stash-{old}.journal"));
    let raw = fs::read(&path).unwrap();
    let n = u32::from_le_bytes(raw[..4].try_into().unwrap()) as usize;
    let text = String::from_utf8(etude_keep::open(&[0x56; 32], &raw[4..4 + n]).unwrap()).unwrap();
    let text = if legacy {
        text.lines()
            .filter(|line| !line.starts_with("identity\t"))
            .collect::<Vec<_>>()
            .join("\n")
            .replacen("sweep-journal 2", "sweep-journal 1", 1)
            + "\n"
    } else {
        text.replace(
            &format!("identity\tstash\t{old}"),
            &format!("identity\tstash\t{new}"),
        )
    };
    let sealed = etude_keep::seal(&[0x56; 32], text.as_bytes()).unwrap();
    let mut output = (sealed.len() as u32).to_le_bytes().to_vec();
    output.extend(sealed);
    output.extend_from_slice(&raw[4 + n..]);
    fs::write(
        f.0.join("state").join(format!("stash-{new}.journal")),
        output,
    )
    .unwrap();
    fs::remove_file(path).unwrap();
}
fn equal_mtimes(f: &Fixture) {
    let out=Command::new("python3").args(["-c","import pathlib,os,sys; [os.utime(p,ns=(1800000000000000000,1800000000000000000)) for p in pathlib.Path(sys.argv[1]).glob('*.journal')]"]).arg(f.0.join("state")).output().unwrap();
    assert!(out.status.success());
}
#[test]
fn legacy_latest_retains_mtime_order_and_exact_legacy_id() {
    let f = Fixture::new();
    let one = f.file("one.txt", b"one");
    let a = f.stash(None);
    rebind(&f, &a, "legacy-a", true);
    std::thread::sleep(std::time::Duration::from_millis(2));
    let two = f.file("two.txt", b"two");
    let b = f.stash(None);
    rebind(&f, &b, "legacy-b", true);
    success(
        &f.command()
            .args(["pop", "--latest", "--json"])
            .output()
            .unwrap(),
    );
    assert!(!one.exists());
    assert_eq!(fs::read(two).unwrap(), b"two");
    success(&f.pop("legacy-a"));
    assert_eq!(fs::read(one).unwrap(), b"one");
}
#[test]
fn legacy_mtime_ties_refuse_latest_but_not_exact_id() {
    let f = Fixture::new();
    let one = f.file("one.txt", b"one");
    let a = f.stash(None);
    rebind(&f, &a, "legacy-a", true);
    f.file("two.txt", b"two");
    let b = f.stash(None);
    rebind(&f, &b, "legacy-b", true);
    equal_mtimes(&f);
    let before = f.snapshot();
    assert_eq!(
        f.command()
            .args(["pop", "--latest", "--json"])
            .output()
            .unwrap()
            .status
            .code(),
        Some(3)
    );
    assert_eq!(before, f.snapshot());
    success(&f.pop("legacy-a"));
    assert_eq!(fs::read(one).unwrap(), b"one");
}
#[test]
fn equal_creation_time_refuses_latest_without_mutation() {
    let f = Fixture::new();
    f.file("one.txt", b"one");
    let a = f.stash(None);
    f.file("two.txt", b"two");
    let b = f.stash(None);
    let mut parts = b.split('-').map(str::to_string).collect::<Vec<_>>();
    parts[0] = a.split('-').next().unwrap().into();
    let tied = parts.join("-");
    rebind(&f, &b, &tied, false);
    let before = f.snapshot();
    assert_eq!(
        f.command()
            .args(["pop", "--latest", "--json"])
            .output()
            .unwrap()
            .status
            .code(),
        Some(3)
    );
    assert_eq!(before, f.snapshot());
}
#[test]
fn exact_id_wrong_key_refuses_without_fallback_or_mutation() {
    let f = Fixture::new();
    f.file("one.txt", b"one");
    let batch = f.stash(None);
    let before = f.snapshot();
    let out = f
        .command()
        .env("ETUDE_JOURNAL_KEY", "78".repeat(32))
        .args(["pop", "--id", &batch, "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(before, f.snapshot());
    success(&f.pop(&batch));
}
#[test]
fn due_can_pass_without_mutating_until_explicit_pop() {
    let f = Fixture::new();
    let source = f.file("due.txt", b"due");
    let batch = f.stash(Some("0m"));
    let before = f.snapshot();
    for args in [vec!["status", "--all", "--json"], vec!["status", "--json"]] {
        let output = f.command().args(args).output().unwrap();
        success(&output);
        assert_eq!(before, f.snapshot());
    }
    success(
        &f.command()
            .args(["pop", "--id", &batch, "--if-due", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(fs::read(source).unwrap(), b"due");
}
