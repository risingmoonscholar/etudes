//! Issue: an unreadable subdirectory is invisible in `sweep`'s own output.
//!
//! Spawns the real `sweep` binary against a tree with a `chmod 000`
//! subdirectory and checks both `--json` and the human-facing `--explain`
//! render for a disclosure that some part of the tree could not be read.
//! "Scanned N items" must never read as complete when it is not. That is
//! the whole issue. This asserts against the actual binary's stdout, not
//! against `etude-core` internals a CLI change could bypass.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

struct TestDir(PathBuf);

impl Drop for TestDir {
    fn drop(&mut self) {
        // A 000 directory can't be removed until its mode is restored.
        let locked = self.0.join("locked");
        let _ = fs::set_permissions(&locked, fs::Permissions::from_mode(0o755));
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn unique_temp(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "sweep-visibility-cli-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn sweep_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_sweep"))
}

/// True if the text discloses, in words a person or an agent can act on,
/// that something could not be read. Mirrors the stress harness's check.
/// Deliberately not a bare "skip" match, since the JSON always has a
/// "skipped" object (for hidden/symlink counts) even at zero.
fn discloses_unreadable(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    [
        "unreadable",
        "permission",
        "could not read",
        "denied",
        "eacces",
        "cannot read",
        "inaccessible",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

#[test]
fn json_reports_a_nonzero_unreadable_count_when_a_subdirectory_is_locked() {
    let root = unique_temp("json-grouped");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("locked")).unwrap();
    for i in 0..5 {
        fs::write(
            root.join("locked")
                .join(format!("secret_{i}_lockedtok.txt")),
            b"x",
        )
        .unwrap();
    }
    for i in 0..5 {
        fs::write(root.join(format!("visible_{i}_lockedtok.txt")), b"x").unwrap();
    }
    fs::set_permissions(root.join("locked"), fs::Permissions::from_mode(0o000)).unwrap();
    let _guard = TestDir(root.clone());

    let out = sweep_bin()
        .arg(&root)
        .args(["--depth", "2", "--json"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();

    assert!(
        stdout.contains("\"scanned\":5") || stdout.contains("\"scanned\": 5"),
        "expected exactly the 5 visible files scanned: {stdout}"
    );
    // discloses_unreadable() alone is not enough here: to_json() always emits
    // the "unreadable" key, including at zero, so a substring match would
    // false-pass a regression that dropped the count back to 0 while leaving
    // the key in place. Pin the actual value.
    assert!(
        stdout.contains("\"unreadable\":1") || stdout.contains("\"unreadable\": 1"),
        "--json's unreadable count is not 1 (or the key is missing): {stdout}"
    );
    assert!(
        discloses_unreadable(&stdout),
        "--json gave no indication that `locked/` could not be read: {stdout}"
    );
}

#[test]
fn human_explain_output_discloses_the_unreadable_directory() {
    let root = unique_temp("human-grouped");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("locked")).unwrap();
    for i in 0..5 {
        fs::write(
            root.join("locked")
                .join(format!("secret_{i}_lockedtok.txt")),
            b"x",
        )
        .unwrap();
    }
    for i in 0..5 {
        fs::write(root.join(format!("visible_{i}_lockedtok.txt")), b"x").unwrap();
    }
    fs::set_permissions(root.join("locked"), fs::Permissions::from_mode(0o000)).unwrap();
    let _guard = TestDir(root.clone());

    let out = sweep_bin()
        .arg(&root)
        .args(["--depth", "2", "--explain"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();

    assert!(
        discloses_unreadable(&stdout),
        "--explain gave no human-readable indication that `locked/` could not be read: {stdout}"
    );
}

#[test]
fn the_no_groups_branch_still_discloses_an_unreadable_directory() {
    // No shared token, no camera burst, no installer set. plan.groups stays
    // empty. run_scan() takes the early "Nothing here needs organising"
    // branch, which is a SEPARATE print site from render(). That branch must
    // not claim completeness either.
    let root = unique_temp("human-empty");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("locked")).unwrap();
    fs::write(root.join("locked").join("secret.txt"), b"x").unwrap();
    fs::write(root.join("lonefile.dat"), b"x").unwrap();
    fs::set_permissions(root.join("locked"), fs::Permissions::from_mode(0o000)).unwrap();
    let _guard = TestDir(root.clone());

    let out = sweep_bin()
        .arg(&root)
        .args(["--depth", "2"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();

    // Either wording is the no-groups branch. It says "Nothing here needs
    // organising" when nothing was held back, and "Nothing else here needs
    // organising" when the grace window kept something -- because claiming a
    // folder is tidy while holding files back would be the same untruth the
    // summary split fixed. What this test is about is the disclosure below.
    assert!(
        stdout.contains("needs organising"),
        "expected the no-groups branch to fire (lonefile.dat forms no group): {stdout}"
    );
    assert!(
        discloses_unreadable(&stdout),
        "the no-groups human branch gave no indication that `locked/` could not be read: {stdout}"
    );
}
