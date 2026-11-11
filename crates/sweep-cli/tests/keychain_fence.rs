//! The shared test helper gives every tool run a key, or fences it.

#[path = "../../etude-cli-support/test-support/test_tool.rs"]
mod test_tool;
use std::ffi::OsStr;
use std::path::Path;
use test_tool::{fenced_tool, keyed_tool};

const KEYCHAIN_PROGRAM: &str = "/usr/bin/security";

#[test]
fn test_tool_gives_every_run_a_key_a_state_folder_and_a_home() {
    let command = keyed_tool("/usr/bin/true");
    let get = |name: &str| {
        command
            .get_envs()
            .find(|(k, _)| *k == OsStr::new(name))
            .and_then(|(_, v)| v)
            .map(|v| v.to_string_lossy().into_owned())
            .unwrap_or_else(|| panic!("{name} is not set"))
    };
    let key = get("ETUDE_JOURNAL_KEY");
    assert_eq!(key.len(), 64, "{key}");
    assert!(key.bytes().all(|b| b.is_ascii_hexdigit()), "{key}");
    let state = get("ETUDE_STATE_DIR");
    let home = get("HOME");
    assert!(Path::new(&state).is_dir(), "{state}");
    assert!(Path::new(&home).is_dir(), "{home}");
    let real_home = std::env::var("HOME").unwrap_or_default();
    assert_ne!(home, real_home, "the test HOME must not be the real one");
}

#[test]
fn fenced_tool_runs_with_no_key() {
    if cfg!(target_os = "macos") || !Path::new(KEYCHAIN_PROGRAM).exists() {
        let command = fenced_tool("/usr/bin/true");
        assert!(
            command
                .get_envs()
                .any(|(k, v)| k == OsStr::new("ETUDE_JOURNAL_KEY") && v.is_none()),
            "a fenced run must remove the journal key"
        );
    }
}
