// Shared integration-test body, included by each CLI. Every run goes through
// the same independent witness: scripts/check-progress-records.py.

#[allow(dead_code)]
#[derive(Default)]
struct Expect<'a> {
    planned: Option<usize>,
    completed: Option<usize>,
    journalled: Option<usize>,
    killed: bool,
    forbid: &'a [&'a str],
}

static NEXT_PROBE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

struct ProbeDir(std::path::PathBuf);
impl ProbeDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "etudes-progress-{tag}-{}-{}",
            std::process::id(),
            NEXT_PROBE.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        // canonicalize so a recorded path matches what the tool reports.
        Self(path.canonicalize().unwrap())
    }
    fn join(&self, name: &str) -> std::path::PathBuf {
        self.0.join(name)
    }
}
impl Drop for ProbeDir {
    fn drop(&mut self) {
        // A killed tool can leave a system extractor writing for a moment
        // after the kill. `remove_dir_all` then fails on a folder that gained
        // a file mid-removal, and the whole fixture (hundreds of MB) stayed
        // behind. Try again until the writer is gone, up to a bound.
        let give_up = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let _ = std::fs::remove_dir_all(&self.0);
            if !self.0.exists() || std::time::Instant::now() >= give_up {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

fn witness(operation: &str, stdout: &[u8], stderr: &[u8], expect: Expect<'_>) {
    let dir = ProbeDir::new("witness");
    std::fs::write(dir.join("stdout"), stdout).unwrap();
    std::fs::write(dir.join("stderr"), stderr).unwrap();
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/check-progress-records.py");
    let mut command = std::process::Command::new("python3");
    command
        .arg("-I")
        .arg(script)
        .arg("--stderr")
        .arg(dir.join("stderr"))
        .arg("--stdout")
        .arg(dir.join("stdout"))
        .args(["--operation", operation]);
    for (name, value) in [
        ("--planned", expect.planned),
        ("--completed", expect.completed),
        ("--journalled", expect.journalled),
    ] {
        if let Some(value) = value {
            command.args([name, &value.to_string()]);
        }
    }
    if expect.killed {
        command.arg("--killed");
    }
    for text in expect.forbid {
        command.args(["--forbid", text]);
    }
    let output = command.output().expect("python3 runs the witness");
    assert!(
        output.status.success(),
        "{}\nstderr was:\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(stderr)
    );
}
