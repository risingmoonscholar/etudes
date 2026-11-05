//! Undo through the shared progress layer, and review alongside apply. The
//! schema-1 consumer test for apply stays in `structured_progress.rs`, unedited.
include!("../../etude-cli-support/test-support/progress_probe.rs");

use std::process::{Command, Output, Stdio};

fn sweep(dir: &ProbeDir, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sweep"))
        .args(args)
        .env("ETUDE_STATE_DIR", dir.join("state"))
        .env("ETUDE_JOURNAL_KEY", "22".repeat(32))
        .env("SWEEP_GRACE_SECS", "0")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn fixture(tag: &str, files: usize) -> (ProbeDir, String) {
    let dir = ProbeDir::new(tag);
    let root = dir.join("root");
    std::fs::create_dir_all(&root).unwrap();
    for n in 0..files {
        std::fs::write(root.join(format!("Screenshot {n:04}.png")), b"synthetic").unwrap();
    }
    let root = root.display().to_string();
    (dir, root)
}

fn apply(dir: &ProbeDir, root: &str) {
    let run = sweep(dir, &["apply", root, "--yes", "--since", "0", "--json"]);
    assert_eq!(run.status.code(), Some(0));
}

#[test]
fn undo_reports_entries_examined_and_reversals_made_durable() {
    let (dir, root) = fixture("undo", 250);
    apply(&dir, &root);
    let run = sweep(&dir, &["undo", &root, "--json", "--progress-json"]);
    assert_eq!(run.status.code(), Some(0));
    witness(
        "undo",
        &run.stdout,
        &run.stderr,
        Expect {
            planned: Some(250),
            completed: Some(250),
            journalled: Some(250),
            forbid: &["Screenshot", &root],
            ..Default::default()
        },
    );
    let on_disk = std::fs::read_dir(&root)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_file())
        .count();
    assert_eq!(on_disk, 250);
}

#[test]
fn an_undo_that_leaves_a_changed_file_ends_in_error_and_counts_only_real_reversals() {
    let (dir, root) = fixture("undo-changed", 5);
    apply(&dir, &root);
    std::fs::write(
        std::path::Path::new(&root).join("Screenshots/Screenshot 0002.png"),
        b"changed since apply, and longer",
    )
    .unwrap();
    let run = sweep(&dir, &["undo", &root, "--json", "--progress-json"]);
    witness(
        "undo",
        &run.stdout,
        &run.stderr,
        Expect {
            planned: Some(5),
            completed: Some(5),
            journalled: Some(4),
            ..Default::default()
        },
    );
    let text = String::from_utf8_lossy(&run.stderr);
    assert!(
        text.lines().last().unwrap().contains("\"phase\":\"error\""),
        "{text}"
    );
}

#[test]
fn undo_with_nothing_left_ends_its_stream_with_zero() {
    let (dir, root) = fixture("undo-empty", 3);
    apply(&dir, &root);
    assert_eq!(
        sweep(&dir, &["undo", &root, "--json"]).status.code(),
        Some(0)
    );
    let run = sweep(&dir, &["undo", &root, "--json", "--progress-json"]);
    assert_eq!(run.status.code(), Some(1));
    witness(
        "undo",
        &run.stdout,
        &run.stderr,
        Expect {
            planned: Some(0),
            completed: Some(0),
            journalled: Some(0),
            ..Default::default()
        },
    );
}

