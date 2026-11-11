//! Undo records past 30 days are dropped only by a run that already holds the
//! key for its own work (apply or undo), with that key. A scan reads no key and
//! drops nothing. A record the key cannot open is kept.
//!
//! Every run here gets a throwaway state folder, a fake HOME and a supplied
//! key, so the login keychain is never reached. The key reader leaves its mark
//! in the result's observations as the `key_material` category; that is the
//! seam these tests read.

use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::{Duration, SystemTime};

#[path = "../../etude-cli-support/test-support/test_tool.rs"]
mod test_tool;
use test_tool::keyed_tool;

const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const OTHER_KEY: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

struct TestDir(PathBuf);
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp(tag: &str) -> TestDir {
    let dir = std::env::temp_dir().join(format!(
        "sweep-expiry-{tag}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(dir.join("home")).unwrap();
    std::fs::create_dir_all(dir.join("state")).unwrap();
    TestDir(dir)
}

fn run(base: &Path, key: &str, args: &[&str]) -> Output {
    keyed_tool(env!("CARGO_BIN_EXE_sweep"))
        .env("HOME", base.join("home"))
        .env("ETUDE_STATE_DIR", base.join("state"))
        .env("ETUDE_JOURNAL_KEY", key)
        .env("SWEEP_GRACE_SECS", "0")
        .args(args)
        .output()
        .unwrap()
}

/// A folder of three screenshots, which sweep groups and moves.
fn folder(base: &Path, name: &str) -> String {
    let dir = base.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    for i in 1..=3 {
        std::fs::write(
            dir.join(format!("Screenshot 2024-01-01 at 00.00.0{i}.png")),
            format!("{name}-{i}"),
        )
        .unwrap();
    }
    dir.to_str().unwrap().to_owned()
}

fn apply(base: &Path, key: &str, dir: &str) -> Output {
    let out = run(
        base,
        key,
        &["apply", dir, "--yes", "--since", "0", "--json"],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn records(base: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(base.join("state"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "journal"))
        .collect();
    found.sort();
    found
}

fn age(record: &Path) {
    let old = SystemTime::now() - Duration::from_secs(40 * 24 * 60 * 60);
    std::fs::File::options()
        .write(true)
        .open(record)
        .unwrap()
        .set_modified(old)
        .unwrap();
}

fn read_key_material(out: &Output) -> bool {
    String::from_utf8_lossy(&out.stdout).contains("\"category\":\"key_material\"")
}

#[test]
fn a_scan_reads_no_key_and_drops_no_old_record() {
    let base = temp("scan");
    let dir = folder(&base.0, "a");
    let applied = apply(&base.0, KEY, &dir);
    // The seam is live: a run that does use the key says so.
    assert!(
        read_key_material(&applied),
        "apply did not report its key read"
    );
    let old = records(&base.0);
    assert_eq!(old.len(), 1);
    age(&old[0]);

    let empty = base.0.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let scan = run(&base.0, KEY, &[empty.to_str().unwrap(), "--json"]);
    assert_eq!(scan.status.code(), Some(1), "an empty folder needs nothing");
    assert!(!read_key_material(&scan), "a scan read the journal key");
    assert!(
        old[0].exists(),
        "a scan, which holds no key, dropped a record"
    );
    let status = run(&base.0, KEY, &["status", &dir, "--json"]);
    assert!(status.status.success());
    assert!(old[0].exists(), "status dropped a record");
}

#[test]
fn an_apply_drops_an_old_record_it_can_open_and_keeps_its_own() {
    let base = temp("apply");
    apply(&base.0, KEY, &folder(&base.0, "a"));
    let old = records(&base.0);
    age(&old[0]);

    let out = apply(&base.0, KEY, &folder(&base.0, "b"));
    assert!(
        !old[0].exists(),
        "an old record that opens survived an apply"
    );
    let now = records(&base.0);
    assert_eq!(now.len(), 1, "the apply's own record is missing");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("dropped 1 undo record older than 30 days")
    );
}

#[test]
fn an_undo_drops_old_records_after_it_has_undone() {
    let base = temp("undo");
    apply(&base.0, KEY, &folder(&base.0, "a"));
    let first = records(&base.0);
    let b = folder(&base.0, "b");
    apply(&base.0, KEY, &b);
    let target: Vec<PathBuf> = records(&base.0)
        .into_iter()
        .filter(|p| !first.contains(p))
        .collect();
    age(&first[0]);
    // The record being undone is old too: the undo must still find it.
    age(&target[0]);
    std::thread::sleep(Duration::from_millis(5));
    let out = run(&base.0, KEY, &["undo", &b]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read_dir(&b).unwrap().count(),
        3,
        "undo did not restore"
    );
    assert!(!first[0].exists(), "an old record survived the undo run");
    assert!(
        target[0].exists(),
        "undo's own record, rewritten now, was dropped"
    );
}

#[test]
fn an_old_record_another_key_cannot_open_is_kept() {
    let base = temp("otherkey");
    apply(&base.0, OTHER_KEY, &folder(&base.0, "a"));
    let old = records(&base.0);
    age(&old[0]);

    apply(&base.0, KEY, &folder(&base.0, "b"));
    assert!(
        old[0].exists(),
        "an apply dropped a record it could not open"
    );
    let undo = run(&base.0, KEY, &["undo", "--id", &id(&old[0])]);
    assert_eq!(undo.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&undo.stderr).contains("Keep that record"));
    assert!(old[0].exists(), "the record undo said to keep is gone");
}

fn id(record: &Path) -> String {
    let name = record.file_stem().unwrap().to_str().unwrap();
    name.strip_prefix("sweep-").unwrap().to_owned()
}

/// No tool removes a stash record it cannot open, so forget, which stops for
/// stash records, must say what they are and how a person removes one.
#[test]
fn forget_names_the_stash_records_it_keeps_and_how_to_remove_one() {
    let base = temp("forget");
    let record = base.0.join("state").join("stash-unopenable.journal");
    std::fs::write(&record, b"sealed under a key nobody has").unwrap();
    let out = keyed_tool(env!("CARGO_BIN_EXE_sweep"))
        .env("HOME", base.0.join("home"))
        .env("ETUDE_STATE_DIR", base.0.join("state"))
        .env("ETUDE_JOURNAL_KEY", KEY)
        .arg("forget")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "a non-terminal forget refuses");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("1 stash record is in"), "{err}");
    assert!(err.contains("they may still restore files"), "{err}");
    assert!(err.contains("stash status --all"), "{err}");
    assert!(err.contains("stash pop --id ID"), "{err}");
    assert!(err.contains(&format!("rm '{}'", record.display())), "{err}");
    assert!(record.exists(), "forget removed a stash record");
}

