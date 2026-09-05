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
fn a_tar_carrying_a_symlink_is_refused_and_touches_nothing() {
    let root =
        std::env::temp_dir().join(format!("unpack-tar-symlink-escape-{}", std::process::id()));
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

/// ZIP has no extractor-level symlink backstop, so its type-listing preflight
/// must reject the link before `unzip` is ever invoked.
#[test]
fn a_zip_carrying_a_symlink_is_refused_and_touches_nothing() {
    let root =
        std::env::temp_dir().join(format!("unpack-zip-symlink-escape-{}", std::process::id()));
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

    let archive = root.0.join("input.zip");
    let zip = Command::new("zip")
        .arg("-y")
        .arg(&archive)
        .arg("ordinary.txt")
        .arg("outside-link")
        .current_dir(&source)
        .status()
        .unwrap();
    assert!(zip.success(), "zip failed with {zip}");

    let dest = root.0.join("target");
    let unpack = Command::new(env!("CARGO_BIN_EXE_unpack"))
        .arg(&archive)
        .arg("--into")
        .arg(&dest)
        .output()
        .unwrap();

    assert_eq!(
        unpack.status.code(),
        Some(2),
        "a ZIP with a symlink was not refused: {}",
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
    assert!(
        !dest.exists(),
        "a refused archive still created its destination"
    );
    assert!(sentinel.exists());
    assert_eq!(std::fs::read(&sentinel).unwrap(), b"outside sentinel");
}

/// The ZIP plain and typed listings use different display formats.  This is
/// the ordinary case that proves their rows still line up before extraction.
#[test]
fn a_benign_zip_with_directory_spaced_and_non_ascii_names_extracts() {
    let root = std::env::temp_dir().join(format!("unpack-zip-benign-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = TestDir(root);

    let source = root.0.join("source");
    let nested = source.join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(nested.join(" spaced  name.txt"), b"nested").unwrap();
    std::fs::write(source.join("café.txt"), b"utf8").unwrap();
    std::fs::write(source.join("literal -> arrow.txt"), b"arrow").unwrap();

    let archive = root.0.join("input.zip");
    let zip = Command::new("zip")
        .args(["-r", "-q"])
        .arg(&archive)
        .arg("nested")
        .arg("café.txt")
        .arg("literal -> arrow.txt")
        .current_dir(&source)
        .status()
        .unwrap();
    assert!(zip.success(), "zip failed with {zip}");

    let dest = root.0.join("target");
    let unpack = Command::new(env!("CARGO_BIN_EXE_unpack"))
        .arg(&archive)
        .arg("--into")
        .arg(&dest)
        .status()
        .unwrap();
    assert!(unpack.success(), "benign ZIP was refused: {unpack}");
    assert_eq!(
        std::fs::read(dest.join("nested/ spaced  name.txt")).unwrap(),
        b"nested"
    );
    assert_eq!(std::fs::read(dest.join("café.txt")).unwrap(), b"utf8");
    assert_eq!(
        std::fs::read(dest.join("literal -> arrow.txt")).unwrap(),
        b"arrow"
    );
}

#[test]
fn a_benign_tar_gz_reports_the_visible_entry_count_as_checked() {
    let root = std::env::temp_dir().join(format!("unpack-targz-count-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = TestDir(root);

    let source = root.0.join("source");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("ordinary.txt"), b"ordinary").unwrap();
    let archive = root.0.join("input.tar.gz");
    let tar = Command::new("tar")
        .arg("-czf")
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
        .arg("--json")
        .output()
        .unwrap();
    assert!(
        unpack.status.success(),
        "benign tar.gz failed: {:?}",
        unpack
    );
    let stdout = String::from_utf8_lossy(&unpack.stdout);
    assert!(
        stdout.contains("\"entries\":2"),
        "unexpected listing count: {stdout}"
    );
    assert!(
        stdout.contains("\"paths_checked\":2"),
        "type count diverged: {stdout}"
    );
}

/// BSD tar changes old timestamps from `Aug 31 18:50` to `Jan  1  2024`.
/// The typed preflight must still reach the member rather than refusing a
/// harmless archive simply because its timestamp has crossed that boundary.
#[test]
fn a_benign_tar_with_an_old_timestamp_extracts() {
    let root = std::env::temp_dir().join(format!("unpack-old-tar-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = TestDir(root);

    let source = root.0.join("source");
    std::fs::create_dir(&source).unwrap();
    let member = source.join("old file -> literal.txt");
    std::fs::write(&member, b"old").unwrap();
    let touch = Command::new("touch")
        .args(["-t", "202401010101"])
        .arg(&member)
        .status()
        .unwrap();
    assert!(touch.success(), "touch failed with {touch}");

    let archive = root.0.join("input.tar");
    let tar = Command::new("tar")
        .arg("-cf")
        .arg(&archive)
        .arg("-C")
        .arg(&source)
        .arg("old file -> literal.txt")
        .status()
        .unwrap();
    assert!(tar.success(), "tar failed with {tar}");

    let dest = root.0.join("target");
    let unpack = Command::new(env!("CARGO_BIN_EXE_unpack"))
        .arg(&archive)
        .arg("--into")
        .arg(&dest)
        .status()
        .unwrap();
    assert!(unpack.success(), "old benign tar was refused: {unpack}");
    assert_eq!(
        std::fs::read(dest.join("old file -> literal.txt")).unwrap(),
        b"old"
    );
}
