#![cfg(unix)]

//! A SIGKILL runs no cleanup, so the private archive copy must already have no
//! name on disk by the time extraction starts.

use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

struct TestDir(PathBuf);
impl TestDir {
    fn new(label: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("unpack-stranded-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        // A killed unpack can leave its system extractor writing for a moment
        // after the kill. `remove_dir_all` then fails on a folder that gained
        // a file mid-removal, and the whole fixture stayed behind. Try again
        // until the extractor is gone, up to a bound.
        let give_up = Instant::now() + Duration::from_secs(30);
        loop {
            let _ = std::fs::remove_dir_all(&self.0);
            if !self.0.exists() || Instant::now() >= give_up {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
    }
}

fn run(command: &mut Command) {
    let status = command.status().unwrap();
    assert!(status.success(), "fixture command failed: {command:?}");
}

fn make_tree(dir: &Path) -> PathBuf {
    // Many small members make extraction slow without using much disk.
    let tree = dir.join("payload");
    std::fs::create_dir(&tree).unwrap();
    for index in 0..8_000 {
        std::fs::write(tree.join(format!("{index:05}.txt")), b"member\n").unwrap();
    }
    tree
}

fn make_archive(dir: &Path, name: &str) -> PathBuf {
    let archive = dir.join(name);
    if name.ends_with(".gz") && !name.ends_with(".tar.gz") {
        let out = std::fs::File::create(&archive).unwrap();
        let mut gzip = Command::new("/usr/bin/gzip")
            .arg("-c")
            .stdin(Stdio::piped())
            .stdout(out)
            .spawn()
            .unwrap();
        let chunk = vec![0_u8; 1024 * 1024];
        let mut stdin = gzip.stdin.take().unwrap();
        for _ in 0..512 {
            std::io::Write::write_all(&mut stdin, &chunk).unwrap();
        }
        drop(stdin);
        assert!(gzip.wait().unwrap().success());
        return archive;
    }
    let tree = make_tree(dir);
    if name.ends_with(".zip") || name.ends_with(".jar") {
        run(Command::new("/usr/bin/zip")
            .args(["-q", "-r", "-0"])
            .arg(&archive)
            .arg("payload")
            .current_dir(dir));
    } else {
        let flag = if name.ends_with(".tar") {
            "-cf"
        } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
            "-czf"
        } else if name.ends_with(".tar.bz2") || name.ends_with(".tbz") {
            "-cjf"
        } else {
            "-cJf"
        };
        run(Command::new("/usr/bin/tar")
            .arg(flag)
            .arg(&archive)
            .arg("payload")
            .current_dir(dir));
    }
    std::fs::remove_dir_all(tree).unwrap();
    archive
}

fn staging_exists(parent: &Path) -> bool {
    std::fs::read_dir(parent)
        .unwrap()
        .flatten()
        .any(|entry| entry.file_name().to_string_lossy().ends_with(".partial"))
}

fn children(parent: u32) -> Vec<String> {
    let output = Command::new("pgrep")
        .args(["-P", &parent.to_string()])
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_owned)
        .collect()
}

fn entries(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect()
}

fn assert_kill_leaves_no_copy(name: &str) {
    let dir = TestDir::new(&name.replace('.', "-"));
    let archive = make_archive(&dir.0, name);
    let temp = dir.0.join("tmp");
    let out = dir.0.join("out");
    std::fs::create_dir(&temp).unwrap();
    std::fs::create_dir(&out).unwrap();
    let dest = out.join("dest");

    let mut unpack = Command::new(env!("CARGO_BIN_EXE_unpack"))
        .arg(&archive)
        .args(["--into", dest.to_str().unwrap(), "--max-size", "4G"])
        .env("TMPDIR", &temp)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(60);
    while !staging_exists(&out) {
        assert!(
            unpack.try_wait().unwrap().is_none(),
            "{name}: unpack exited before extraction started"
        );
        assert!(
            Instant::now() < deadline,
            "{name}: extraction never started"
        );
        thread::sleep(Duration::from_millis(1));
    }
    let orphans = children(unpack.id());
    unpack.kill().unwrap();
    let status = unpack.wait().unwrap();
    // The system extractor survives the kill. Stop it so the folder can go.
    for pid in orphans {
        let _ = Command::new("/bin/kill")
            .args(["-KILL", &pid])
            .stderr(Stdio::null())
            .status();
    }

    assert_eq!(status.signal(), Some(9), "{name}: unpack was not killed");
    assert!(!dest.exists(), "{name}: the kill came after publication");
    let left = entries(&temp);
    assert!(left.is_empty(), "{name}: stranded in TMPDIR: {left:?}");
}

macro_rules! kill_tests {
    ($($test:ident => $name:literal,)*) => {
        $(#[test]
        fn $test() {
            assert_kill_leaves_no_copy($name);
        })*
    };
}

kill_tests! {
    sigkill_leaves_no_copy_zip => "input.zip",
    sigkill_leaves_no_copy_jar => "input.jar",
    sigkill_leaves_no_copy_tar => "input.tar",
    sigkill_leaves_no_copy_tar_gz => "input.tar.gz",
    sigkill_leaves_no_copy_tgz => "input.tgz",
    sigkill_leaves_no_copy_tar_bz2 => "input.tar.bz2",
    sigkill_leaves_no_copy_tbz => "input.tbz",
    sigkill_leaves_no_copy_tar_xz => "input.tar.xz",
    sigkill_leaves_no_copy_txz => "input.txz",
    sigkill_leaves_no_copy_gz => "input.txt.gz",
}

#[test]
fn extractor_messages_name_the_archive_and_never_the_descriptor() {
    let dir = TestDir::new("message");
    let archive = dir.0.join("broken.txt.gz");
    std::fs::write(&archive, b"this is not gzip data").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_unpack"))
        .arg(&archive)
        .arg("--json")
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.status.success(), "{text}");
    assert!(!text.contains("/dev/fd/"), "{text}");
    assert!(text.contains("broken.txt.gz"), "{text}");
}

