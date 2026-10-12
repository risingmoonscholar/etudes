#![cfg(unix)]

use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

struct TestDir(PathBuf);
impl TestDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("unpack-sigterm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct ProcessGuard {
    unpack: Child,
    extractor: Option<u32>,
    staging_prefix: String,
}
impl Drop for ProcessGuard {
    fn drop(&mut self) {
        let extractor = self
            .extractor
            .or_else(|| extraction_child(self.unpack.id(), &self.staging_prefix).0);
        if let Some(pid) = extractor
            && process_state(pid).is_some_and(|state| !state.starts_with('Z'))
        {
            let _ = Command::new("/bin/kill")
                .args(["-KILL", &pid.to_string()])
                .status();
        }
        let _ = self.unpack.kill();
        let _ = self.unpack.wait();
    }
}

#[cfg(target_os = "macos")]
mod process_inspection {
    use std::process::Command;

    fn process_info(pid: u32) -> Option<(String, String)> {
        let state = Command::new("/bin/ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        let command = Command::new("/bin/ps")
            .args(["-o", "command=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        if !state.status.success() || !command.status.success() {
            return None;
        }
        Some((
            String::from_utf8_lossy(&state.stdout).trim().to_owned(),
            String::from_utf8_lossy(&command.stdout).trim().to_owned(),
        ))
    }

    pub(super) fn state(pid: u32) -> Option<String> {
        process_info(pid).map(|(state, _)| state)
    }

    pub(super) fn extraction_child(
        parent: u32,
        staging_root: &str,
    ) -> (Option<u32>, Vec<String>, Vec<String>) {
        let output = Command::new("/usr/bin/pgrep")
            .args(["-P", &parent.to_string()])
            .output();
        let Ok(output) = output else {
            return (
                None,
                vec!["could not start /usr/bin/pgrep -P".into()],
                Vec::new(),
            );
        };
        if !output.status.success() && output.status.code() != Some(1) {
            return (
                None,
                vec![format!("pgrep failed: {}", output.status)],
                Vec::new(),
            );
        }
        let mut candidates = Vec::new();
        let mut staging_candidates = Vec::new();
        let mut matching = None;
        for pid in String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| line.trim().parse::<u32>().ok())
        {
            let Some((state, command)) = process_info(pid) else {
                candidates.push(format!("pid={pid} ps details unavailable"));
                continue;
            };
            candidates.push(format!("pid={pid} state={state:?} command={command:?}"));
            if command.contains(staging_root) {
                staging_candidates.push(format!("pid={pid} command={command:?}"));
            }
            if (command == "/usr/bin/unzip" || command.starts_with("/usr/bin/unzip "))
                && command.contains(staging_root)
            {
                matching = Some(pid);
            }
        }
        (matching, candidates, staging_candidates)
    }
}

#[cfg(target_os = "linux")]
mod process_inspection {
    use std::fs;

    fn details(pid: u32) -> Option<(u32, char)> {
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let tail = stat
            .rsplit_once(')')?
            .1
            .split_whitespace()
            .collect::<Vec<_>>();
        Some((tail.get(1)?.parse().ok()?, tail.first()?.chars().next()?))
    }

    pub(super) fn state(pid: u32) -> Option<String> {
        let (_, state) = details(pid)?;
        Some(state.to_string())
    }

