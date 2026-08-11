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

    // Five distinct filenames sharing a token — apply succeeds, no collision.
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
