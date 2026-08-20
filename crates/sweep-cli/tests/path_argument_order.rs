//! The path means the same thing on either side of a flag, in every command.
//!
//! Why a CLI test and not a unit test on the finder: the unit version passes
//! while a command that never calls the finder is still wrong. I wrote that
//! unit test first, reverted the fix in `cmd_review`, and it stayed green --
//! it proves the helper is correct, not that anyone uses it. That is the same
//! gap `damaged_journal_cli.rs` was written for.
//!
//! The bug: `sweep --depth 2 ~/Downloads` scanned the CURRENT directory and
//! never looked at the path. Exit 0, no warning, a confident report about a
//! folder the user did not name. `sweep review --depth 2 DIR` took "2" as the
//! path. `apply` alone was right.

use std::path::PathBuf;
use std::process::Command;

struct TestDir(PathBuf);
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn sweep_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_sweep"))
}

/// A folder with a known, small number of files, and a cwd with a different
/// one. If a command scans the cwd instead, the count gives it away.
fn fixture(tag: &str) -> (TestDir, TestDir) {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let target = std::env::temp_dir().join(format!("sweep-pathorder-{tag}-t-{stamp}"));
    let elsewhere = std::env::temp_dir().join(format!("sweep-pathorder-{tag}-e-{stamp}"));
    std::fs::create_dir_all(&target).unwrap();
    std::fs::create_dir_all(&elsewhere).unwrap();
    // Two in the target, seven where the command is run from.
    for i in 0..2 {
        std::fs::write(target.join(format!("named_{i}.pdf")), b"x").unwrap();
    }
    for i in 0..7 {
        std::fs::write(elsewhere.join(format!("other_{i}.pdf")), b"x").unwrap();
    }
    (TestDir(target), TestDir(elsewhere))
}

#[test]
fn a_flag_before_the_path_does_not_send_the_scan_to_the_current_directory() {
    let (target, elsewhere) = fixture("scan");
    for args in [
        vec!["--depth", "2", target.0.to_str().unwrap()],
        vec![target.0.to_str().unwrap(), "--depth", "2"],
    ] {
        let out = sweep_bin()
            .args(&args)
            .current_dir(&elsewhere.0)
            .env("SWEEP_GRACE_SECS", "0")
            .output()
            .expect("run sweep");
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        assert!(
            text.contains("Scanned 2 items"),
            "with args {args:?} sweep reported on the wrong folder: {text}"
        );
    }
}

#[test]
fn review_does_not_mistake_a_flag_value_for_the_path() {
    let (target, elsewhere) = fixture("review");
    // review needs a terminal, so it exits before prompting; what matters is
    // that it did not fail trying to read a folder called "2".
    let out = sweep_bin()
        .args(["review", "--depth", "2", target.0.to_str().unwrap()])
        .current_dir(&elsewhere.0)
        .env("SWEEP_GRACE_SECS", "0")
        .output()
        .expect("run sweep review");
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        !err.contains("could not read that folder"),
        "review took the value of --depth as the path: {err}"
    );
}

#[test]
fn a_bare_command_still_uses_the_current_directory() {
    let (_target, elsewhere) = fixture("bare");
    let out = sweep_bin()
        .current_dir(&elsewhere.0)
        .env("SWEEP_GRACE_SECS", "0")
        .output()
        .expect("run sweep");
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        text.contains("Scanned 7 items"),
        "a bare sweep stopped defaulting to the current directory: {text}"
    );
}
