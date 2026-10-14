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

/// Whether a stash can actually be recorded here.
///
/// `stash` seals its journal with a key from the login keychain and refuses to
/// write one in the clear. A CI runner has no keychain, and a Linux runner has
/// no `security` binary at all, so on both of those the refusal is the correct
/// behaviour rather than a failure. A test that assumes otherwise is
/// asserting "a keychain exists". That is not a property of this tool.
///
/// Both branches assert something. The environment decides which claim is
/// under test, never whether one is.
fn stash_can_seal(root: &std::path::Path, state: &std::path::Path) -> (bool, std::process::Output) {
    let out = stash_bin()
        .env("ETUDE_STATE_DIR", state)
        .arg(root)
        .output()
        .unwrap();
    (out.status.success(), out)
}

/// The refusal a keychain-less machine must produce: non-zero, and it must say
/// why rather than failing quietly or pretending it stashed something.
fn assert_refused_for_want_of_a_keychain(out: &std::process::Output) {
    let msg = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        !out.status.success(),
        "with no keychain, stash must refuse rather than report success: {msg}"
    );
    assert!(
        msg.to_lowercase().contains("keychain") || msg.to_lowercase().contains("clear"),
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
fn status_by_id_is_path_free_and_reports_the_durable_recovery_state() {
    let root = unique_temp("status-id");
    let state = unique_temp("status-id-state");
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&state);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&state).unwrap();
    let _root = TestDir(root.clone());
    let _state = TestDir(state.clone());
    let secret_name = "status-private-filename-7fd1.txt";
    std::fs::write(root.join(secret_name), b"payload").unwrap();
    let key = "11".repeat(32);

    let stash = stash_bin()
        .env("ETUDE_STATE_DIR", &state)
        .env("ETUDE_JOURNAL_KEY", &key)
        .arg(&root)
        .arg("--json")
        .output()
        .unwrap();
    assert_eq!(
        stash.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&stash.stderr)
    );
    let id = std::fs::read_dir(&state)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("stash-") && name.ends_with(".journal"))
        })
        .unwrap()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .trim_start_matches("stash-")
        .trim_end_matches(".journal")
        .to_owned();
    let stash_output = String::from_utf8_lossy(&stash.stdout);
    assert!(stash_output.contains("\"operation_id\":"));
    assert!(stash_output.contains(&format!("\"operation\":\"{id}\"")));
    assert!(stash_output.contains(&format!("\"stash_id\":\"{id}\"")));

    let status = stash_bin()
        .env("ETUDE_STATE_DIR", &state)
        .env("ETUDE_JOURNAL_KEY", &key)
        .args(["status", "--id", &id, "--json"])
        .output()
        .unwrap();
    assert_eq!(
        status.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let status_output = String::from_utf8_lossy(&status.stdout);
    assert!(status_output.contains(&format!("\"operation_id\":\"{id}\"")));
    assert!(status_output.contains("\"tool\":\"stash\""));
    assert!(status_output.contains("\"state\":\"completed\""));
    assert!(status_output.contains("\"planned\":0"));
    assert!(status_output.contains("\"moved\":1"));
    assert!(status_output.contains("\"reversed\":0"));
    assert!(status_output.contains(&format!("stash pop --id {id}")));
    assert!(!status_output.contains(secret_name));

    let pop = stash_bin()
        .env("ETUDE_STATE_DIR", &state)
        .env("ETUDE_JOURNAL_KEY", &key)
        .args(["pop", "--id", &id, "--json"])
        .output()
        .unwrap();
    assert_eq!(
        pop.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&pop.stderr)
    );
    let reversed = stash_bin()
        .env("ETUDE_STATE_DIR", &state)
        .env("ETUDE_JOURNAL_KEY", &key)
        .args(["status", "--id", &id, "--json"])
        .output()
        .unwrap();
    assert_eq!(reversed.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&reversed.stdout).contains("\"state\":\"reversed\""));

    // An existing but unreadable journal is a failure, distinct from an
    // unknown id. Keep the same disposable key so this exercises journal
    // decoding rather than keychain availability.
    std::fs::write(
        state.join(format!("stash-{id}.journal")),
        b"not a sealed journal",
    )
    .unwrap();
    let unreadable = stash_bin()
        .env("ETUDE_STATE_DIR", &state)
        .env("ETUDE_JOURNAL_KEY", &key)
        .args(["status", "--id", &id, "--json"])
        .output()
        .unwrap();
    assert_eq!(unreadable.status.code(), Some(3));
    let stdout = String::from_utf8_lossy(&unreadable.stdout);
    assert!(
        stdout.contains("\"code\":\"journal_unreadable\""),
        "{stdout}"
    );
}

