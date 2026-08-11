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
    let out = unpack_bin().arg(&missing).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(3),
        "missing archive must be error 3, not refusal 2: {}",
        String::from_utf8_lossy(&out.stderr)
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
    let out = unpack_bin().arg(&bogus).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(3),
        "unrecognised type must be error 3, not refusal 2: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
