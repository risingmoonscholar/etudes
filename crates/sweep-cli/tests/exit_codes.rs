//! Process-level exit-code contract for the shipped `sweep` binary.
//!
//! The unit tests cover the pure helpers; these spawn the real binary and
//! assert the codes a caller would see.

use std::path::PathBuf;
use std::process::Command;

struct TestDir(PathBuf);

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn unique_temp(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "sweep-exit-{tag}-{}-{}",
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

/// The refusal a machine with no keychain must produce.
///
/// `sweep apply` seals its journal with a key from the login keychain and
/// refuses to write one in the clear. A CI runner has no keychain, and a Linux
/// runner has no `security` binary at all, so the refusal is correct there
/// rather than a failure. A test that asserts apply succeeds is asserting "a
/// keychain exists", which is a property of the machine and not of this tool.
///
/// Both branches assert something. The environment picks which claim is under
/// test, never whether one is.
fn assert_refused_for_want_of_a_keychain(out: &std::process::Output) {
    let msg = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        !out.status.success(),
        "with no keychain, apply must refuse rather than report success: {msg}"
    );
    let lower = msg.to_lowercase();
    assert!(
        lower.contains("keychain") || lower.contains("clear") || lower.contains("journal"),
        "the refusal must say why it refused, got: {msg}"
    );
}

