//! macOS tar writes each file under a temporary name and renames it into place.
//! The soft-limit poll walks staging while that happens, so a name can vanish
//! between being listed and being examined. That must not fail the extraction.
use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "../../etude-cli-support/test-support/test_tool.rs"]
mod test_tool;
use test_tool::keyed_tool;

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("unpack-tar-race-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("tree")).unwrap();
        Fixture(root)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn file_count(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .map(|path| if path.is_dir() { file_count(&path) } else { 1 })
        .sum()
}

#[test]
fn a_large_tar_extracts_while_the_size_poll_races_its_renames() {
    const FILES: usize = 5000;
    let fixture = Fixture::new();
    for n in 0..FILES {
        std::fs::write(fixture.0.join(format!("tree/f{n}")), b"").unwrap();
    }
    let archive = fixture.0.join("big.tar");
    let made = Command::new("/usr/bin/tar")
        .arg("-cf")
        .arg(&archive)
        .arg("-C")
        .arg(&fixture.0)
        .arg("tree")
        .status()
        .unwrap();
    assert!(made.success());
    for run in 0..5 {
        let into = fixture.0.join(format!("out-{run}"));
        let output = keyed_tool(env!("CARGO_BIN_EXE_unpack"))
            .arg(&archive)
            .arg("--into")
            .arg(&into)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "run {run}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(file_count(&into), FILES, "run {run}");
    }
}
