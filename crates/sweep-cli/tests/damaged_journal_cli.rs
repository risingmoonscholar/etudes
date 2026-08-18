//! `sweep undo` against a journal whose records were lost, through the CLI.
//!
//! Why a CLI test and not another unit test: the API-level version of this
//! passed while the command-line path was still wrong. `apply::undo` refuses
//! correctly, but `sweep undo` never reached it -- `journal_is_fully_undone`
//! saw no entry marked Moved, concluded there was nothing to reverse, and
//! printed "already restored" with exit 1 while every file sat at its
//! destination. A test that calls the library directly cannot see that.

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
        "sweep-damaged-{tag}-{}-{}",
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

/// Cut a journal back to `frames` complete progress records.
fn truncate_journal(state: &PathBuf, frames: usize) -> bool {
    let Some(j) = std::fs::read_dir(state).ok().and_then(|rd| {
        rd.flatten()
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|x| x == "journal"))
    }) else {
        return false;
    };
    let Ok(raw) = std::fs::read(&j) else {
        return false;
    };
    if raw.len() < 4 {
        return false;
    }
    let base = u32::from_le_bytes(raw[0..4].try_into().unwrap()) as usize;
    let mut off = 4 + base;
    let mut kept = 0;
    while off < raw.len() && kept < frames {
        if off + 4 > raw.len() {
            break;
        }
        let len = u32::from_le_bytes(raw[off..off + 4].try_into().unwrap()) as usize;
        off += 4 + len;
        kept += 1;
    }
    std::fs::write(&j, &raw[..off.min(raw.len())]).is_ok()
}

fn run_case(tag: &str, frames: usize) {
    run_case_inner(tag, frames, false);
    run_case_inner(&format!("{tag}-path"), frames, true);
}

/// `with_path` picks which of the two lookups is exercised. They are separate
/// functions with separate filters, and the first fix covered only one of
/// them: `sweep undo` refused correctly while `sweep undo PATH` still said no
/// apply of that folder was reversible. Two routes, two chances to be wrong.
fn run_case_inner(tag: &str, frames: usize, with_path: bool) {
    let root = unique_temp(tag);
    let _g = TestDir(root.clone());
    let desk = root.join("D");
    std::fs::create_dir_all(&desk).expect("mkdir");
    for i in 0..20 {
        std::fs::write(
            desk.join(format!(
                "Screenshot 2026-01-0{} at 0{i}.00.00 AM.png",
                i % 9 + 1
            )),
            b"",
        )
        .expect("write");
    }
    let state = unique_temp(&format!("{tag}-state"));
    std::fs::create_dir_all(&state).expect("mkdir state");
    let _gs = TestDir(state.clone());

    let applied = sweep_bin()
        .env("ETUDE_STATE_DIR", &state)
        .arg("apply")
        .arg(&desk)
        .arg("--yes")
        .output()
        .expect("run apply");
    if !applied.status.success() {
        eprintln!("skipped: apply did not succeed (no keychain?)");
        return;
    }
    if !truncate_journal(&state, frames) {
        eprintln!("skipped: could not truncate the journal");
        return;
    }

    let moved_before = std::fs::read_dir(desk.join("Screenshots"))
        .map(|rd| rd.flatten().count())
        .unwrap_or(0);
    assert!(moved_before > 1, "test setup: expected several moved files");

    let mut cmd = sweep_bin();
    cmd.env("ETUDE_STATE_DIR", &state).arg("undo");
    if with_path {
        cmd.arg(&desk);
    }
    let undo = cmd.output().expect("run undo");
    let stdout = String::from_utf8_lossy(&undo.stdout);
    let stderr = String::from_utf8_lossy(&undo.stderr);

    assert!(
        !stdout.contains("already restored"),
        "undo claimed the apply was already restored while {moved_before} files sit \
         at their destinations (with_path={with_path}). stdout={stdout}"
    );
    assert!(
        !stderr.contains("No apply of that folder is still"),
        "the path lookup skipped a journal whose records were lost, so it \
         reported nothing reversible while {moved_before} files sit at their \
         destinations. stderr={stderr}"
    );
    assert_eq!(
        undo.status.code(),
        Some(3),
        "a journal missing several records must exit 3, not report success or \
         nothing-to-do (with_path={with_path}). stdout={stdout} stderr={stderr}"
    );
    let after = std::fs::read_dir(desk.join("Screenshots"))
        .map(|rd| rd.flatten().count())
        .unwrap_or(0);
    assert_eq!(
        after, moved_before,
        "undo moved files despite refusing; it should touch nothing"
    );
}

/// The shape that slipped past: every record gone, so nothing reads Moved and
/// no tail reads torn.
#[test]
fn a_journal_cut_to_its_base_frame_is_not_reported_as_already_restored() {
    run_case("base", 0);
}

/// And the shape with a couple of records left, which reaches the check by a
/// different route.
#[test]
fn a_journal_missing_most_of_its_records_refuses_through_the_cli() {
    run_case("partial", 2);
}