#[test]
fn a_nonexistent_path_exits_3_not_2() {
    let root = unique_temp("missing");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let _root = TestDir(root.clone());
    let state = unique_temp("missing-state");
    let _ = std::fs::remove_dir_all(&state);
    std::fs::create_dir_all(&state).unwrap();
    let _state = TestDir(state.clone());

    let missing = root.join("does-not-exist");
    let out = sweep_bin()
        .env("ETUDE_STATE_DIR", &state)
        .arg(&missing)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(3),
        "missing path must be error 3, not refusal 2: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn a_destination_collision_on_apply_exits_2_not_3() {
    let root = unique_temp("collision");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("a")).unwrap();
    std::fs::create_dir_all(root.join("b")).unwrap();
    let _root = TestDir(root.clone());
    let state = unique_temp("collision-state");
    let _ = std::fs::remove_dir_all(&state);
    std::fs::create_dir_all(&state).unwrap();
    let _state = TestDir(state.clone());

    // Five files share the "notes" token; two same basenames collide once
    // grouped under root/notes/.
    std::fs::write(root.join("a/notes-shared.txt"), b"a").unwrap();
    std::fs::write(root.join("b/notes-shared.txt"), b"b").unwrap();
    std::fs::write(root.join("notes-one.txt"), b"1").unwrap();
    std::fs::write(root.join("notes-two.txt"), b"2").unwrap();
    std::fs::write(root.join("notes-three.txt"), b"3").unwrap();

    let out = sweep_bin()
        .env("ETUDE_STATE_DIR", &state)
        .args(["apply"])
        .arg(&root)
        .args(["--depth", "2", "--yes", "--no-journal"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "destination collision must be refusal 2, not failure 3: stderr={} stdout={}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn an_exhausted_undo_exits_1_not_0() {
    let root = unique_temp("undo");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let _root = TestDir(root.clone());
    let state = unique_temp("undo-state");
    let _ = std::fs::remove_dir_all(&state);
    std::fs::create_dir_all(&state).unwrap();
    let _state = TestDir(state.clone());

    // Five distinct filenames sharing a token. apply succeeds, no collision.
    for name in [
        "notes-one.txt",
        "notes-two.txt",
        "notes-three.txt",
        "notes-four.txt",
        "notes-five.txt",
    ] {
        std::fs::write(root.join(name), name.as_bytes()).unwrap();
    }

    let apply = sweep_bin()
        .env("ETUDE_STATE_DIR", &state)
        .args(["apply"])
        .arg(&root)
        .args(["--yes"])
        .output()
        .unwrap();
    if !apply.status.success() {
        assert_refused_for_want_of_a_keychain(&apply);
        return;
    }

    let first = sweep_bin()
        .env("ETUDE_STATE_DIR", &state)
        .arg("undo")
        .output()
        .unwrap();
    assert!(
        first.status.success(),
        "first undo must restore: stderr={} stdout={}",
        String::from_utf8_lossy(&first.stderr),
        String::from_utf8_lossy(&first.stdout)
    );

    let second = sweep_bin()
        .env("ETUDE_STATE_DIR", &state)
        .arg("undo")
        .output()
        .unwrap();
    assert_eq!(
        second.status.code(),
        Some(1),
        "exhausted undo must exit 1, not 0: stderr={} stdout={}",
        String::from_utf8_lossy(&second.stderr),
        String::from_utf8_lossy(&second.stdout)
    );
}
// appended to crates/sweep-cli/tests/exit_codes.rs
#[test]
fn undo_does_not_imply_it_reversed_an_unjournalled_apply() {
    // With a stale restored journal lying around — the normal state after
    // anyone has used undo once — an apply run with --no-journal leaves files
    // moved and no record. `sweep undo` then finds the OLD journal, sees it is
    // already restored, and says so.
    //
    // The exit code is right: nothing to undo, so 1. The sentence is not. A
    // user who just ran apply reads "this journal was already restored" as a
    // statement about that apply, and it is a statement about an operation
    // they may not remember running. The reassurance is real and the subject
    // is wrong, which is this project's least favourite shape.
    let root = unique_temp("undo-msg-root");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let _root = TestDir(root.clone());
    let state = unique_temp("undo-msg-state");
    let _ = std::fs::remove_dir_all(&state);
    std::fs::create_dir_all(&state).unwrap();
    let _state = TestDir(state.clone());

    for i in 1..=4 {
        std::fs::write(
            root.join(format!("Screenshot 2026-01-0{i} at 9.0{i}.11 AM.png")),
            b"x",
        )
        .unwrap();
    }

    let apply = sweep_bin()
        .env("ETUDE_STATE_DIR", &state)
        .args(["apply"])
        .arg(&root)
        .args(["--yes"])
        .output()
        .unwrap();
    if !apply.status.success() {
        assert_refused_for_want_of_a_keychain(&apply);
        return;
    }
    let _ = sweep_bin()
        .env("ETUDE_STATE_DIR", &state)
        .args(["undo"])
        .output()
        .unwrap();

    // Now an apply with no record at all.
    let _ = sweep_bin()
        .env("ETUDE_STATE_DIR", &state)
        .args(["apply"])
        .arg(&root)
        .args(["--yes", "--no-journal"])
        .output()
        .unwrap();

    let undo = sweep_bin()
        .env("ETUDE_STATE_DIR", &state)
        .args(["undo"])
        .output()
        .unwrap();
    let msg =
        String::from_utf8_lossy(&undo.stdout).to_string() + &String::from_utf8_lossy(&undo.stderr);

    assert_eq!(
        undo.status.code(),
        Some(1),
        "nothing to undo is exit 1: {msg}"
    );
    assert!(
        msg.contains("--no-journal") || msg.contains("no record"),
        "undo must say that an unrecorded apply cannot be reversed, rather than \
         reporting on an older journal as if it were the recent one. got: {msg}"
    );
}

#[test]
fn lesson_lists_steps_and_refuses_ones_that_do_not_exist() {
    let list = sweep_bin().args(["lesson"]).output().unwrap();
    assert_eq!(list.status.code(), Some(0), "bare `lesson` lists the steps");
    let listing = String::from_utf8_lossy(&list.stdout).to_string();

    // The listing states a count. Every step it claims must actually print.
    let claimed: usize = listing
        .split_whitespace()
        .find_map(|w| w.parse::<usize>().ok())
        .expect("the listing states how many exercises there are");

    for n in 1..=claimed {
        let out = sweep_bin()
            .args(["lesson", &n.to_string()])
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(0),
            "lesson {n} is listed but does not print"
        );
        let body = String::from_utf8_lossy(&out.stdout);
        assert!(
            body.contains(&format!("{n}/{claimed}")),
            "lesson {n} does not say where it is in the sequence"
        );
        assert!(
            body.trim().lines().count() > 2,
            "lesson {n} printed a heading and nothing else"
        );
    }

    // And a step that does not exist is refused rather than silently empty.
    for bad in ["0", &(claimed + 1).to_string(), "abc", "-1"] {
        let out = sweep_bin().args(["lesson", bad]).output().unwrap();
        assert_eq!(
            out.status.code(),
            Some(2),
            "lesson {bad:?} must be refused with exit 2, not accepted or ignored"
        );
    }
}

