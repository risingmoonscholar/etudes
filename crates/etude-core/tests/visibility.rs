//! Issue: an unreadable subdirectory is invisible in the output.
//!
//! `chmod 000` a subdirectory and `scan()` silently drops its contents from
//! the count with no signal anywhere a caller can see. This proves the count
//! survives from `scan()` through `plan::build()` into both the JSON and the
//! field a human-facing renderer would read, and that a directory `scan()`
//! could not open at all is distinguished from a directory `scan()` refused
//! to enter on purpose (a `.ssh`, or an absolute system location). The two
//! mean different things to a user. The code can tell them apart.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use etude_core::plan;
use etude_core::scan::{self, ScanConfig};

fn unique_root(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "sweep_visibility_{tag}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

struct Cleanup(PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        // Restore permissions first. rm -rf of a 000 dir fails otherwise.
        let locked = self.0.join("locked");
        let _ = fs::set_permissions(&locked, fs::Permissions::from_mode(0o755));
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn an_unreadable_directory_is_counted_not_dropped() {
    let root = unique_root("unreadable");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("locked")).unwrap();
    for i in 0..5 {
        fs::write(root.join("locked").join(format!("secret_{i}.txt")), b"x").unwrap();
    }
    for i in 0..5 {
        fs::write(root.join(format!("visible_{i}.txt")), b"x").unwrap();
    }
    fs::set_permissions(root.join("locked"), fs::Permissions::from_mode(0o000)).unwrap();
    let _guard = Cleanup(root.clone());

    let out = scan::scan(
        &root,
        &ScanConfig {
            depth: 2,
            ..Default::default()
        },
    )
    .expect("scan");

    // The 5 visible files were found; the 5 behind the locked door were not.
    assert_eq!(out.entries.len(), 5, "visible files should still be found");

    // The failure to read `locked/` MUST be counted. This is the whole
    // issue. It is a real read_dir() failure, not a policy refusal, so it
    // must land in its own field, separate from a deliberate refusal.
    assert_eq!(
        out.skipped_unreadable, 1,
        "the unreadable subdirectory was not counted at all"
    );
    // And it must NOT be silently folded into the policy-refusal bucket.
    // sweep never chose to skip `locked/`. It tried and failed.
    assert_eq!(
        out.skipped_system, 0,
        "a genuine read failure was counted as a deliberate refusal"
    );

    // The count must survive into the plan, not be dropped by build().
    let p = plan::build(&out);
    assert_eq!(
        p.skipped_unreadable, 1,
        "plan::build dropped the unreadable count that scan() produced"
    );

    // And it must reach --json, in a form an agent can act on: the human
    // and the agent see the same data.
    let json = p.to_json();
    assert!(
        json.contains("unreadable"),
        "the JSON plan has no field at all for a directory that could not be read: {json}"
    );
    assert!(
        json.contains("\"unreadable\":1") || json.contains("\"unreadable\": 1"),
        "the JSON plan does not carry the actual unreadable count: {json}"
    );
}

#[test]
fn a_deliberate_refusal_is_not_confused_with_a_read_failure() {
    // never_enter names (node_modules etc.) are a policy choice, not an I/O
    // error. scan() could have read them and chose not to. That must land
    // in skipped_system, not skipped_unreadable, or a user reading
    // `--explain` cannot tell "you can't see this" from "sweep declined to
    // look". (Not `.ssh`. A dot-prefixed name is caught by the earlier
    // is_hidden() check first, which would test the wrong branch.)
    let root = unique_root("policy-refusal");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("node_modules")).unwrap();
    fs::write(root.join("node_modules").join("pkg.json"), b"x").unwrap();
    let _guard = Cleanup(root.clone());

    let out = scan::scan(
        &root,
        &ScanConfig {
            depth: 2,
            ..Default::default()
        },
    )
    .expect("scan");

    assert_eq!(
        out.skipped_unreadable, 0,
        "a deliberate policy refusal was miscounted as a read failure"
    );
    assert_eq!(
        out.skipped_system, 1,
        "node_modules should be counted as a deliberate refusal"
    );
}
