//! Filesystem walk with the safety rules for untrusted trees.
//!
//! Metadata only. `scan` never opens a file for reading.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

/// Directory names that are never entered, regardless of depth or location.
/// A credential/noise directory is dangerous by NAME, wherever it appears —
/// `.ssh` under a project checkout is still `.ssh`. This list must not carry
/// anything that is only dangerous because of WHERE it sits; that's what
/// `is_refused_system_location` is for. See the split's rationale below.
const NEVER_ENTER: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".aws",
    ".kube",
    ".docker",
    ".password-store",
    ".mozilla",
    ".config",
    "Keychains",
    ".Trash",
    "node_modules",
    ".git",
];

/// Directory suffixes that are opaque units — moved whole, never entered.
/// Walking into a `.photoslibrary` is both wrong and a privacy catastrophe.
const PACKAGE_SUFFIXES: &[&str] = &[
    ".app",
    ".rtfd",
    ".photoslibrary",
    ".sparsebundle",
    ".bundle",
    ".framework",
    ".pkg",
    ".xcodeproj",
    ".playground",
];

/// Roots under which cloud sync agents operate. Presence triggers a refusal
/// unless the caller opts in.
const SYNC_MARKERS: &[&str] = &[
    "Library/Mobile Documents",
    "iCloud Drive",
    "Dropbox",
    "OneDrive",
    "Google Drive",
    "Sync.com",
    "pCloud Drive",
];

#[derive(Debug, Clone)]
pub struct ScanConfig {
    /// Recursion depth. 1 means the directory itself only.
    pub depth: u8,
    /// Refuse rather than warn when the root is inside a sync root.
    pub allow_sync: bool,
    /// Hard ceiling on entries. Above this, sweep refuses rather than churn.
    pub max_entries: usize,
    /// Treat every top-level item as one opaque unit: directories are returned
    /// as entries instead of being descended into, and symlinks are returned as
    /// links rather than skipped.
    ///
    /// sweep leaves this off — it organises *files*, and moving a symlink versus
    /// its target are different operations its plan cannot express. stash turns
    /// it on, because "clear this folder" is meaningless if a directory stays
    /// behind. Added when stash became the second caller.
    pub whole_units: bool,
}

impl Default for ScanConfig {
    fn default() -> Self {
        Self {
            depth: 1,
            allow_sync: false,
            max_entries: 20_000,
            whole_units: false,
        }
    }
}

/// One item considered for organisation. A package directory is one entry, not
/// a subtree.
#[derive(Debug, Clone)]
pub struct Entry {
    pub path: PathBuf,
    /// File name as bytes-turned-string. Never logged unless explicitly asked.
    pub name: String,
    /// Lowercased extension without the dot, empty when absent.
    pub ext: String,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub is_dir: bool,
    /// True when this is an opaque package directory.
    pub is_package: bool,
}

#[derive(Debug)]
pub enum ScanError {
    NotADirectory(PathBuf),
    RefusedSystemLocation(PathBuf),
    RefusedSyncRoot(PathBuf),
    RefusedRunningAsRoot,
    TooManyEntries { found: usize, cap: usize },
    Io(io::Error),
}

impl ScanError {
    /// True for a deliberate safety refusal -- the scan could have
    /// proceeded but chose not to. False for a genuine I/O or input
    /// problem. Exit-code contract: refusals map to 2, everything else
    /// maps to 3 (see README's "Meaningful exit codes").
    pub fn is_refusal(&self) -> bool {
        matches!(
            self,
            ScanError::RefusedSystemLocation(_)
                | ScanError::RefusedSyncRoot(_)
                | ScanError::RefusedRunningAsRoot
                | ScanError::TooManyEntries { .. }
        )
    }
}

impl std::fmt::Display for ScanError {
    /// Paths are redacted here. Full paths appear only under `--explain`,
    /// which formats them deliberately. See THREAT-MODEL § T3.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScanError::NotADirectory(p) => {
                write!(f, "not a directory: {}", crate::redact::path(p))
            }
            ScanError::RefusedSystemLocation(p) => {
                write!(
                    f,
                    "refused: system or credential location {}",
                    crate::redact::path(p)
                )
            }
            ScanError::RefusedSyncRoot(p) => write!(
                f,
                "refused: {} is inside a cloud-synced folder; pass --allow-sync to override",
                crate::redact::path(p)
            ),
            ScanError::RefusedRunningAsRoot => {
                write!(f, "refused: will not run as root")
            }
            ScanError::TooManyEntries { found, cap } => {
                write!(f, "refused: {found} items exceeds the {cap} item cap")
            }
            ScanError::Io(e) => write!(f, "io error: {}", e.kind()),
        }
    }
}

