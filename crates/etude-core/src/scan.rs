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

/// Absolute system roots refused outright, regardless of `$HOME`. `/Library`
/// is the system-wide counterpart to `$HOME/Library` (LaunchDaemons, root-
/// owned Application Support, etc.) — a different directory from any user's
/// home Library, but no less sensitive.
const SYSTEM_ROOTS: &[&str] = &["/System", "/Applications", "/Library"];

/// Whether `path` is a system location that is never organised.
///
/// This deliberately does NOT consult `$HOME`. An earlier version did, and it
/// meant a wrong `HOME` silently unprotected the real `~/Library` — the
/// environment became a safety input, and it failed open. There is no way to
/// validate `HOME` from inside the process, so the honest fix is not to need it.
///
/// The rule instead: a `Library` component is refused wherever it appears,
/// unless it is immediately followed by `Mobile Documents`. That pair is
/// unmistakably iCloud Drive, which is user documents that Apple happens to
/// store under Library, and it identifies itself without anyone having to say
/// where home is.
///
/// The cost is that a folder of your own named `Library` is refused, which is
/// what happened before this was ever split apart. Refusing to organise a
/// folder is a small harm; organising `~/Library` because an environment
/// variable lied is not.
fn is_refused_system_location(path: &Path) -> bool {
    if SYSTEM_ROOTS.iter().any(|r| path.starts_with(r)) {
        return true;
    }
    let comps: Vec<_> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(n) => Some(n.to_string_lossy().to_string()),
            _ => None,
        })
        .collect();
    for (i, c) in comps.iter().enumerate() {
        if c.eq_ignore_ascii_case("Library") {
            let is_icloud = comps
                .get(i + 1)
                .is_some_and(|n| n.eq_ignore_ascii_case("Mobile Documents"));
            if !is_icloud {
                return true;
            }
        }
    }
    false
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
    if is_refused_system_location(&root) {
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
    walk(&root, &root, 0, cfg, &mut out, &mut visited)?;

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
        if is_refused_system_location(&path) {
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
            walk(root, &path, depth + 1, cfg, out, visited)?;
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
        assert!(is_refused_system_location(Path::new(
            "/System/Library/CoreServices"
        )));
        assert!(is_refused_system_location(Path::new(
            "/Applications/Xcode.app"
        )));
        // The system-wide /Library, distinct from $HOME/Library — this is
        // the gap an adversarial review caught: narrowing Library's refusal
        // to $HOME/Library alone silently reopened the system-wide one.
        assert!(is_refused_system_location(Path::new(
            "/Library/LaunchDaemons"
        )));
    }

    #[test]
    fn a_home_library_is_refused_except_the_icloud_carve_out() {
        let home = Path::new("/Users/fixture");
        assert!(is_refused_system_location(
            &home.join("Library/Preferences")
        ));
        assert!(!is_refused_system_location(
            &home.join("Library/Mobile Documents/com~apple~CloudDocs/Desktop")
        ));
    }

    #[test]
    fn a_folder_of_your_own_named_library_is_refused_and_that_is_the_trade() {
        // Deliberate. Telling `~/Projects/Library` apart from `~/Library` needs
        // to know where home is, and the only way to ask is `$HOME`, which a
        // caller can lie about. A wrong `HOME` then unprotects the real
        // `~/Library` — the environment becomes a safety input and it fails
        // open. Refusing to organise a folder you named Library is the smaller
        // harm, and it is what happened before any of this was split apart.
        assert!(is_refused_system_location(Path::new(
            "/Users/fixture/Projects/Library"
        )));
    }

    #[test]
    fn icloud_drive_is_reachable_without_knowing_where_home_is() {
        // `Library/Mobile Documents` as consecutive components identifies iCloud
        // Drive on its own. No `$HOME` required, so no `$HOME` to be wrong.
        assert!(!is_refused_system_location(Path::new(
            "/anywhere/at/all/Library/Mobile Documents/com~apple~CloudDocs/Desktop"
        )));
        assert!(is_refused_system_location(Path::new(
            "/anywhere/at/all/Library/Preferences"
        )));
    }

    #[test]
    fn a_lied_about_home_cannot_unprotect_a_library() {
        // The regression this replaced: the check took a home argument, and
        // when it pointed somewhere else a Library path sailed straight
        // through. There is no argument to get wrong now, which is the fix.
        assert!(is_refused_system_location(Path::new(
            "/Users/fixture/Library/Preferences"
        )));
        assert!(is_refused_system_location(Path::new(
            "/Users/somebody-else/Library/Preferences"
        )));
    }

    #[test]
    fn without_a_trustworthy_home_library_is_refused_anywhere_fail_closed() {
        // No known $HOME (unset, empty, or relative — home_dir() returns
        // None for all three): there is no way to scope the iCloud
        // carve-out, so this falls back to the pre-fix behaviour of
        // refusing any `Library` component, anywhere. This is what an
        // adversarial review caught: `env -u HOME sweep ~/Library/Preferences`
        // must not silently drop the Library guard.
        assert!(is_refused_system_location(Path::new(
            "/Volumes/External/Library"
        )));
        assert!(is_refused_system_location(Path::new(
            "/Users/fixture/Library/Preferences"
        )));
        // Case-insensitive, matching never_enter's own matching.
        assert!(is_refused_system_location(Path::new(
            "/Users/fixture/library/Preferences"
        )));
        // A folder that merely CONTAINS "Library" as a substring, not as a
        // whole path component, is not a false positive.
        assert!(!is_refused_system_location(Path::new(
            "/Users/fixture/LibraryOfBabel"
        )));
    }
}
