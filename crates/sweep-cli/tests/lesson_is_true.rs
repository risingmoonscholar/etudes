//! The lesson is a promise about what the tool does. This walks it against the
//! real binary so it cannot quietly stop being true.
//!
//! It already had: steps 5 and 6 taught that undo reaches exactly one apply
//! back and then refuses, which was true when they were written and stopped
//! being true when `sweep undo [PATH]` landed. Nothing caught it, because
//! nothing ran the lesson.

use std::path::PathBuf;
use std::process::Command;

struct TestDir(PathBuf);

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn unique(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "sweep-lesson-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn sweep(state: &std::path::Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_sweep"));
    c.env("XDG_STATE_HOME", state);
    c
}

/// Every command the lesson prints must be one the tool accepts. A lesson that
/// teaches a refused invocation is worse than no lesson.
#[test]
fn every_command_the_lesson_prints_is_accepted() {
    let root = TestDir(unique("cmds"));
    let work = root.0.join("work");
    let state = root.0.join("state");
    std::fs::create_dir_all(&work).expect("mkdir");
    std::fs::create_dir_all(&state).expect("mkdir");
    for n in 0..6 {
        std::fs::write(
            work.join(format!("Screenshot 2026-01-0{n} at 10.00.png")),
            b"x",
        )
        .expect("write");
    }

    for step in 1..=7 {
        let out = sweep(&state)
            .args(["lesson", &step.to_string()])
            .output()
            .expect("run");
        let text = String::from_utf8_lossy(&out.stdout).to_string();

        for line in text.lines().map(str::trim) {
            // The lines that are commands are the ones that start with the
            // tool's own name.
            let Some(rest) = line.strip_prefix("sweep ") else {
                continue;
            };
            // `sweep help` and further lesson steps are self-evidently fine,
            // and mkfx is a different binary.
            if rest.starts_with("lesson") || rest.starts_with("help") {
                continue;
            }
            // `forget` destroys the key in the login keychain, which is shared
            // by every process on this machine and not scoped by
            // XDG_STATE_HOME. Running it here sabotages whatever else is
            // running, which is exactly what it did when this test was
            // written: it turned the undo-walk test red four runs in five.
            // Its own behaviour is covered in exit_codes.rs, serially.
            if rest.starts_with("forget") {
                continue;
            }
            let args: Vec<String> = rest
                .split_whitespace()
                .map(|a| {
                    if a == "Desktop" {
                        work.to_string_lossy().to_string()
                    } else {
                        a.to_string()
                    }
                })
                .collect();
            let run = sweep(&state).args(&args).output().expect("run");
            let code = run.status.code();
            // 0 did it, 1 had nothing to do. Both are honest outcomes for a
            // taught command. 2 means the tool refuses what the lesson teaches
            // and 3 means it errors on it, and neither belongs in a lesson.
            //
            // An earlier version of this assertion only rejected 2, and so it
            // did not catch a taught `sweep undo --plan LATEST`: undo does not
            // check its flags, so the bad flag became a path and the command
            // exited 3. Rejecting only refusals let an error through.
            assert!(
                code == Some(0) || code == Some(1),
                "lesson {step} teaches `{line}`, which exits {code:?}:\n{}",
                String::from_utf8_lossy(&run.stderr)
            );
        }
    }
}

/// Step 4's claim is the one the whole tool rests on: apply everything, and a
/// file that looks like a personal record is still where it was.
#[test]
fn step_4_is_true_a_personal_record_survives_apply_everything() {
    let root = TestDir(unique("personal"));
    let work = root.0.join("work");
    let state = root.0.join("state");
    std::fs::create_dir_all(&work).expect("mkdir");
    std::fs::create_dir_all(&state).expect("mkdir");
    for n in 0..6 {
        std::fs::write(
            work.join(format!("Screenshot 2026-01-0{n} at 10.00.png")),
            b"x",
        )
        .expect("write");
    }
    let private = work.join("2024-1099-INT.pdf");
    std::fs::write(&private, b"tax").expect("write");

    let out = sweep(&state)
        .arg(&work)
        .args(["apply", "--yes"])
        .output()
        .expect("run");
    assert!(out.status.code() != Some(3), "apply errored");
    assert!(
        private.exists(),
        "step 4 says a personal record cannot be moved by any flag, and one moved"
    );
}

/// Steps 5 and 6 together: undo walks back one apply per run, and then says
/// there is nothing left rather than pretending.
#[test]
fn steps_5_and_6_are_true_undo_walks_back_then_stops() {
    let root = TestDir(unique("walk"));
    let work = root.0.join("work");
    let state = root.0.join("state");
    std::fs::create_dir_all(&work).expect("mkdir");
    std::fs::create_dir_all(&state).expect("mkdir");
    for n in 0..6 {
        std::fs::write(
            work.join(format!("Screenshot 2026-01-0{n} at 10.00.png")),
            b"x",
        )
        .expect("write");
    }
    for n in 0..6 {
        std::fs::write(work.join(format!("invoice-acme-{n}.pdf")), b"x").expect("write");
    }

    // Two applies, so there is a stack to walk.
    let first = sweep(&state)
        .args(["apply"])
        .arg(&work)
        .args(["--only", "Screenshots", "--yes"])
        .output()
        .expect("run");
    assert_eq!(
        first.status.code(),
        Some(0),
        "first apply did not move anything"
    );
    let second = sweep(&state)
        .args(["apply"])
        .arg(&work)
        .arg("--yes")
        .output()
        .expect("run");
    assert_eq!(
        second.status.code(),
        Some(0),
        "second apply did not move anything"
    );

    // Step 5: each run goes back one.
    let u1 = sweep(&state).arg("undo").output().expect("run");
    assert_eq!(u1.status.code(), Some(0), "the first undo did nothing");
    let u2 = sweep(&state).arg("undo").output().expect("run");
    assert_eq!(
        u2.status.code(),
        Some(0),
        "the second undo did nothing, so the lesson's 'run it again' is false"
    );

    // Step 6: and then it refuses rather than inventing work.
    let u3 = sweep(&state).arg("undo").output().expect("run");
    assert_eq!(
        u3.status.code(),
        Some(1),
        "step 6 says undo refuses once everything is back"
    );
}
