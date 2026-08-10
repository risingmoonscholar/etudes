#![cfg(unix)]

use std::path::PathBuf;
use std::process::Command;

struct TestDir(PathBuf);

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn junk_removal_does_not_follow_symlinks_outside_the_target() {
    let root = std::env::temp_dir().join(format!("unpack-symlink-escape-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = TestDir(root);

    let outside = root.0.join("outside");
    std::fs::create_dir(&outside).unwrap();
    let sentinel = outside.join(".DS_Store");
    std::fs::write(&sentinel, b"outside sentinel").unwrap();

    let source = root.0.join("source");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("ordinary.txt"), b"ordinary").unwrap();
    std::os::unix::fs::symlink(&outside, source.join("outside-link")).unwrap();

    let archive = root.0.join("input.tar");
    let tar = Command::new("tar")
        .arg("-cf")
        .arg(&archive)
        .arg("-C")
        .arg(&source)
        .arg(".")
        .status()
        .unwrap();
    assert!(tar.success(), "tar failed with {tar}");

    let dest = root.0.join("target");
    let unpack = Command::new(env!("CARGO_BIN_EXE_unpack"))
        .arg(&archive)
        .arg("--into")
        .arg(&dest)
        .output()
        .unwrap();
    assert!(
        unpack.status.success(),
        "unpack failed: {}",
        String::from_utf8_lossy(&unpack.stderr)
    );

    assert!(sentinel.exists());
    assert_eq!(std::fs::read(&sentinel).unwrap(), b"outside sentinel");
}
