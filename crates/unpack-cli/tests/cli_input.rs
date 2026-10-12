use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("unpack-cli-input-{}-{serial}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn zip(&self) -> PathBuf {
        std::fs::write(self.0.join("payload.txt"), b"payload").unwrap();
        let zip = self.0.join("input.zip");
        let result = Command::new("zip")
            .args(["-q", "-j"])
            .arg(&zip)
            .arg(self.0.join("payload.txt"))
            .output()
            .expect("zip must be installed for unpack tests");
        assert!(
            result.status.success(),
            "could not create zip fixture: {result:?}"
        );
        zip
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_unpack"))
            .args(args)
            .output()
            .unwrap()
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn assert_refused_without_writes(output: Output, input: &str, root: &Path) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(2),
        "{input} should be refused: {stderr}"
    );
    assert!(
        stderr.contains(input),
        "refusal must name {input}: {stderr}"
    );
    assert_eq!(
        std::fs::read_dir(root).unwrap().count(),
        2,
        "refusal wrote into {}",
        root.display()
    );
}

#[test]
fn unknown_flags_after_archive_are_refused_and_named() {
    let d = TestDir::new();
    let zip = d.zip();
    let out = d.0.join("out");
    for flag in ["--lst", "--typo"] {
        let output = d.run(&[zip.to_str().unwrap(), "--into", out.to_str().unwrap(), flag]);
        assert_refused_without_writes(output, flag, &d.0);
    }
}

#[test]
fn valued_flags_refuse_missing_or_flag_shaped_values() {
    let d = TestDir::new();
    let zip = d.zip();
    let archive = zip.to_str().unwrap();
    for (args, bad) in [
        (vec![archive, "--into"], "--into"),
        (vec![archive, "--into", "--json"], "--json"),
        (vec![archive, "--max-size"], "--max-size"),
        (vec![archive, "--max-size", "--json"], "--json"),
    ] {
        let output = d.run(&args);
        assert_refused_without_writes(output, bad, &d.0);
    }
}

#[test]
fn malformed_or_overflowing_max_size_is_refused_and_named() {
    let d = TestDir::new();
    let zip = d.zip();
    let archive = zip.to_str().unwrap();
    for bad in ["abc", "1x", "-5", "99999999999999999999G", "17179869184G"] {
        let output = d.run(&[
            archive,
            "--into",
            d.0.join("out").to_str().unwrap(),
            "--max-size",
            bad,
        ]);
        assert_refused_without_writes(output, bad, &d.0);
    }
}