#[test]
fn every_command_the_lesson_teaches_exists() {
    // The lesson is a claim about the tool. If it teaches a flag or subcommand
    // that `sweep help` does not have, the lesson is wrong and a learner finds
    // out by being confused rather than by anything failing.
    let help =
        String::from_utf8_lossy(&sweep_bin().arg("help").output().unwrap().stdout).to_string();

    let list = sweep_bin().args(["lesson"]).output().unwrap();
    let claimed: usize = String::from_utf8_lossy(&list.stdout)
        .split_whitespace()
        .find_map(|w| w.parse::<usize>().ok())
        .unwrap();

    for n in 1..=claimed {
        let out = sweep_bin()
            .args(["lesson", &n.to_string()])
            .output()
            .unwrap();
        let body = String::from_utf8_lossy(&out.stdout).to_string();
        for word in body.split_whitespace() {
            let flag = word.trim_matches(|c: char| !c.is_ascii_graphic() || c == '`' || c == ',');
            if let Some(f) = flag.strip_prefix("--") {
                let f = f.trim_end_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-');
                if f.is_empty() {
                    continue;
                }
                assert!(
                    help.contains(&format!("--{f}")),
                    "lesson {n} teaches `--{f}`, which is not in `sweep help`"
                );
            }
        }
    }
}

#[test]
fn the_lesson_never_prints_a_bare_double_hyphen() {
    // In a lesson about a command-line tool, `--` on its own reads as a flag
    // or as the end-of-options separator. It was being used as a dash.
    let list = sweep_bin().args(["lesson"]).output().unwrap();
    let claimed: usize = String::from_utf8_lossy(&list.stdout)
        .split_whitespace()
        .find_map(|w| w.parse::<usize>().ok())
        .unwrap();

    for n in 1..=claimed {
        let out = sweep_bin()
            .args(["lesson", &n.to_string()])
            .output()
            .unwrap();
        let body = String::from_utf8_lossy(&out.stdout).to_string();
        for word in body.split_whitespace() {
            assert_ne!(
                word, "--",
                "lesson {n} prints a bare `--`, which reads as a flag rather than punctuation"
            );
        }
    }
}

#[test]
fn undo_can_reach_an_apply_that_is_not_the_most_recent() {
    // Issue #8. `sweep undo` took no path and always reversed the newest
    // journal, so applying to two folders left the first one unreachable: its
    // journal stayed on disk for its full retention as an index of the user's
    // filenames, with no remaining way to use it. The exposure without the
    // benefit.
    //
    // `stash pop` already takes an optional path for exactly this reason.
    let a = unique_temp("undo-two-a");
    let b = unique_temp("undo-two-b");
    let state = unique_temp("undo-two-state");
    for d in [&a, &b, &state] {
        let _ = std::fs::remove_dir_all(d);
        std::fs::create_dir_all(d).unwrap();
    }
    let (_a, _b, _s) = (
        TestDir(a.clone()),
        TestDir(b.clone()),
        TestDir(state.clone()),
    );

    for dir in [&a, &b] {
        for i in 1..=4 {
            std::fs::write(
                dir.join(format!("Screenshot 2026-01-0{i} at 9.0{i}.11 AM.png")),
                b"x",
            )
            .unwrap();
        }
    }

    let apply = |dir: &std::path::Path| {
        sweep_bin()
            .env("ETUDE_STATE_DIR", &state)
            .args(["apply"])
            .arg(dir)
            .args(["--yes"])
            .output()
            .unwrap()
    };

    let first = apply(&a);
    if !first.status.success() {
        assert_refused_for_want_of_a_keychain(&first);
        return;
    }
    assert!(apply(&b).status.success(), "second apply");

    // Undo the newest, which is B.
    let undo_b = sweep_bin()
        .env("ETUDE_STATE_DIR", &state)
        .args(["undo"])
        .output()
        .unwrap();
    assert_eq!(undo_b.status.code(), Some(0), "undo of the newest apply");

    // A is still applied. Name it, and it must come back.
    let undo_a = sweep_bin()
        .env("ETUDE_STATE_DIR", &state)
        .args(["undo"])
        .arg(&a)
        .output()
        .unwrap();
    let msg = String::from_utf8_lossy(&undo_a.stdout).to_string()
        + &String::from_utf8_lossy(&undo_a.stderr);
    assert_eq!(
        undo_a.status.code(),
        Some(0),
        "naming an earlier apply must reach it: {msg}"
    );
    assert_eq!(
        std::fs::read_dir(&a)
            .unwrap()
            .filter(|e| e.as_ref().unwrap().path().is_file())
            .count(),
        4,
        "every file should be back at the root of A: {msg}"
    );
}