#[derive(Debug)]
pub struct ScanOutcome {
    pub root: PathBuf,
    pub entries: Vec<Entry>,
    /// Paths refused during the walk, for the "what was inspected" report.
    pub skipped_hidden: usize,
    pub skipped_symlink: usize,
    pub skipped_system: usize,
    /// True when the root sits inside a cloud-synced tree.
    pub root_is_synced: bool,
    /// The `allow_sync` this scan was actually run with. NOT derived from
    /// `root_is_synced` — a caller can pass `--allow-sync` on a root that
    /// turns out not to be synced at all, and that consent must still be
    /// honoured later if a destination happens to look synced (e.g. a
    /// `sweep review` rename to a name that collides with a sync marker).
    /// `Plan::allow_sync` is copied from this field for exactly that reason.
    pub allow_sync: bool,
}

/// True when any component of `path` looks like a cloud-sync root.
pub fn is_synced(path: &Path) -> bool {
    let s = path.to_string_lossy();
    SYNC_MARKERS.iter().any(|m| s.contains(m))
}

fn is_package(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    PACKAGE_SUFFIXES.iter().any(|s| lower.ends_with(s))
}

fn is_hidden(name: &str) -> bool {
    name.starts_with('.')
}

fn never_enter(name: &str) -> bool {
    NEVER_ENTER.iter().any(|d| d.eq_ignore_ascii_case(name))
}

/// The user's home directory, if known and trustworthy. `Library` is only a
/// system location there — a folder named `Library` anywhere else is just a
/// folder named `Library`, and the check below must know the difference.
///
/// Returns `None` for anything that isn't a usable absolute path (`HOME`
/// unset, empty, or relative) rather than pretending an unusable value is
/// real. `is_refused_system_location` treats `None` as "cannot scope the
/// carve-out" and fails closed — this function must not paper over that by
/// handing back a home that can never match anything.
fn home_dir() -> Option<PathBuf> {
    let raw = std::env::var_os("HOME").map(PathBuf::from)?;
    if raw.as_os_str().is_empty() || !raw.is_absolute() {
        return None;
    }
    Some(raw.canonicalize().unwrap_or(raw))
}

/// Absolute system roots refused outright, regardless of `$HOME`. `/Library`
/// is the system-wide counterpart to `$HOME/Library` (LaunchDaemons, root-
/// owned Application Support, etc.) — a different directory from any user's
/// home Library, but no less sensitive.
const SYSTEM_ROOTS: &[&str] = &["/System", "/Applications", "/Library"];

/// True when `path` is a system location refused by absolute position
/// rather than by name: `/System`, `/Applications`, `/Library`, and
/// `$HOME/Library`.
///
/// `$HOME/Library/Mobile Documents` is carved back out: it is iCloud Drive,
/// which is user documents that Apple happens to park under `Library`. A
/// Mac with "Desktop & Documents" sync on has its real Desktop there, and
/// refusing it unconditionally means `--allow-sync` never gets a chance to
/// run for the one sync provider built into every Mac. The carve-out only
/// removes the system-location refusal — the path is still inside
/// `SYNC_MARKERS` territory and still needs `--allow-sync` to proceed.
///
/// The `$HOME/Library` check runs BEFORE the blunt `SYSTEM_ROOTS` prefixes on
/// purpose: `$HOME` is canonicalized once by `home_dir()`, but nothing
/// guarantees a future platform quirk (e.g. a filesystem view that presents
/// user directories under an OS-managed prefix) couldn't make a real home
/// path also start with one of `SYSTEM_ROOTS`. Checking the carve-out first
/// means that hypothetical can only ever widen what's reachable under a
/// user's own iCloud Drive, never accidentally narrow it.
///
/// Deliberately NOT name-based: `never_enter` already handles "dangerous by
/// name, anywhere" for credential directories. This handles "dangerous
/// because of where it sits", which is a different property. Conflating the
/// two is exactly what made iCloud Drive unreachable.
///
/// `home: None` (no trustworthy `$HOME` — unset, empty, or relative) fails
/// closed: refuse any `Library` component anywhere, exactly like before this
/// defect was split apart. Losing the iCloud carve-out in that case is the
/// safe trade — without a real home there is no way to tell iCloud Drive
/// apart from anywhere else, so the old blanket refusal is what's honest.
fn is_refused_system_location(path: &Path, home: Option<&Path>) -> bool {
    match home {
        Some(home) => {
            let library = home.join("Library");
            if path.starts_with(&library) {
                let icloud = library.join("Mobile Documents");
                return !path.starts_with(&icloud);
            }
        }
        None => {
            let has_library_component = path
                .components()
                .any(|c| matches!(c, Component::Normal(n) if n.eq_ignore_ascii_case("Library")));
            if has_library_component {
                return true;
            }
        }
    }
    SYSTEM_ROOTS.iter().any(|r| path.starts_with(r))
}