#[test]
fn every_undo_with_nothing_to_do_ends_its_stream() {
    let (dir, root) = fixture("undo-nothing", 3);
    let none_on_record = sweep(&dir, &["undo", "--json", "--progress-json"]);
    let unknown_id = sweep(&dir, &["undo", "--id", "nope", "--json", "--progress-json"]);
    apply(&dir, &root);
    let status = sweep(&dir, &["status", &root, "--json"]);
    let text = String::from_utf8_lossy(&status.stdout).into_owned();
    let id = text
        .split("sweep undo --id ")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("status names the undo id")
        .to_owned();
    assert_eq!(sweep(&dir, &["undo", "--id", &id]).status.code(), Some(0));
    let already_undone = sweep(&dir, &["undo", "--id", &id, "--json", "--progress-json"]);
    for (label, run) in [
        ("no apply on record", none_on_record),
        ("unknown id", unknown_id),
        ("already undone", already_undone),
    ] {
        assert_eq!(run.status.code(), Some(1), "{label}");
        let stderr = String::from_utf8_lossy(&run.stderr);
        let records: Vec<_> = stderr
            .lines()
            .filter(|line| line.contains("\"event\":\"progress\""))
            .collect();
        assert_eq!(records.len(), 1, "{label}: {stderr}");
        assert!(
            records[0].contains("\"phase\":\"done\""),
            "{label}: {stderr}"
        );
        assert!(records[0].contains("\"planned\":0"), "{label}: {stderr}");
    }
}

#[test]
fn apply_of_an_exported_plan_keeps_the_apply_operation_name() {
    let (dir, root) = fixture("plan", 3);
    let plan = dir.join("exported.plan");
    let exported = sweep(
        &dir,
        &[
            &root,
            "--since",
            "0",
            "--export-plan",
            plan.to_str().unwrap(),
            "--json",
        ],
    );
    assert_eq!(exported.status.code(), Some(0));
    let digest = String::from_utf8_lossy(&std::fs::read(&plan).unwrap()[..])
        .chars()
        .rev()
        .take(64)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    let run = sweep(
        &dir,
        &[
            "apply",
            "--plan",
            plan.to_str().unwrap(),
            "--plan-digest",
            &digest,
            "--yes",
            "--json",
            "--progress-json",
        ],
    );
    assert_eq!(
        run.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    witness(
        "apply",
        &run.stdout,
        &run.stderr,
        Expect {
            planned: Some(3),
            completed: Some(3),
            journalled: Some(3),
            ..Default::default()
        },
    );
}

/// Review refuses to run without a terminal, so it gets a real one.
#[test]
fn review_reports_through_the_same_layer() {
    let (dir, root) = fixture("review", 3);
    let driver = r#"
import os, pty, subprocess, sys
master, slave = pty.openpty()
child = subprocess.Popen(sys.argv[1:], stdin=slave, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
os.close(slave)
os.write(master, b"a\ny\n")
_, stderr = child.communicate(timeout=60)
sys.stderr.buffer.write(stderr)
sys.exit(child.returncode)
"#;
    let run = Command::new("python3")
        .args(["-I", "-c", driver, env!("CARGO_BIN_EXE_sweep")])
        .args(["review", &root, "--since", "0", "--progress-json"])
        .env("ETUDE_STATE_DIR", dir.join("state"))
        .env("ETUDE_JOURNAL_KEY", "22".repeat(32))
        .env("SWEEP_GRACE_SECS", "0")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(
        run.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    witness(
        "review",
        b"",
        &run.stderr,
        Expect {
            planned: Some(3),
            completed: Some(3),
            journalled: Some(3),
            ..Default::default()
        },
    );
}

#[test]
fn human_undo_milestones_still_appear_without_the_flag_and_stdout_is_unchanged() {
    let (dir, root) = fixture("undo-human", 200);
    apply(&dir, &root);
    let run = sweep(&dir, &["undo", &root]);
    assert_eq!(run.status.code(), Some(0));
    let text = String::from_utf8_lossy(&run.stderr);
    assert_eq!(
        text.lines()
            .filter(|line| line.starts_with("sweep undo: "))
            .count(),
        10,
        "{text}"
    );
    assert!(text.contains("sweep undo: 200/200 items"));
    assert!(!text.contains('{'));
    assert!(!String::from_utf8_lossy(&run.stdout).contains("items\n"));
}

#[test]
fn the_flag_is_refused_where_there_is_no_progress_to_report() {
    let (dir, root) = fixture("refused", 1);
    for args in [
        vec![root.as_str(), "--progress-json"],
        vec!["status", "--progress-json"],
        vec!["forget", "--progress-json"],
    ] {
        let run = sweep(&dir, &args);
        assert_eq!(run.status.code(), Some(2), "{args:?}");
    }
}