#[test]
fn every_format_extracts_after_two_listings_share_one_descriptor() {
    let dir = TestDir::new("offset");
    for name in [
        "a.zip",
        "a.jar",
        "a.tar",
        "a.tar.gz",
        "a.tgz",
        "a.tar.bz2",
        "a.tbz",
        "a.tar.xz",
        "a.txz",
        "a.txt.gz",
    ] {
        let work = dir.0.join(name.replace('.', "-"));
        std::fs::create_dir(&work).unwrap();
        let archive = if name == "a.txt.gz" {
            std::fs::write(work.join("a.txt"), b"member\n").unwrap();
            run(Command::new("/usr/bin/gzip").arg(work.join("a.txt")));
            work.join(name)
        } else {
            std::fs::create_dir(work.join("tree")).unwrap();
            std::fs::write(work.join("tree/one.txt"), b"member\n").unwrap();
            std::fs::write(work.join("two.txt"), b"member\n").unwrap();
            let archive = work.join(name);
            if name.ends_with(".zip") || name.ends_with(".jar") {
                run(Command::new("/usr/bin/zip")
                    .args(["-q", "-r"])
                    .arg(&archive)
                    .args(["tree", "two.txt"])
                    .current_dir(&work));
            } else {
                run(Command::new("/usr/bin/tar")
                    .arg("-caf")
                    .arg(&archive)
                    .args(["tree", "two.txt"])
                    .current_dir(&work));
            }
            archive
        };
        let dest = work.join("out");
        let output = Command::new(env!("CARGO_BIN_EXE_unpack"))
            .arg(&archive)
            .args(["--into", dest.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let member = if name == "a.txt.gz" {
            dest.join("a.txt")
        } else {
            dest.join("tree/one.txt")
        };
        assert_eq!(std::fs::read(&member).unwrap(), b"member\n", "{name}");
    }
}