    pub(super) fn extraction_child(
        parent: u32,
        staging_root: &str,
    ) -> (Option<u32>, Vec<String>, Vec<String>) {
        let mut parent_candidates = Vec::new();
        let mut staging_candidates = Vec::new();
        let mut matching = None;
        let Ok(entries) = fs::read_dir("/proc") else {
            return (None, vec!["could not read /proc".into()], Vec::new());
        };
        for entry in entries.flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<u32>().ok())
            else {
                continue;
            };
            let Some((ppid, state)) = details(pid) else {
                continue;
            };
            let executable = fs::read_link(format!("/proc/{pid}/exe"))
                .map(|path| path.display().to_string())
                .unwrap_or_else(|_| "<path unavailable>".into());
            let command = fs::read(format!("/proc/{pid}/cmdline"))
                .map(|bytes| String::from_utf8_lossy(&bytes).replace('\0', " "))
                .unwrap_or_default();
            let candidate =
                format!("pid={pid} ppid={ppid} executable={executable:?} state={state}");
            if ppid == parent {
                parent_candidates.push(candidate.clone());
            }
            if command.contains(staging_root) {
                staging_candidates.push(candidate);
            }
            if ppid == parent && executable == "/usr/bin/unzip" && command.contains(staging_root) {
                matching = Some(pid);
            }
        }
        (matching, parent_candidates, staging_candidates)
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
compile_error!("the SIGTERM process witness supports macOS and Linux only");

fn process_state(pid: u32) -> Option<String> {
    process_inspection::state(pid)
}

fn extraction_child(parent: u32, staging_root: &str) -> (Option<u32>, Vec<String>, Vec<String>) {
    process_inspection::extraction_child(parent, staging_root)
}

fn make_active_extractor_zip(dir: &std::path::Path) -> PathBuf {
    // Many modest stored members make extraction spend time creating files,
    // without relying on a large byte-copy window or consuming gigabytes.
    let source = dir.join("payload");
    std::fs::create_dir(&source).unwrap();
    let bytes = vec![b'x'; 2 * 1024];
    for index in 0..32_768 {
        std::fs::write(source.join(format!("entry-{index:04}.bin")), &bytes).unwrap();
    }
    let archive = dir.join("input.zip");
    let result = Command::new("/usr/bin/zip")
        .args(["-q", "-r", "-0"])
        .arg(&archive)
        .arg(&source)
        .status()
        .unwrap();
    assert!(result.success(), "zip fixture failed");
    std::fs::remove_dir_all(source).unwrap();
    archive
}

fn wait_output(child: &mut Child, extractor: Option<u32>) -> Output {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            // If unpack died before it could reap the child, the stopped
            // extractor still owns the inherited output pipes. Kill it only
            // as test cleanup so reading the parent's captured output cannot
            // hang forever; the assertions below still require unpack itself
            // to report successful SIGTERM cleanup.
            if let Some(pid) = extractor
                && process_state(pid).is_some_and(|state| !state.starts_with('Z'))
            {
                let killed = Command::new("/bin/kill")
                    .args(["-KILL", &pid.to_string()])
                    .status()
                    .unwrap();
                assert!(
                    killed.success(),
                    "test cleanup could not kill extractor {pid}"
                );
                let cleanup_deadline = Instant::now() + Duration::from_secs(5);
                while process_state(pid).is_some_and(|state| !state.starts_with('Z')) {
                    assert!(
                        Instant::now() < cleanup_deadline,
                        "test cleanup could not reap extractor {pid}"
                    );
                    thread::sleep(Duration::from_millis(10));
                }
            }
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            child
                .stdout
                .take()
                .unwrap()
                .read_to_end(&mut stdout)
                .unwrap();
            child
                .stderr
                .take()
                .unwrap()
                .read_to_end(&mut stderr)
                .unwrap();
            return Output {
                status,
                stdout,
                stderr,
            };
        }
        assert!(
            Instant::now() < deadline,
            "unpack did not stop after SIGTERM"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn sigterm_stops_extractor_cleans_staging_and_reports_final_result() {
    let dir = TestDir::new();
    let archive = make_active_extractor_zip(&dir.0);
    let dest = dir.0.join("out");
    let unpack = Command::new(env!("CARGO_BIN_EXE_unpack"))
        .arg(&archive)
        .args(["--into", dest.to_str().unwrap(), "--max-size", "1G"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let parent = unpack.id();
    let staging_prefix = format!(".{}.unpack-", dest.file_name().unwrap().to_string_lossy());
    let mut guard = ProcessGuard {
        unpack,
        extractor: None,
        staging_prefix: staging_prefix.clone(),
    };
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut parent_candidates: Vec<String>;
    let mut staging_candidates: Vec<String>;
    let extractor = loop {
        let (child, parents, staging) = extraction_child(parent, &staging_prefix);
        parent_candidates = parents;
        staging_candidates = staging;
        if let Some(pid) = child {
            break pid;
        }
        if guard.unpack.try_wait().unwrap().is_some() {
            let output = wait_output(&mut guard.unpack, child);
            panic!(
                "unpack exited before the extractor could be observed (status {:?}); parent candidates={parent_candidates:?}; staging candidates={staging_candidates:?}; stdout={} stderr={}",
                output.status.code(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        assert!(
            Instant::now() < deadline,
            "unzip extractor never started; PPID={parent} candidates={parent_candidates:?}; staging matches for {staging_prefix:?}={staging_candidates:?}"
        );
        thread::sleep(Duration::from_millis(2));
    };
    guard.extractor = Some(extractor);

    let pause = Command::new("/bin/kill")
        .args(["-STOP", &extractor.to_string()])
        .status()
        .unwrap();
    assert!(pause.success(), "could not SIGSTOP the live extractor");
    assert!(
        process_state(extractor).is_some_and(|state| state.starts_with('T')),
        "extractor did not remain alive in the stopped state"
    );

    let signal = Command::new("/bin/kill")
        .args(["-TERM", &parent.to_string()])
        .status()
        .unwrap();
    assert!(signal.success(), "could not send SIGTERM to unpack");
    let output = wait_output(&mut guard.unpack, Some(extractor));

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "SIGTERM result: {stderr}");
    assert!(
        stderr.contains("SIGTERM"),
        "final result does not name SIGTERM: {stderr}"
    );
    assert!(
        stderr.contains("Nothing was left behind") || stderr.contains("staging remains"),
        "cleanup result missing: {stderr}"
    );
    assert!(!dest.exists(), "destination was published after SIGTERM");
    assert!(
        process_state(extractor).is_none_or(|state| state.starts_with('Z')),
        "extractor was still running after unpack exited"
    );
    assert!(
        std::fs::read_dir(&dir.0)
            .unwrap()
            .flatten()
            .all(|entry| !entry.file_name().to_string_lossy().contains(".unpack-")),
        "staging was left behind"
    );
}
