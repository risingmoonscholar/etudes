//! Issue: undo throws away what it already did when it hits an error.
//!
//! `sweep undo` restores journal entries in reverse order. If one entry's
//! origin directory has lost write permission, the entries that come before
//! it in that reverse walk still get physically restored -- but the old code
//! discarded the accumulated `UndoReport` the instant the failing entry's
//! move errored, so the CLI printed only the raw io error and never called
//! `save_sealed` on the error path. The on-disk journal was left claiming
//! every entry was still sitting in the holding directory, even though some
//! of them, in physical reality, were not.
//!
//! This reproduces that with a chmod'd directory rather than a mounted disk
//! image, so it runs everywhere the suite does.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

struct TestDir(PathBuf);

impl Drop for TestDir {
    fn drop(&mut self) {
        // Restore write permission before recursive removal, or cleanup
        // itself fails on the directory we deliberately locked down.
        let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
        if let Ok(rd) = std::fs::read_dir(&self.0) {
            for e in rd.flatten() {
                if e.path().is_dir() {
                    let _ =
                        std::fs::set_permissions(e.path(), std::fs::Permissions::from_mode(0o755));
                }
            }
        }
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn unique_temp(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "sweep-undo-partial-{tag}-{}-{}",
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

/// Reverse-undo-order trick: scan sorts by path, and undo walks entries in
/// reverse. A subdirectory name that sorts *before* the root-level filenames
/// gets a *lower* journal index, so it is processed *last* by undo's reverse
/// loop -- i.e. only after the writable, root-level entries already
/// succeeded. That gives a deterministic 3-restored-then-1-blocked split
/// with no timing luck.
/// The refusal a machine with no keychain must produce.
///
/// Third file to need this, so stating it once more plainly: `apply` seals its
/// journal with a key from the login keychain and refuses to write one in the
/// clear. CI has no keychain and Linux has no `security` binary, so the
/// refusal is correct there. A test asserting success is asserting that a
/// keychain exists, which is a fact about the machine.
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
fn undo_reports_and_persists_progress_made_before_a_mid_walk_failure() {
    let root = unique_temp("root");
    let _ = std::fs::remove_dir_all(&root);
    let locked_src = root.join("aaa_locked_source");
    std::fs::create_dir_all(&locked_src).unwrap();
    let _root = TestDir(root.clone());
    let state = unique_temp("state");
    let _ = std::fs::remove_dir_all(&state);
    std::fs::create_dir_all(&state).unwrap();
    let _state = TestDir(state.clone());

    // 5 filenames share the "stuck" token (MIN_TOKEN_GROUP): 3 live directly
    // in root (writable origin), 2 live in the subdirectory that will be
    // locked down before undo runs.
    for name in ["stuck_delta.txt", "stuck_echo.txt", "stuck_foxtrot.txt"] {
        std::fs::write(root.join(name), name.as_bytes()).unwrap();
    }
    for name in ["stuck_alpha.txt", "stuck_bravo.txt"] {
        std::fs::write(locked_src.join(name), name.as_bytes()).unwrap();
    }

    let apply = sweep_bin()
        .env("ETUDE_STATE_DIR", &state)
        .args(["apply"])
        .arg(&root)
        .args(["--depth", "2", "--yes"])
        .output()
        .unwrap();
    if !apply.status.success() {
        assert_refused_for_want_of_a_keychain(&apply);
        return;
    }
    assert!(
        root.join("stuck/stuck_alpha.txt").exists() && root.join("stuck/stuck_delta.txt").exists(),
        "setup: apply did not produce the expected grouped layout, cannot continue"
    );

    // Lock the subdirectory: the two entries whose origin lives there must
    // fail to restore (can't create a new directory entry without write
    // permission on the containing directory), while the three root-level
    // entries -- processed first by undo's reverse walk -- must succeed.
    std::fs::set_permissions(&locked_src, std::fs::Permissions::from_mode(0o555)).unwrap();

    let first = sweep_bin()
        .env("ETUDE_STATE_DIR", &state)
        .arg("undo")
        .output()
        .unwrap();
    let first_stdout = String::from_utf8_lossy(&first.stdout).to_string();
    let first_stderr = String::from_utf8_lossy(&first.stderr).to_string();

    // Physical proof this is a genuine partial restore, not a fluke: the 3
    // writable-origin files are back, the 2 locked-origin ones are still
    // sitting in the holding directory.
    let restored_to_root = ["stuck_delta.txt", "stuck_echo.txt", "stuck_foxtrot.txt"]
        .iter()
        .filter(|n| root.join(n).exists())
        .count();
    let still_stuck = ["stuck_alpha.txt", "stuck_bravo.txt"]
        .iter()
        .filter(|n| root.join("stuck").join(n).exists())
        .count();
    assert_eq!(
        (restored_to_root, still_stuck),
        (3, 2),
        "setup: expected a deterministic 3-restored/2-stuck split, got restored={restored_to_root} stuck={still_stuck} \
         (undo exit={:?} stdout=[{first_stdout}] stderr=[{first_stderr}])",
        first.status.code()
    );

    assert!(
        !first.status.success(),
        "undo must not report success (exit 0) while 2 files are still stuck: stdout=[{first_stdout}]"
    );

    // THE DEFECT: the CLI's error path used to print only the raw io error
    // and never mention the 3 files it had already, physically, restored.
    assert!(
        first_stdout.to_lowercase().contains("restored 3"),
        "undo's error-path output must report the 3 files it restored before hitting the \
         locked directory, but got stdout=[{first_stdout}] stderr=[{first_stderr}]"
    );

    // Restore write access and run undo again. The journal on disk must
    // already reflect that the 3 files were restored by the first call --
    // if it doesn't, the second call will misdescribe them as "already
    // gone" instead of just quietly not touching them again.
    std::fs::set_permissions(&locked_src, std::fs::Permissions::from_mode(0o755)).unwrap();

    let second = sweep_bin()
        .env("ETUDE_STATE_DIR", &state)
        .arg("undo")
        .output()
        .unwrap();
    let second_stdout = String::from_utf8_lossy(&second.stdout).to_string();
    let second_stderr = String::from_utf8_lossy(&second.stderr).to_string();

    assert!(
        second.status.success(),
        "second undo (write access restored) must finish the job: stdout=[{second_stdout}] stderr=[{second_stderr}]"
    );
    assert!(
        !second_stdout.to_lowercase().contains("already gone"),
        "the on-disk journal must already know the 3 files from the first call were restored -- \
         it must not conflate 'silently restored earlier, unreported' with 'the file is missing'. \
         stdout=[{second_stdout}]"
    );

    for name in [
        "stuck_delta.txt",
        "stuck_echo.txt",
        "stuck_foxtrot.txt",
        "stuck_alpha.txt",
        "stuck_bravo.txt",
    ] {
        assert!(
            root.join(name).exists() || locked_src.join(name).exists(),
            "file never made it back to its origin: {name}"
        );
    }
}