/// `--yes` skips the question, not the explanation. With a supplied key forget
/// destroys no key, and says so in the explanation.
#[test]
fn forget_yes_still_names_the_stash_records_it_keeps() {
    let base = temp("forget-yes");
    let record = base.0.join("state").join("stash-kept.journal");
    std::fs::write(&record, b"sealed under a key nobody has").unwrap();
    let sweep_record = base.0.join("state").join("sweep-gone.journal");
    std::fs::write(&sweep_record, b"sealed").unwrap();
    let out = keyed_tool(env!("CARGO_BIN_EXE_sweep"))
        .env("HOME", base.0.join("home"))
        .env("ETUDE_STATE_DIR", base.0.join("state"))
        .env("ETUDE_JOURNAL_KEY", KEY)
        .args(["forget", "--yes"])
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("1 stash record is in"), "{err}");
    assert!(err.contains("says cannot be opened"), "{err}");
    assert!(err.contains("which forget cannot destroy"), "{err}");
    assert!(err.contains(&format!("rm '{}'", record.display())), "{err}");
    assert!(
        !err.contains("--yes destroys the keychain key"),
        "a supplied key is not destroyed: {err}"
    );
    assert!(record.exists(), "forget removed a stash record");
    assert!(!sweep_record.exists(), "forget kept a sweep record");
}
