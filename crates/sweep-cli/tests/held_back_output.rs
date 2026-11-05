//! Regression coverage for issue #47: apply and review must name files that
//! stay behind because they are too recent or still being downloaded.
//!
//! These tests spawn the shipped binary and assert its human-facing output.
//! The plan counters were already correct; silence in the CLI was the bug.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::process::Stdio;
use std::process::{Command, Output};
use std::time::{Duration, SystemTime};

struct TestDir(PathBuf);

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn unique_temp(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "sweep-held-back-{tag}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn sweep_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_sweep"))
}

fn write_file(path: &Path) {
    fs::write(path, b"x").unwrap();
}

fn backdate(path: &Path) {
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(2 * 24 * 60 * 60))
        .unwrap();
}

fn fixture(tag: &str, old_pdfs: bool) -> TestDir {
    let root = unique_temp(tag);
    fs::create_dir_all(&root).unwrap();
    for n in 1..=4 {
        let pdf = root.join(format!("document-{n}.pdf"));
        write_file(&pdf);
        if old_pdfs {
            backdate(&pdf);
        }
    }
    write_file(&root.join("fresh.pdf"));
    write_file(&root.join("something.mp4.part"));
    fs::create_dir(root.join("project")).unwrap();
    write_file(&root.join("project/Cargo.toml"));
    TestDir(root)
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn assert_mixed_disclosure(text: &str) {
    assert!(
        text.contains("1 file changed too recently to judge and was left alone"),
        "the fresh file's count and reason must share one output line: {text}"
    );
    assert!(
        text.contains("1 download is still in progress and was left alone"),
        "the in-flight download's count and reason must share one output line: {text}"
    );
    assert!(
        text.contains("1 folder was left alone because it holds a project file"),
        "the guarded project's count and reason must share one output line: {text}"
    );
}

fn assert_all_held_disclosure(text: &str) {
    assert!(
        text.contains("5 files changed too recently to judge and were left alone"),
        "the fresh files' count and reason must share one output line: {text}"
    );
    assert!(
        text.contains("1 download is still in progress and was left alone"),
        "the in-flight download's count and reason must share one output line: {text}"
    );
    assert!(
        text.contains("1 folder was left alone because it holds a project file"),
        "the guarded project's count and reason must share one output line: {text}"
    );
}

#[test]
fn apply_with_moves_discloses_every_held_back_reason() {
    let root = fixture("apply-mixed", true);

    let out = sweep_bin()
        .args(["apply", "--yes", "--no-journal", "--depth", "2"])
        .arg(&root.0)
        .output()
        .unwrap();
    let text = stdout(&out);

    assert_eq!(
        out.status.code(),
        Some(0),
        "mixed apply failed: stdout={text} stderr={}",
        stderr(&out)
    );
    assert!(
        text.contains("Moved 4 files."),
        "fixture did not reproduce: {text}"
    );
    assert_mixed_disclosure(&text);
}

#[test]
fn apply_with_nothing_eligible_explains_what_was_held_back() {
    let root = fixture("apply-empty", false);

    let out = sweep_bin()
        .args(["apply", "--yes", "--no-journal", "--depth", "2"])
        .arg(&root.0)
        .output()
        .unwrap();
    let text = stdout(&out);

    assert_eq!(out.status.code(), Some(1), "expected nothing-to-apply exit");
    assert!(
        stderr(&out).contains("nothing to apply"),
        "the empty apply branch did not run: {}",
        stderr(&out)
    );
    assert_all_held_disclosure(&text);
}

#[test]
fn review_with_only_held_back_files_explains_why_it_cannot_organise_them() {
    let root = fixture("review-empty", false);

    let out = sweep_bin()
        .arg("review")
        .arg(&root.0)
        .args(["--depth", "2"])
        .output()
        .unwrap();
    let text = stdout(&out);

    assert_eq!(out.status.code(), Some(1), "expected no-groups review exit");
    assert_all_held_disclosure(&text);
    assert!(
        text.contains("Nothing else here needs organising"),
        "review falsely claimed the held-back files need no organising: {text}"
    );
}

// Uses the macOS form of script(1) to give review a real terminal.
#[cfg(target_os = "macos")]
#[test]
fn review_with_groups_discloses_held_back_files_before_reviewing() {
    let root = fixture("review-mixed", true);

    // With no terminal, review prints only its refusal: a disclosure there
    // would read as a review starting.
    let out = sweep_bin()
        .arg("review")
        .arg(&root.0)
        .args(["--depth", "2"])
        .output()
        .unwrap();
    assert!(
        stderr(&out).contains("review needs a terminal"),
        "the groups-present review branch did not run: {}",
        stderr(&out)
    );
    assert_eq!(stdout(&out), "", "only the refusal");

    // In a terminal the disclosure comes before the first question. `script`
    // gives review one, and its empty stdin quits at that question.
    let sweep = sweep_bin();
    let mut terminal = Command::new("/usr/bin/script");
    terminal
        .args(["-q", "/dev/null"])
        .arg(sweep.get_program())
        .arg("review")
        .arg(&root.0)
        .args(["--depth", "2"])
        .stdin(Stdio::null());
    for (name, value) in sweep.get_envs() {
        match value {
            Some(value) => terminal.env(name, value),
            None => terminal.env_remove(name),
        };
    }
    let text = stdout(&terminal.output().unwrap());
    assert_mixed_disclosure(&text);
    let question = text.find("[a]ccept").expect("review asked nothing");
    assert!(
        text.find("left alone").unwrap() < question,
        "the disclosure must come before the first question: {text}"
    );
}

fn reason_of(out: &Output) -> (String, String) {
    let text = stdout(out);
    let field = |name: &str| {
        let key = format!("\"{name}\":\"");
        let reason = text.split("\"reason\":").nth(1).expect("a reason");
        let start = reason.find(&key).expect("a reason field") + key.len();
        reason[start..].split('"').next().unwrap().to_string()
    };
    (field("code"), field("retry"))
}

/// Scan and apply both print "Run again later" for this folder, so the JSON
/// reason must promise the same. A hidden file or a link is never a candidate,
/// and nearly every real folder holds a .DS_Store.
#[test]
fn a_folder_held_only_by_recency_says_retry_later_on_scan_and_apply() {
    let root = TestDir(unique_temp("only-recent"));
    fs::create_dir_all(&root.0).unwrap();
    write_file(&root.0.join("photo.jpg"));
    write_file(&root.0.join(".DS_Store"));
    #[cfg(unix)]
    std::os::unix::fs::symlink(root.0.join("photo.jpg"), root.0.join("link.jpg")).unwrap();

    let later = (String::from("nothing_to_do"), String::from("later"));
    let scan = sweep_bin().arg(&root.0).arg("--json").output().unwrap();
    assert_eq!(reason_of(&scan), later, "{}", stdout(&scan));
    let apply = sweep_bin()
        .args(["apply", "--yes", "--json"])
        .arg(&root.0)
        .output()
        .unwrap();
    assert_eq!(reason_of(&apply), later, "{}", stdout(&apply));

    // A download cannot be released by waiting out the grace window alone.
    write_file(&root.0.join("movie.mp4.part"));
    let not_needed = (String::from("nothing_to_do"), String::from("not_needed"));
    let mixed = sweep_bin().arg(&root.0).arg("--json").output().unwrap();
    assert_eq!(reason_of(&mixed), not_needed, "{}", stdout(&mixed));
    let mixed = sweep_bin()
        .args(["apply", "--yes", "--json"])
        .arg(&root.0)
        .output()
        .unwrap();
    assert_eq!(reason_of(&mixed), not_needed, "{}", stdout(&mixed));
}