/// Walk `root` and return the entries eligible for organisation.
///
/// Safety rules enforced here:
/// - never descends into a hidden or credential-bearing directory
/// - never follows a symlink whose target escapes `root`
/// - treats package directories as single opaque entries
/// - caps depth and total entries
pub fn scan(root: &Path, cfg: &ScanConfig) -> Result<ScanOutcome, ScanError> {
    #[cfg(unix)]
    {
        // Ownership preservation would need root, and a filesystem-mutating tool
        // running as root is a blast radius nobody asked for.
        // SAFETY: getuid is always safe; it reads a process property.
        if unsafe { libc_getuid() } == 0 {
            return Err(ScanError::RefusedRunningAsRoot);
        }
    }

    let root = root.canonicalize().map_err(ScanError::Io)?;
    if !root.is_dir() {
        return Err(ScanError::NotADirectory(root));
    }

    // Refuse credential/noise directories outright, by name, anywhere in
    // the path.
    for comp in root.components() {
        if let Component::Normal(c) = comp {
            let s = c.to_string_lossy();
            if never_enter(&s) {
                return Err(ScanError::RefusedSystemLocation(root.clone()));
            }
        }
    }

    // Refuse system locations outright, by absolute position — see
    // `is_refused_system_location`.
    let home = home_dir();
    if is_refused_system_location(&root, home.as_deref()) {
        return Err(ScanError::RefusedSystemLocation(root.clone()));
    }

    let root_is_synced = is_synced(&root);
    if root_is_synced && !cfg.allow_sync {
        return Err(ScanError::RefusedSyncRoot(root.clone()));
    }

    let mut out = ScanOutcome {
        root: root.clone(),
        entries: Vec::new(),
        skipped_hidden: 0,
        skipped_symlink: 0,
        skipped_system: 0,
        root_is_synced,
        allow_sync: cfg.allow_sync,
    };

    // Visited device+inode pairs close the symlink-cycle case.
    let mut visited: HashSet<(u64, u64)> = HashSet::new();
    walk(
        &root,
        &root,
        0,
        cfg,
        home.as_deref(),
        &mut out,
        &mut visited,
    )?;

    if out.entries.len() > cfg.max_entries {
        return Err(ScanError::TooManyEntries {
            found: out.entries.len(),
            cap: cfg.max_entries,
        });
    }
    // Deterministic order — a plan must be byte-identical across runs.
    out.entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

fn walk(
    root: &Path,
    dir: &Path,
    depth: u8,
    cfg: &ScanConfig,
    home: Option<&Path>,
    out: &mut ScanOutcome,
    visited: &mut HashSet<(u64, u64)>,
) -> Result<(), ScanError> {
    if depth >= cfg.depth.min(8) {
        return Ok(());
    }

    let rd = match fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(_) => {
            out.skipped_system += 1;
            return Ok(());
        }
    };

    for item in rd {
        let item = match item {
            Ok(i) => i,
            Err(_) => continue,
        };
        let path = item.path();
        let name = item.file_name().to_string_lossy().into_owned();

        // symlink_metadata does not follow — this is the TOCTOU-safe read.
        let meta = match fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };

        if meta.file_type().is_symlink() {
            if cfg.whole_units {
                // Move the link itself. No target is followed, so an escaping
                // or cyclic link is inert — it is just a small file to relocate.
                out.entries.push(Entry {
                    path,
                    name,
                    ext: String::new(),
                    size: meta.len(),
                    modified: meta.modified().ok(),
                    is_dir: false,
                    is_package: false,
                });
                continue;
            }
            // A symlink is followed only if its target stays inside root.
            match path.canonicalize() {
                Ok(target) if target.starts_with(root) && target != *root => {
                    // In-root symlink: still skipped in v0.1. Moving a symlink
                    // and moving its target are different operations and the
                    // plan cannot express the difference yet.
                    out.skipped_symlink += 1;
                }
                _ => out.skipped_symlink += 1,
            }
            continue;
        }

        if is_hidden(&name) {
            out.skipped_hidden += 1;
            continue;
        }
        if never_enter(&name) {
            out.skipped_system += 1;
            continue;
        }
        if is_refused_system_location(&path, home) {
            out.skipped_system += 1;
            continue;
        }

        let is_dir = meta.is_dir();
        let pkg = is_dir && (is_package(&name) || cfg.whole_units);

        if is_dir && !pkg {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if !visited.insert((meta.dev(), meta.ino())) {
                    continue; // already seen — cycle
                }
            }
            walk(root, &path, depth + 1, cfg, home, out, visited)?;
            continue;
        }

        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();

        out.entries.push(Entry {
            path,
            name,
            ext,
            size: meta.len(),
            modified: meta.modified().ok(),
            is_dir,
            is_package: pkg,
        });
    }
    Ok(())
}

