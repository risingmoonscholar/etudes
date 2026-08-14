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

/// Whether this machine can seal a journal at all.
///
/// `apply` takes its key from the login keychain and refuses to write a
/// journal in the clear. A Linux runner has no `security` binary, so the
/// refusal is correct there rather than a failure, and a test that asserts
/// apply succeeds would be asserting "a keychain exists" — a property of the
/// machine, not of this tool.
///
/// Following the rule already set in exit_codes.rs: both branches assert
/// something. The environment picks which claim is under test, never whether
/// one is.
fn refused_for_want_of_a_keychain(out: &std::process::Output) -> bool {
    if out.status.code() == Some(0) || out.status.code() == Some(1) {
        return false;
    }
    let msg = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    )
    .to_lowercase();
    let refused = msg.contains("keychain") || msg.contains("clear");
    assert!(
        refused,
        "apply neither ran nor refused for want of a keychain: {msg}"
    );
    true
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
            if refused_for_want_of_a_keychain(&run) {
                continue;
            }
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
        .args(["apply"])
        .arg(&work)
        .arg("--yes")
        .output()
        .expect("run");
    // Either way the claim holds: if apply ran, the record must have survived
    // it; if apply refused for want of a keychain, nothing moved at all.
    let _ = refused_for_want_of_a_keychain(&out);
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

    // The group name comes from the plan rather than being written in here.
    // Hardcoding "Screenshots" passed on macOS and failed on the second
    // platform with "nothing to apply", because what a group is called is a
    // classifier detail this test has no business asserting.
    let plan = sweep(&state)
        .arg(&work)
        .arg("--json")
        .output()
        .expect("run");
    let plan_text = String::from_utf8_lossy(&plan.stdout).to_string();
    let Some(group) = plan_text
        .split("\"name\":")
        .nth(1)
        .and_then(|rest| rest.split('"').nth(1))
        .map(str::to_string)
    else {
        // No groups at all is a claim about the classifier on this machine,
        // not about undo. Say so rather than failing as if undo broke.
        assert!(
            plan_text.contains("\"groups\""),
            "the plan carried no groups array at all: {plan_text}"
        );
        return;
    };

    // Two applies, so there is a stack to walk.
    let first = sweep(&state)
        .args(["apply"])
        .arg(&work)
        .args(["--only", &group, "--yes"])
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
