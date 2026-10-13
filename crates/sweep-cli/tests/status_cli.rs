use std::path::PathBuf;
use std::process::Command;

struct TestDir(PathBuf);
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn temp(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "sweep-status-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}
fn run(state: &PathBuf, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_sweep"))
        .env("ETUDE_STATE_DIR", state)
        .env(
            "ETUDE_JOURNAL_KEY",
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .args(args)
        .output()
        .unwrap()
}
#[test]
fn unknown_status_id_is_a_classified_read_only_lookup() {
    let state = temp("unknown");
    std::fs::create_dir_all(&state).unwrap();
    let _state = TestDir(state.clone());
    let out = run(
        &state,
        &["status", "--id", "sweep-does-not-exist", "--json"],
    );
    assert_eq!(out.status.code(), Some(1));
    let json = String::from_utf8(out.stdout).unwrap();
    assert!(json.contains("\"code\":\"unknown_operation\""), "{json}");
}
#[test]
fn status_by_path_reports_durable_operation_and_counts_without_names() {
    let state = temp("path-state");
    std::fs::create_dir_all(&state).unwrap();
    let _state = TestDir(state.clone());
    let root = temp("folder");
    std::fs::create_dir_all(&root).unwrap();
    let _root = TestDir(root.clone());
    for i in 1..=3 {
        std::fs::write(
            root.join(format!("document_{i}.pdf")),
            format!("document-{i}"),
        )
        .unwrap();
        let file = std::fs::File::open(root.join(format!("document_{i}.pdf"))).unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3 * 24 * 60 * 60);
        file.set_times(std::fs::FileTimes::new().set_modified(old))
            .unwrap();
    }
    let applied = run(
        &state,
        &[
            "apply",
            root.to_str().unwrap(),
            "--yes",
            "--json",
            "--since",
            "0",
        ],
    );
    assert!(
        applied.status.success(),
        "{}",
        String::from_utf8_lossy(&applied.stderr)
    );
    let json = String::from_utf8(applied.stdout).unwrap();
    let durable_id = json
        .rsplit("\"operation\":\"")
        .next()
        .unwrap()
        .split('\"')
        .next()
        .unwrap();
    assert!(!durable_id.is_empty(), "{json}");
    let by_id = run(&state, &["status", "--id", durable_id, "--json"]);
    assert!(
        by_id.status.success(),
        "{}",
        String::from_utf8_lossy(&by_id.stderr)
    );
    let out = run(&state, &["status", root.to_str().unwrap(), "--json"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let status = String::from_utf8(out.stdout).unwrap();
    assert!(status.contains("\"state\":\"completed\""), "{status}");
    assert!(status.contains("\"moved\":"), "{status}");
    assert!(status.contains("sweep undo"), "{status}");
    assert!(
        !status.contains("document_1.pdf"),
        "status exposed an entry name: {status}"
    );

    let undone = run(&state, &["undo", root.to_str().unwrap(), "--json"]);
    assert!(
        undone.status.success(),
        "{}",
        String::from_utf8_lossy(&undone.stderr)
    );
    let undo_json = String::from_utf8(undone.stdout).unwrap();
    assert!(
        undo_json.contains(&format!("\"operation\":\"{durable_id}\"")),
        "{undo_json}"
    );
    let reversed = run(&state, &["status", "--id", durable_id, "--json"]);
    assert!(
        reversed.status.success(),
        "{}",
        String::from_utf8_lossy(&reversed.stderr)
    );
    assert!(
        String::from_utf8(reversed.stdout)
            .unwrap()
            .contains("\"state\":\"reversed\"")
    );
}