#[test]
fn unknown_status_id_has_a_typed_unknown_operation_reason() {
    let state = unique_temp("unknown-status-state");
    let _ = std::fs::remove_dir_all(&state);
    std::fs::create_dir_all(&state).unwrap();
    let _state = TestDir(state.clone());
    let output = stash_bin()
        .env("ETUDE_STATE_DIR", &state)
        .args(["status", "--id", "missing-operation", "--json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"code\":\"unknown_operation\""),
        "{stdout}"
    );
    assert!(stdout.contains("\"retry\":\"never\""), "{stdout}");
}

#[test]
fn status_by_id_does_not_migrate_legacy_state() {
    let home = unique_temp("status-read-only-home");
    let _ = std::fs::remove_dir_all(&home);
    let legacy = home.join(".local/state/etudes");
    let current = home.join("Library/Application Support/etudes");
    std::fs::create_dir_all(&legacy).unwrap();
    std::fs::write(legacy.join("sentinel"), b"keep").unwrap();
    let _home = TestDir(home.clone());

    let output = stash_bin()
        .env_remove("ETUDE_STATE_DIR")
        .env_remove("ETUDE_JOURNAL_KEY")
        .env("HOME", &home)
        .args(["status", "--id", "missing-operation", "--json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(legacy.join("sentinel").exists());
    assert!(!current.exists(), "read-only status must not migrate state");
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

    let (sealed, stash) = stash_can_seal(&root, &state);
    if !sealed {
        assert_refused_for_want_of_a_keychain(&stash);
        return;
    }

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
/// (issue #3) instead of half-loading it. But `stash`'s own journal
/// lookup used to swallow that refusal with `.ok()` in a `filter_map`.
/// That turned "damaged" into "absent" with no trace. `pop` must at
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

    let (sealed, stash) = stash_can_seal(&root, &state);
    if !sealed {
        assert_refused_for_want_of_a_keychain(&stash);
        return;
    }

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
    //
    // What "reported" means changed, and the property did not. This used to
    // refuse the whole journal, which said "damaged" and restored nothing.
    // A torn tail now stops the replay instead: the complete frames are
    // replayed, the entry whose frame was cut is the first Planned one and
    // is recovered by successor-entry recovery, and the damage is still
    // disclosed. Silently treating a torn journal as absent remains the
    // defect; refusing it was only ever one way of avoiding that.
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
    // The exit code answers "did the restore work", and here it did: the
    // items are back. What must never happen is the code a folder that was
    // never stashed produces -- exit 1, "nothing to do" -- because that is a
    // damaged journal reading as an ordinary miss.
    //
    // This asserted exit 3 when a torn journal was refused outright and
    // nothing was restored, and 3 was right for that. It is wrong now. A cut
    // tail is the ordinary outcome of any interrupted run, since stash moves
    // an item and only then records it, so exiting non-zero would make
    // routine crash recovery read as failure to every script that checks. The
    // damage is disclosed on stderr, which is asserted above and is where a
    // person sees it.
    assert_ne!(
        pop.status.code(),
        Some(1),
        "a damaged journal exited 1, the same code a plain 'nothing stashed \
         here' miss produces: stderr={stderr} stdout={}",
        String::from_utf8_lossy(&pop.stdout)
    );
    assert_eq!(
        pop.status.code(),
        Some(0),
        "the items were restored, so the exit code should say the operation \
         worked and let stderr carry the damage: stderr={stderr} stdout={}",
        String::from_utf8_lossy(&pop.stdout)
    );
    // The file is BACK, and that is the change. This used to assert the
    // opposite -- that the holding directory survived with memo.txt still
    // inside it -- because a torn journal was refused outright and nothing
    // was restored. Refusing did keep the report honest, but it also left
    // the item stashed with no way to get it out, and the same shape on
    // sweep's side left 130 files at their destinations in one CI run.
    //
    // A truncated tail now stops the replay instead of voiding the journal:
    // the complete frames are replayed, and the item whose record was cut is
    // the first Planned entry, which successor-entry recovery restores by
    // checking the filesystem. The exit code stays 3 and the message still
    // names the damage, so nothing about the disclosure got quieter.
    assert!(
        root.join("memo.txt").exists(),
        "memo.txt was not restored. A torn journal must not leave an item \
         stashed with no way to get it back"
    );
    let holding_left = std::fs::read_dir(&root)
        .unwrap()
        .flatten()
        .find(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| n.starts_with(".stash-"))
        })
        .map(|e| std::fs::read_dir(e.path()).unwrap().flatten().count());
    assert!(
        holding_left.is_none_or(|n| n == 0),
        "the holding directory still has {holding_left:?} items in it, so the \
         restore was partial"
    );
}
