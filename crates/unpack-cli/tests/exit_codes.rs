use std::path::PathBuf;
use std::process::Command;

struct TestDir(PathBuf);

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn unpack_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_unpack"))
}

#[test]
fn a_missing_archive_exits_3_not_2() {
    let root = std::env::temp_dir().join(format!(
        "unpack-exit-missing-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let _root = TestDir(root.clone());

    let missing = root.join("does-not-exist.tar");
    let out = unpack_bin()
        .arg(&missing)
        .arg("--json")
        .env("ETUDE_STATE_DIR", root.join("state"))
        .env("ETUDE_JOURNAL_KEY", "throwaway-unpack-test-key")
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(3),
        "missing archive must be error 3, not refusal 2: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let result = String::from_utf8_lossy(&out.stdout);
    assert!(
        result.contains(
            r#""reason":{"code":"missing_input","retry":"after_change","subject":{"kind":"input""#
        ),
        "missing input result was not classified: {result}"
    );
}

#[test]
fn an_unrecognised_archive_type_exits_3_not_2() {
    let root = std::env::temp_dir().join(format!(
        "unpack-exit-rar-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let _root = TestDir(root.clone());

    let bogus = root.join("notes.rar");
    std::fs::write(&bogus, b"not an archive").unwrap();
    let out = unpack_bin()
        .arg(&bogus)
        .arg("--json")
        .env("ETUDE_STATE_DIR", root.join("state"))
        .env("ETUDE_JOURNAL_KEY", "throwaway-unpack-test-key")
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(3),
        "unrecognised type must be error 3, not refusal 2: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let result = String::from_utf8_lossy(&out.stdout);
    assert!(
        result.contains(r#""reason":{"code":"unsupported_format","retry":"never"}"#),
        "unsupported archive format was not classified: {result}"
    );
}

/// The README claims the exit-code contract is uniform across the tools.
/// It was not: `unpack --frobnicate` took the typo as an archive name and
/// reported "not a file" with exit 3, so a caller could not tell a usage
/// mistake from a missing archive. `sweep` and `stash` both answer 2 here.
///
/// This is the same defect stash had with `--version`, found by checking a
/// documentation claim rather than by the harness.
#[test]
fn a_leading_typod_flag_is_refused_not_treated_as_an_archive() {
    let root = std::env::temp_dir().join(format!(
        "unpack-reason-unknown-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let _root = TestDir(root.clone());
    for typo in ["--frobnicate", "-x", "--lst"] {
        let out = unpack_bin()
            .args([typo, "--json"])
            .env("ETUDE_STATE_DIR", root.join("state"))
            .env("ETUDE_JOURNAL_KEY", "throwaway-unpack-test-key")
            .output()
            .expect("run");
        assert_eq!(
            out.status.code(),
            Some(2),
            "{typo} must be a refusal, not an error about a missing file"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            !stderr.contains("not a file"),
            "{typo} was taken as an archive name: {stderr}"
        );
        let result = String::from_utf8_lossy(&out.stdout);
        assert!(
            result.contains(r#""reason":{"code":"unknown_option","retry":"never"}"#),
            "unknown option result was not classified: {result}"
        );
    }

    let out = unpack_bin()
        .args(["contract", "--bogus"])
        .env("ETUDE_STATE_DIR", root.join("state"))
        .env("ETUDE_JOURNAL_KEY", "throwaway-unpack-test-key")
        .output()
        .expect("run invalid contract request");
    assert_eq!(out.status.code(), Some(2));
    let result = String::from_utf8_lossy(&out.stdout);
    assert!(
        result.contains(r#""reason":{"code":"invalid_arguments","retry":"never"}"#),
        "invalid contract arguments were not classified: {result}"
    );
}

/// And a real flag with no archive is a different mistake from a typo, so it
/// gets a different sentence. Calling a flag that exists "unknown" would be
/// its own small lie.
#[test]
fn a_real_flag_without_an_archive_says_what_is_missing() {
    let root = std::env::temp_dir().join(format!(
        "unpack-reason-invalid-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let _root = TestDir(root.clone());
    for flag in ["--list", "--json", "--into"] {
        let out = unpack_bin()
            .args([flag, "--json"])
            .env("ETUDE_STATE_DIR", root.join("state"))
            .env("ETUDE_JOURNAL_KEY", "throwaway-unpack-test-key")
            .output()
            .expect("run");
        assert_eq!(
            out.status.code(),
            Some(2),
            "{flag} alone is still a refusal"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            !stderr.contains("unknown option"),
            "{flag} is a real flag and must not be called unknown: {stderr}"
        );
        assert!(
            stderr.contains("needs one"),
            "{flag} should say an archive is missing: {stderr}"
        );
        let result = String::from_utf8_lossy(&out.stdout);
        assert!(
            result.contains(r#""reason":{"code":"invalid_arguments","retry":"never"}"#),
            "missing archive argument was not classified: {result}"
        );
    }
}

/// An archive path that runs through a regular file names nothing. The OS
/// calls it "not a directory" rather than "not found", and unpack used to
/// report it as `internal_error`, which tells a caller the request can never
/// work.
#[test]
fn an_archive_path_through_a_file_is_missing_input_not_an_internal_error() {
    let root = std::env::temp_dir().join(format!(
        "unpack-exit-through-file-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("state")).unwrap();
    let _root = TestDir(root.clone());
    std::fs::write(root.join("plain.txt"), b"x").unwrap();

    let out = unpack_bin()
        .arg(root.join("plain.txt/inside.zip"))
        .arg("--json")
        .env("ETUDE_STATE_DIR", root.join("state"))
        .env("ETUDE_JOURNAL_KEY", "ab".repeat(32))
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(3),
        "exit code is unchanged: {stdout}"
    );
    assert!(
        stdout.contains(
            r#""reason":{"code":"missing_input","retry":"after_change","subject":{"kind":"input""#
        ),
        "{stdout}"
    );
    assert!(!stdout.contains("internal_error"), "{stdout}");
}