#[cfg(unix)]
unsafe extern "C" {
    #[link_name = "getuid"]
    fn libc_getuid() -> u32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_refusal_separates_policy_from_io() {
        assert!(ScanError::RefusedRunningAsRoot.is_refusal());
        assert!(!ScanError::NotADirectory(PathBuf::from("/tmp/x")).is_refusal());
    }

    // is_refused_system_location is a pure function, tested directly against
    // synthetic absolute paths — no real /System, /Applications, or /Library
    // is ever touched, and no real $HOME is ever used as the `home` argument.

    #[test]
    fn system_and_applications_and_library_stay_refused() {
        let home = Path::new("/Users/fixture");
        assert!(is_refused_system_location(
            Path::new("/System/Library/CoreServices"),
            Some(home)
        ));
        assert!(is_refused_system_location(
            Path::new("/Applications/Xcode.app"),
            Some(home)
        ));
        // The system-wide /Library, distinct from $HOME/Library — this is
        // the gap an adversarial review caught: narrowing Library's refusal
        // to $HOME/Library alone silently reopened the system-wide one.
        assert!(is_refused_system_location(
            Path::new("/Library/LaunchDaemons"),
            Some(home)
        ));
    }

    #[test]
    fn home_library_is_refused_except_the_icloud_carve_out() {
        let home = Path::new("/Users/fixture");
        assert!(is_refused_system_location(
            &home.join("Library/Preferences"),
            Some(home)
        ));
        assert!(!is_refused_system_location(
            &home.join("Library/Mobile Documents/com~apple~CloudDocs/Desktop"),
            Some(home)
        ));
    }

    #[test]
    fn a_folder_merely_named_library_elsewhere_is_not_refused() {
        let home = Path::new("/Users/fixture");
        assert!(!is_refused_system_location(
            Path::new("/Users/fixture/Projects/Library"),
            Some(home)
        ));
    }

    #[test]
    fn without_a_trustworthy_home_library_is_refused_anywhere_fail_closed() {
        // No known $HOME (unset, empty, or relative — home_dir() returns
        // None for all three): there is no way to scope the iCloud
        // carve-out, so this falls back to the pre-fix behaviour of
        // refusing any `Library` component, anywhere. This is what an
        // adversarial review caught: `env -u HOME sweep ~/Library/Preferences`
        // must not silently drop the Library guard.
        assert!(is_refused_system_location(
            Path::new("/Volumes/External/Library"),
            None
        ));
        assert!(is_refused_system_location(
            Path::new("/Users/fixture/Library/Preferences"),
            None
        ));
        // Case-insensitive, matching never_enter's own matching.
        assert!(is_refused_system_location(
            Path::new("/Users/fixture/library/Preferences"),
            None
        ));
        // A folder that merely CONTAINS "Library" as a substring, not as a
        // whole path component, is not a false positive.
        assert!(!is_refused_system_location(
            Path::new("/Users/fixture/LibraryOfBabel"),
            None
        ));
    }
}
