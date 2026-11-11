// Shared by the integration tests of every tool crate:
//
//     #[path = "../../etude-cli-support/test-support/test_tool.rs"]
//     mod test_tool;
//
// Tests never use the login keychain. A tool run with no journal key asks the
// keychain for one, and with a throwaway HOME it may try to create one. So
// every test that runs a tool goes through `keyed_tool` (or `with_test_key`
// for a wrapper such as a shell or a script that runs the tool). Both give the
// run a random journal key, a throwaway state folder and a throwaway HOME.
// A test may still set its own key, state folder or HOME after that; the last
// value set wins.
//
// A test that must run a tool with no key at all uses `fenced_tool`. It runs
// the tool where the keychain program cannot start, and it fails the test
// before running anything if that cannot be arranged.
//
// scripts/check-test-keys.py fails CI when a test names a tool binary without
// going through this file.

#![allow(dead_code)]

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// The keychain program the tools call by its full path.
const KEYCHAIN_PROGRAM: &str = "/usr/bin/security";

/// Everything is allowed except starting the keychain program. CI runs the
/// whole suite under this same profile, and a sandbox can only be applied
/// again inside itself when the profile is the same, so keep the two equal.
pub const KEYCHAIN_FENCE: &str =
    r#"(version 1)(allow default)(deny process-exec (literal "/usr/bin/security"))"#;

struct TestEnvironment {
    key: String,
    state: PathBuf,
    home: PathBuf,
}

fn test_environment() -> &'static TestEnvironment {
    static ENVIRONMENT: OnceLock<TestEnvironment> = OnceLock::new();
    ENVIRONMENT.get_or_init(|| {
        let key = random_key();
        let base = Path::new(env!("CARGO_TARGET_TMPDIR")).join("test-tool");
        remove_stale_folders(&base);
        let own = base.join(format!("{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&own);
        let state = own.join("state");
        let home = own.join("home");
        std::fs::create_dir_all(&state).expect("create the throwaway state folder");
        std::fs::create_dir_all(&home).expect("create the throwaway HOME");
        TestEnvironment { key, state, home }
    })
}

/// 32 random bytes as 64 hex digits, new for each test process.
fn random_key() -> String {
    use std::io::Read;
    let mut bytes = [0u8; 32];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut source| source.read_exact(&mut bytes))
        .expect("read /dev/urandom for a test journal key");
    assert!(
        bytes.iter().any(|&b| b != 0),
        "/dev/urandom returned only zero bytes"
    );
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Remove folders left by earlier test processes. A test process has no safe
/// moment to remove its own folder (its tools may still be writing there when
/// the last test returns), so each one is left behind, inside the build's own
/// temp folder, and removed by a later run once it is more than a day old. A
/// day keeps clear of any test process still running.
fn remove_stale_folders(base: &Path) {
    let Ok(entries) = std::fs::read_dir(base) else {
        return;
    };
    let day = std::time::Duration::from_secs(24 * 60 * 60);
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > day);
        if old {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Give a command the test journal key, state folder and HOME. Use this for a
/// wrapper (a shell, a script, a pseudo-terminal) that runs a tool.
pub fn with_test_key(command: &mut Command) -> &mut Command {
    let environment = test_environment();
    command
        .env("ETUDE_JOURNAL_KEY", &environment.key)
        .env("ETUDE_STATE_DIR", &environment.state)
        .env("HOME", &environment.home)
}

/// A command for a tool binary, with the test journal key, state folder and
/// HOME already set.
pub fn keyed_tool(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    with_test_key(&mut command);
    command
}

/// A command for a tool binary with NO journal key, run where the keychain
/// program cannot start, so the keychain looks unavailable and is never
/// reached. Panics, before anything runs, if that cannot be arranged.
pub fn fenced_tool(program: impl AsRef<OsStr>) -> Command {
    let mut command = if cfg!(target_os = "macos") {
        assert_keychain_fence_works();
        let mut command = Command::new("/usr/bin/sandbox-exec");
        command.arg("-p").arg(KEYCHAIN_FENCE).arg(program);
        command
    } else {
        // There is no login keychain to reach without the keychain program.
        assert!(
            !Path::new(KEYCHAIN_PROGRAM).exists(),
            "{KEYCHAIN_PROGRAM} exists on this host but the keychain fence is \
             macOS only; refusing to run a tool without a journal key"
        );
        Command::new(program)
    };
    with_test_key(&mut command);
    command.env_remove("ETUDE_JOURNAL_KEY");
    command
}

/// True only if the fence applies and can deny a program from starting. The
/// proof denies a harmless program; the keychain program is never started,
/// not even as a control.
fn keychain_fence_works() -> bool {
    let sandbox = "/usr/bin/sandbox-exec";
    let applies = Command::new(sandbox)
        .args(["-p", KEYCHAIN_FENCE, "/usr/bin/true"])
        .output()
        .is_ok_and(|out| out.status.success());
    let denies = Command::new(sandbox)
        .arg("-p")
        .arg(format!(
            r#"{KEYCHAIN_FENCE}(deny process-exec (literal "/usr/bin/true"))"#
        ))
        .arg("/usr/bin/true")
        .output()
        .is_ok_and(|out| !out.status.success());
    applies && denies
}

fn assert_keychain_fence_works() {
    static WORKS: OnceLock<bool> = OnceLock::new();
    assert!(
        *WORKS.get_or_init(keychain_fence_works),
        "the keychain fence (sandbox-exec) is unavailable here; refusing to run \
         a tool without a journal key, because it would reach the login keychain"
    );
}
