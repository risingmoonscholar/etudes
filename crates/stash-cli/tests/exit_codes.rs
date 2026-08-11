//! Process-level exit-code contract for the shipped `stash` binary.
//!
//! Spawns the real binary so the codes match what a caller would see.
//! Journal encryption uses the login keychain (same path as production).

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
        "stash-exit-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn stash_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_stash"))
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
    let out = stash_bin()
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
fn an_exhausted_pop_exits_1_not_0() {
    let root = unique_temp("pop");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let _root = TestDir(root.clone());
    let state = unique_temp("pop-state");
    let _ = std::fs::remove_dir_all(&state);
    std::fs::create_dir_all(&state).unwrap();
    let _state = TestDir(state.clone());

    std::fs::write(root.join("memo.txt"), b"stash me").unwrap();

    let stash = stash_bin()
        .env("ETUDE_STATE_DIR", &state)
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        stash.status.success(),
        "stash must succeed before pop: stderr={} stdout={}",
        String::from_utf8_lossy(&stash.stderr),
        String::from_utf8_lossy(&stash.stdout)
    );

    let first = stash_bin()
        .env("ETUDE_STATE_DIR", &state)
        .args(["pop"])
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        first.status.success(),
        "first pop must restore: stderr={} stdout={}",
        String::from_utf8_lossy(&first.stderr),
        String::from_utf8_lossy(&first.stdout)
    );

    let second = stash_bin()
        .env("ETUDE_STATE_DIR", &state)
        .args(["pop"])
        .arg(&root)
        .output()
        .unwrap();
    assert_eq!(
        second.status.code(),
        Some(1),
        "exhausted pop must exit 1, not 0: stderr={} stdout={}",
        String::from_utf8_lossy(&second.stderr),
        String::from_utf8_lossy(&second.stdout)
    );
}
