#![cfg(unix)]

use std::path::PathBuf;
use std::process::Command;

struct TestDir(PathBuf);

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// An archive carrying a symlink is refused outright, and nothing outside is
/// touched.
///
/// This test used to assert that unpack SUCCEEDED on such an archive and
/// merely declined to follow the link while tidying junk. That was the right
/// assertion for a tool that extracted symlinks; it is the wrong one now.
/// Measured before the change: `unpack lone.zip` printed "Checked 2 paths
/// before writing anything" and landed `shortcut -> /etc/passwd` in the
/// target, because a name-only listing cannot see a member's type. The
/// symlink is refused at preflight now, so the guarantee is stronger --
/// nothing is written at all -- and this asserts the stronger thing.
#[test]
fn an_archive_carrying_a_symlink_is_refused_and_touches_nothing() {
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

    // Refused, exit 2, naming the member and offering a recourse.
    assert_eq!(
        unpack.status.code(),
        Some(2),
        "an archive with a symlink was not refused: {}",
        String::from_utf8_lossy(&unpack.stderr)
    );
    let msg = String::from_utf8_lossy(&unpack.stderr).to_string();
    assert!(
        msg.contains("symlink"),
        "the refusal does not name the kind: {msg}"
    );
    assert!(
        msg.contains("outside-link"),
        "the refusal does not name the member: {msg}"
    );
    assert!(
        msg.contains("--list"),
        "the refusal offers no recourse: {msg}"
    );

    // Nothing written, and nothing outside disturbed.
    assert!(
        !dest.exists(),
        "a refused archive still created its destination"
    );
    assert!(sentinel.exists());
    assert_eq!(std::fs::read(&sentinel).unwrap(), b"outside sentinel");
}
