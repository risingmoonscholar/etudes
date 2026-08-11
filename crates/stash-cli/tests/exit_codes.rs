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

/// `Journal::load_sealed` now refuses a journal torn mid progress-frame
/// (issue #3) instead of half-loading it — but `stash`'s own journal
/// lookup used to swallow that refusal with `.ok()` in a `filter_map`,
/// which turned "damaged" into "absent" with no trace. `pop` must at
/// least say the journal was found and is damaged, not just claim there
/// is nothing here.
#[test]
fn a_torn_journal_is_reported_as_damaged_not_silently_treated_as_absent() {
    let root = unique_temp("torn");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let _root = TestDir(root.clone());
    let state = unique_temp("torn-state");
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
        "stash must succeed before truncating its journal: stderr={} stdout={}",
        String::from_utf8_lossy(&stash.stderr),
        String::from_utf8_lossy(&stash.stdout)
    );

    let journal_path = std::fs::read_dir(&state)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("stash-") && n.ends_with(".journal"))
        })
        .expect("stash wrote a journal");

    // Cut the journal to 85% of its length, same shape as the deep
    // truncations in stress/scenarios/30-journal-truncation.sh: the base
    // frame survives intact, but the trailing progress frame recording
    // memo.txt's completed move is torn mid-write, exactly what a crash
    // leaves behind.
    let full = std::fs::read(&journal_path).unwrap();
    let cutoff = full.len() * 85 / 100;
    std::fs::write(&journal_path, &full[..cutoff]).unwrap();

    let pop = stash_bin()
        .env("ETUDE_STATE_DIR", &state)
        .args(["pop"])
        .arg(&root)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&pop.stderr);
    assert!(
        stderr.contains("damaged"),
        "pop against a torn journal must say the journal is damaged, not just claim \
         there is nothing here: stderr={stderr} stdout={}",
        String::from_utf8_lossy(&pop.stdout)
    );
    assert!(
        stderr.contains("truncated") || stderr.contains("progress record"),
        "the damage report should name a torn progress frame, not some other \
         load failure, or this isn't actually witnessing issue #3's shape: stderr={stderr}"
    );
    // Exit class matters as much as the message: a caller that only checks
    // the exit code (not stderr text) must not see the same code a folder
    // that was simply never stashed would produce. sweep's parallel path
    // (cmd_undo) already treats any non-NotFound load failure as exit 3;
    // stash must match that severity, not silently fall back to the
    // "nothing to do" exit 1 it uses for a genuine miss.
    assert_eq!(
        pop.status.code(),
        Some(3),
        "a damaged journal must exit 3 (a real failure), not the same exit 1 \
         a plain 'nothing stashed here' miss produces: stderr={stderr} stdout={}",
        String::from_utf8_lossy(&pop.stdout)
    );
    // Nothing was half-restored: the holding directory pop refused to touch
    // must still exist with the file inside it, not partially unpacked.
    let holding = std::fs::read_dir(&root)
        .unwrap()
        .flatten()
        .find(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| n.starts_with(".stash-"))
        })
        .expect("holding directory must survive a refused pop");
    assert!(
        std::fs::read_dir(holding.path())
            .unwrap()
            .flatten()
            .any(|e| e.file_name() == "memo.txt"),
        "the stashed file must still be inside the holding directory, not stranded elsewhere"
    );
}
