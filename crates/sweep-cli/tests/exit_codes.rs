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

    // Screenshot-named files form a structural group; the two same basenames
    // in a/ and b/ collide once grouped under root/Screenshots/. This used to
    // build its group from a shared "notes" token; that rule is gone.
    std::fs::write(root.join("a/Screenshot 2026-07-01 at 9.00.00 AM.png"), b"a").unwrap();
    std::fs::write(root.join("b/Screenshot 2026-07-01 at 9.00.00 AM.png"), b"b").unwrap();
    std::fs::write(root.join("Screenshot 2026-07-02 at 9.00.00 AM.png"), b"1").unwrap();
    std::fs::write(root.join("Screenshot 2026-07-03 at 9.00.00 AM.png"), b"2").unwrap();

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
        "Screenshot 2026-07-01 at 9.00.00 AM.png",
        "Screenshot 2026-07-02 at 9.00.00 AM.png",
        "Screenshot 2026-07-03 at 9.00.00 AM.png",
        "Screenshot 2026-07-04 at 9.00.00 AM.png",
        "Screenshot 2026-07-05 at 9.00.00 AM.png",
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

/// The defect itself, witnessed through the shipped binary rather than the
/// helper: a typo'd flag must not produce output that looks like a scan.
///
/// A review pointed out the unit tests only proved `check_scan_flags` returns
/// an error. That is the implementation. What the bug was actually about is
/// what a person sees on their terminal, which is this.
#[test]
fn a_typod_scan_flag_prints_no_scan_output() {
    let dir = TestDir(unique_temp("typo"));
    std::fs::create_dir_all(&dir.0).expect("mkdir");
    for n in 0..3 {
        std::fs::write(dir.0.join(format!("shot{n}.png")), b"x").expect("write");
    }

    // The real flag scans, so the fixture is known to produce output.
    let good = sweep_bin()
        .arg(&dir.0)
        .arg("--explain")
        .output()
        .expect("run");
    let good_out = String::from_utf8_lossy(&good.stdout);
    assert!(
        good_out.contains("Scanned"),
        "the fixture must produce a scan for the comparison to mean anything"
    );

    for typo in ["--explainn", "--jsonn", "--quite"] {
        let out = sweep_bin().arg(&dir.0).arg(typo).output().expect("run");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            !stdout.contains("Scanned"),
            "{typo} printed scan output; a reader cannot tell they did not get \
             the flag they asked for"
        );
        assert_eq!(
            out.status.code(),
            Some(2),
            "{typo} must exit 2, the refusal code, not a success-shaped one"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("Did you mean"),
            "{typo} should point at the flag that was meant, got: {stderr}"
        );
    }
}

/// The other half of the same contract: refusing typos must not cost anyone a
/// working command. Every invocation here is documented in `sweep help`.
#[test]
fn documented_scan_invocations_still_run() {
    let dir = TestDir(unique_temp("still-works"));
    std::fs::create_dir_all(&dir.0).expect("mkdir");
    for n in 0..3 {
        std::fs::write(dir.0.join(format!("shot{n}.png")), b"x").expect("write");
    }

    let ok: &[&[&str]] = &[
        &[],
        &["--json"],
        &["--quiet"],
        &["--explain"],
        &["--allow-sync"],
        &["--inspect-content"],
        &["--depth", "2"],
        &["--json", "--quiet"],
        &["--explain", "--depth", "3"],
    ];
    for args in ok {
        let out = sweep_bin().arg(&dir.0).args(*args).output().expect("run");
        assert_ne!(
            out.status.code(),
            Some(2),
            "a documented invocation was refused: {args:?} — {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

/// A refusal names the operation that failed, not only the reason.
///
/// The audit that produced this found nine sites forwarding an inner error
/// with `eprintln!("sweep: {e}")`. "io error: Permission denied (os error
/// 13)" is true and leaves the reader guessing whether sweep was reading
/// their folder, writing a journal, or moving a file. rustc never does
/// that: it names the operation, then the reason.
///
/// Triggers the real path rather than a synthetic one -- ScanError::Io comes
/// from canonicalize(), so an unreadable PARENT is what reaches it. An
/// unreadable target directory does not: those are counted and reported,
/// which is issue #4's fix and a different code path entirely.
#[test]
fn a_refusal_names_the_operation_not_only_the_reason() {
    use std::os::unix::fs::PermissionsExt;

    // Restores the mode on drop, including on a panic between the chmod and
    // the assertions. A review caught the first version restoring inline:
    // any panic in between (the spawn, an expect) would have left a 0o000
    // directory behind that TestDir's own cleanup then could not walk, so a
    // single failing assertion would have stranded an unreadable directory
    // in the temp tree.
    struct RestoreMode(std::path::PathBuf);
    impl Drop for RestoreMode {
        fn drop(&mut self) {
            if let Ok(md) = std::fs::metadata(&self.0) {
                let mut perms = md.permissions();
                perms.set_mode(0o700);
                let _ = std::fs::set_permissions(&self.0, perms);
            }
        }
    }

    let root = TestDir(unique_temp("refusal-context"));
    let parent = root.0.join("parent");
    let child = parent.join("child");
    std::fs::create_dir_all(&child).expect("mkdir");
    std::fs::write(child.join("a.png"), b"x").expect("write");

    let mut perms = std::fs::metadata(&parent).expect("stat").permissions();
    perms.set_mode(0o000);
    std::fs::set_permissions(&parent, perms).expect("chmod");
    let _restore = RestoreMode(parent.clone());

    let out = sweep_bin().arg(&child).output().expect("run");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();

    // An empty stderr means the condition could not be created at all --
    // running as root, or a filesystem that ignores the mode bits. A review
    // pointed out the first version returned early here, which cargo counts
    // as a pass: a green test that checked nothing, the exact shape this
    // project's `unproven` category exists to keep out of the pass column.
    // Asserting instead means a host that cannot host this test says so.
    assert!(
        !stderr.is_empty(),
        "could not make a directory unreadable on this host (running as root, \
         or a filesystem ignoring mode bits), so the refusal path was never \
         reached and this test proved nothing"
    );
    assert!(
        stderr.contains("could not read that folder"),
        "the refusal must name what sweep was attempting, got: {stderr}"
    );
    assert!(
        stderr.contains("os error"),
        "and must still carry the OS's own reason, got: {stderr}"
    );
}
