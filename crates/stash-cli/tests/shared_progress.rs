//! Stash, select, pop and restore through the shared progress layer.
include!("../../etude-cli-support/test-support/progress_probe.rs");

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn stash(dir: &ProbeDir, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_stash"))
        .args(args)
        .env("ETUDE_STATE_DIR", dir.join("state"))
        .env("ETUDE_JOURNAL_KEY", "33".repeat(32))
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn fixture(tag: &str, files: usize) -> (ProbeDir, PathBuf) {
    let dir = ProbeDir::new(tag);
    let root = dir.join("root");
    std::fs::create_dir_all(&root).unwrap();
    for n in 0..files {
        std::fs::write(root.join(format!("item {n:04}.txt")), b"synthetic").unwrap();
    }
    (dir, root)
}

fn stash_id(run: &Output) -> String {
    let script = r#"
import json, sys
print(json.load(sys.stdin)["recovery"]["operation"])
"#;
    let mut parser = Command::new("python3")
        .args(["-I", "-c", script])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    std::io::Write::write_all(parser.stdin.as_mut().unwrap(), &run.stdout).unwrap();
    let output = parser.wait_with_output().unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn files_in(root: &Path) -> usize {
    std::fs::read_dir(root)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("item"))
        .count()
}

#[test]
fn stash_then_pop_report_counts_the_folder_confirms() {
    let (dir, root) = fixture("round", 250);
    let root_text = root.to_str().unwrap();
    let held = stash(&dir, &[root_text, "--json", "--progress-json"]);
    assert_eq!(held.status.code(), Some(0));
    witness(
        "stash",
        &held.stdout,
        &held.stderr,
        Expect {
            planned: Some(250),
            completed: Some(250),
            journalled: Some(250),
            forbid: &["item 0", root_text],
            ..Default::default()
        },
    );
    assert_eq!(files_in(&root), 0);
    let id = stash_id(&held);
    let popped = stash(&dir, &["pop", "--id", &id, "--json", "--progress-json"]);
    assert_eq!(popped.status.code(), Some(0));
    witness(
        "pop",
        &popped.stdout,
        &popped.stderr,
        Expect {
            planned: Some(250),
            completed: Some(250),
            journalled: Some(250),
            forbid: &["item 0", root_text],
            ..Default::default()
        },
    );
    assert_eq!(files_in(&root), 250);
}

#[test]
fn restore_is_the_same_operation_as_pop() {
    let (dir, root) = fixture("restore", 3);
    let held = stash(&dir, &[root.to_str().unwrap(), "--json"]);
    let id = stash_id(&held);
    let restored = stash(&dir, &["restore", "--id", &id, "--json", "--progress-json"]);
    assert_eq!(restored.status.code(), Some(0));
    witness(
        "pop",
        &restored.stdout,
        &restored.stderr,
        Expect {
            planned: Some(3),
            completed: Some(3),
            journalled: Some(3),
            ..Default::default()
        },
    );
}

#[test]
fn restoring_one_item_plans_one() {
    let (dir, root) = fixture("item", 3);
    let held = stash(&dir, &[root.to_str().unwrap(), "--json"]);
    let id = stash_id(&held);
    let inspect = stash(&dir, &["inspect", "--id", &id, "--json"]);
    let text = String::from_utf8_lossy(&inspect.stdout).to_string();
    let item = text
        .split('"')
        .find(|part| part.starts_with("item-"))
        .expect("inspect names item ids")
        .to_string();
    let run = stash(
        &dir,
        &[
            "pop",
            "--id",
            &id,
            "--item",
            &item,
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
        "pop",
        &run.stdout,
        &run.stderr,
        Expect {
            planned: Some(1),
            completed: Some(1),
            journalled: Some(1),
            ..Default::default()
        },
    );
    assert_eq!(files_in(&root), 1);
    // The same item again has nothing to do, and that still ends the stream.
    let again = stash(
        &dir,
        &[
            "pop",
            "--id",
            &id,
            "--item",
            &item,
            "--json",
            "--progress-json",
        ],
    );
    assert_eq!(again.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&again.stderr);
    assert!(stderr.contains("\"phase\":\"done\""), "{stderr}");
}

#[test]
fn explicit_selection_reports_like_any_stash() {
    let (dir, root) = fixture("select", 4);
    let parent = dir.join("holding");
    std::fs::create_dir_all(&parent).unwrap();
    let first = root.join("item 0000.txt");
    let second = root.join("item 0001.txt");
    let run = stash(
        &dir,
        &[
            "select",
            "--into",
            parent.to_str().unwrap(),
            "--json",
            "--progress-json",
            first.to_str().unwrap(),
            second.to_str().unwrap(),
        ],
    );
    assert_eq!(
        run.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    witness(
        "stash",
        &run.stdout,
        &run.stderr,
        Expect {
            planned: Some(2),
            completed: Some(2),
            journalled: Some(2),
            ..Default::default()
        },
    );
}

#[test]
fn no_journal_stash_moves_and_records_nothing_durable() {
    let (dir, root) = fixture("nojournal", 3);
    let run = stash(
        &dir,
        &[
            root.to_str().unwrap(),
            "--no-journal",
            "--json",
            "--progress-json",
        ],
    );
    witness(
        "stash",
        &run.stdout,
        &run.stderr,
        Expect {
            planned: Some(3),
            completed: Some(3),
            journalled: Some(0),
            ..Default::default()
        },
    );
}

#[test]
fn nothing_to_stash_and_nothing_to_pop_end_with_zero() {
    let (dir, root) = fixture("empty", 0);
    let run = stash(&dir, &[root.to_str().unwrap(), "--json", "--progress-json"]);
    assert_eq!(run.status.code(), Some(1));
    witness(
        "stash",
        &run.stdout,
        &run.stderr,
        Expect {
            planned: Some(0),
            completed: Some(0),
            ..Default::default()
        },
    );
    let run = stash(
        &dir,
        &["pop", root.to_str().unwrap(), "--json", "--progress-json"],
    );
    assert_eq!(run.status.code(), Some(1));
    witness(
        "pop",
        &run.stdout,
        &run.stderr,
        Expect {
            planned: Some(0),
            completed: Some(0),
            ..Default::default()
        },
    );
}

#[test]
fn a_changed_item_turns_a_pop_terminal_into_an_error() {
    let (dir, root) = fixture("changed", 3);
    let held = stash(&dir, &[root.to_str().unwrap(), "--json"]);
    let id = stash_id(&held);
    // Change one held file in place.
    let holding = std::fs::read_dir(&root)
        .unwrap()
        .filter_map(Result::ok)
        .find(|entry| entry.path().is_dir())
        .unwrap()
        .path();
    let member = std::fs::read_dir(&holding)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::write(member, b"changed while held, and longer").unwrap();
    let run = stash(&dir, &["pop", "--id", &id, "--json", "--progress-json"]);
    assert_eq!(run.status.code(), Some(3));
    witness(
        "pop",
        &run.stdout,
        &run.stderr,
        Expect {
            planned: Some(3),
            completed: Some(3),
            journalled: Some(2),
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
fn human_milestones_are_what_they_were_without_the_flag() {
    let (dir, root) = fixture("human", 200);
    let run = stash(&dir, &[root.to_str().unwrap()]);
    let text = String::from_utf8_lossy(&run.stderr);
    assert_eq!(
        text.lines().filter(|l| l.starts_with("stash: ")).count(),
        10,
        "{text}"
    );
    assert!(text.contains("stash: 200/200 items"));
    assert!(!text.contains('{'));
}

#[test]
fn the_flag_is_refused_where_there_is_no_progress_to_report() {
    let (dir, _root) = fixture("refused", 1);
    for args in [
        vec!["status", "--progress-json"],
        vec!["inspect", "--progress-json"],
    ] {
        let run = stash(&dir, &args);
        assert_eq!(run.status.code(), Some(2), "{args:?}");
    }
}
