//! Defect #1: `Library`/`System`/`Applications` were refused by NAME anywhere
//! in the path, conflated with credential directories (which correctly ARE
//! name-based). That made `$HOME/Library/Mobile Documents` — iCloud Drive,
//! where a Mac with "Desktop & Documents" sync on keeps the real Desktop —
//! permanently unreachable: the system-location refusal fired before
//! `is_synced`/`--allow-sync` ever got a chance to run.
//!
//! These tests prove: iCloud Drive is now reachable with `--allow-sync` (and
//! only with it), a real system location like `$HOME/Library/Preferences`
//! stays refused regardless of the flag, and a user's own folder that simply
//! happens to be NAMED `Library` elsewhere is no longer refused at all.
//!
//! `HOME` is process-global, so these tests serialise on a lock the same way
//! `apply_undo.rs` serialises on `ETUDE_STATE_DIR`.

use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

use etude_core::scan::{self, ScanConfig, ScanError};

fn lock() -> MutexGuard<'static, ()> {
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Sets `HOME` for the duration of the guard, restoring whatever was there
/// (or unsetting it) on drop — so a failing test can't leak a fake HOME into
/// the rest of the suite.
struct FakeHome {
    previous: Option<std::ffi::OsString>,
}

impl FakeHome {
    fn set(path: &std::path::Path) -> Self {
        let previous = std::env::var_os("HOME");
        unsafe { std::env::set_var("HOME", path) };
        Self { previous }
    }

    /// Removes `HOME` entirely for the duration of the guard, proving the
    /// fail-closed path: `home_dir()` must return `None` rather than a
    /// value that quietly disables the `$HOME/Library` guard.
    fn unset() -> Self {
        let previous = std::env::var_os("HOME");
        unsafe { std::env::remove_var("HOME") };
        Self { previous }
    }
}

impl Drop for FakeHome {
    fn drop(&mut self) {
        match &self.previous {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }
}

fn fresh_root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("sweep_sysloc_{tag}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    root
}

fn write_files(dir: &std::path::Path, names: &[&str]) {
    fs::create_dir_all(dir).expect("mkdir");
    for n in names {
        fs::write(dir.join(n), b"x").expect("write fixture file");
    }
}

#[test]
fn icloud_desktop_is_reachable_with_allow_sync() {
    let _g = lock();
    let home = fresh_root("icloud_home");
    let desktop = home.join("Library/Mobile Documents/com~apple~CloudDocs/Desktop");
    write_files(&desktop, &["a.pdf", "b.pdf", "c.pdf"]);
    let _fake_home = FakeHome::set(&home);

    // Without --allow-sync: refused, but as a SYNC refusal, not a system
    // location refusal — proof the two paths finally meet.
    let cfg_default = ScanConfig::default();
    let err = scan::scan(&desktop, &cfg_default).expect_err("should refuse without allow_sync");
    assert!(
        matches!(err, ScanError::RefusedSyncRoot(_)),
        "expected RefusedSyncRoot, got {err:?}"
    );

    // With --allow-sync: succeeds.
    let cfg_allow = ScanConfig {
        allow_sync: true,
        ..Default::default()
    };
    let out = scan::scan(&desktop, &cfg_allow).expect("scan should succeed with allow_sync");
    assert!(
        out.root_is_synced,
        "iCloud Desktop should be flagged synced"
    );
    assert_eq!(out.entries.len(), 3);

    fs::remove_dir_all(&home).ok();
}

#[test]
fn home_library_preferences_stays_refused_even_with_allow_sync() {
    let _g = lock();
    let home = fresh_root("prefs_home");
    let prefs = home.join("Library/Preferences");
    write_files(&prefs, &["com.example.plist"]);
    let _fake_home = FakeHome::set(&home);

    let cfg_allow = ScanConfig {
        allow_sync: true,
        ..Default::default()
    };
    let err = scan::scan(&prefs, &cfg_allow).expect_err("Library/Preferences must stay refused");
    assert!(
        matches!(err, ScanError::RefusedSystemLocation(_)),
        "expected RefusedSystemLocation, got {err:?}"
    );

    fs::remove_dir_all(&home).ok();
}

#[test]
fn without_home_a_library_folder_anywhere_is_refused_not_silently_allowed() {
    let _g = lock();
    // Real adversarial-review finding: with HOME unset (`env -u HOME sweep
    // ...`), home_dir() has nothing to scope $HOME/Library against, and a
    // naive implementation would just never refuse Library at all — turning
    // a real system-location guard into a no-op. It must fail closed
    // instead: refuse a `Library` component anywhere, same as before this
    // defect was split apart.
    let _fake_home = FakeHome::unset();

    let somewhere = fresh_root("no_home").join("random/nested/Library");
    write_files(&somewhere, &["secret.plist"]);

    let err = scan::scan(&somewhere, &ScanConfig::default())
        .expect_err("Library must be refused when HOME is unknown");
    assert!(
        matches!(err, ScanError::RefusedSystemLocation(_)),
        "expected RefusedSystemLocation, got {err:?}"
    );

    fs::remove_dir_all(somewhere.parent().unwrap().parent().unwrap()).ok();
}

#[test]
fn a_folder_of_your_own_named_library_is_refused_and_that_is_the_trade() {
    let _g = lock();
    let home = fresh_root("elsewhere_home");
    fs::create_dir_all(&home).expect("mkdir home");
    let _fake_home = FakeHome::set(&home);

    // Telling this apart from $HOME/Library needs to trust $HOME, and a wrong
    // $HOME then unprotects the real ~/Library. Refusing a folder you named
    // Library is the smaller harm, so it is refused on purpose.
    let projects_library = fresh_root("projects_library").join("Projects/Library");
    write_files(&projects_library, &["notes.txt", "draft.txt"]);

    let err = scan::scan(&projects_library, &ScanConfig::default())
        .expect_err("a Library component is refused wherever it appears");
    assert!(matches!(err, scan::ScanError::RefusedSystemLocation(_)));

    fs::remove_dir_all(projects_library.parent().unwrap().parent().unwrap()).ok();
    fs::remove_dir_all(&home).ok();
}

#[test]
fn a_lied_about_home_cannot_unprotect_a_library() {
    let _g = lock();
    // $HOME points at one place; the Library we attack is somewhere else
    // entirely. This is the fail-open that a review caught before this shipped.
    let wrong_home = fresh_root("wrong_home");
    fs::create_dir_all(&wrong_home).expect("mkdir home");
    let _fake_home = FakeHome::set(&wrong_home);

    let victim = fresh_root("real_home").join("Library/Preferences");
    write_files(&victim, &["app.plist", "other.plist"]);

    let err = scan::scan(&victim, &ScanConfig::default())
        .expect_err("a Library must be refused even when $HOME points elsewhere");
    assert!(matches!(err, scan::ScanError::RefusedSystemLocation(_)));

    fs::remove_dir_all(victim.parent().unwrap().parent().unwrap()).ok();
    fs::remove_dir_all(&wrong_home).ok();
}
