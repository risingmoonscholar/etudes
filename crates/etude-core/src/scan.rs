//! Filesystem walk with the safety rules for untrusted trees.
//!
//! Metadata only. `scan` never opens a file for reading.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

/// Directory names that are never entered, regardless of depth or location.
/// A credential/noise directory is dangerous by NAME, wherever it appears.
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

/// Directory suffixes that are opaque units. Moved whole, never entered.
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

/// Suffixes a downloader writes while a transfer is still running.
///
/// These are already safe by accident -- none is in any type family, so
/// nothing groups them. Naming them explicitly means a future addition to the
/// extension table cannot silently start moving half-finished downloads, and
/// the plan can say WHY the file was left rather than reporting it as
/// unrecognised.
pub const IN_FLIGHT_SUFFIXES: &[&str] = &[
    ".part",
    ".crdownload",
    ".download",
    ".partial",
    ".opdownload",
    ".!ut",
];

/// Files whose presence makes the folder holding them a project.
///
/// A project file references its siblings by relative path: an .als expects
/// its bounces beside it, a .prproj expects its footage. Sorting those into
/// Media/ opens the project with everything offline.
///
/// This list is incomplete and always will be, and unlike the stoplist it
/// replaced that is safe: a missing entry fails to add a protection, leaving
/// the behaviour exactly as it was before the list existed. It never causes a
/// wrong grouping. Add freely.
///
/// What is deliberately NOT here: single-document formats that happen to be
/// authored in a creative app. A .sketch, .psd or .fig is a document, not a
/// project. People keep them in Downloads beside unrelated files, and one of
/// them freezing a whole folder is worse than the thing this guards against.
/// Found by the test fixture, which holds an .fig and a .sketch among a
/// hundred unrelated files and was refused wholesale.
///
/// The test is whether the format OWNS a directory layout. An .als expects
/// Samples/ beside it; a .sketch is one file that expects nothing.
const PROJECT_MARKERS: &[&str] = &[
    // --- Game engines -----------------------------------------------------
    // Verified against a real 18,724-file Godot project on the author's
    // machine. Its .tscn files reference siblings as res://scripts/main.gd --
    // absolute from the project root -- so moving ANY file inside a Godot
    // project breaks every reference to it. project.godot was missing from
    // the first version of this list, which is how that project came within
    // one `sweep ~/Documents/dev-projects/ad-astra` of losing its layout.
    "project.godot",
    ".uproject", // Unreal: sits beside Content/ Config/ Source/ Saved/
    ".unity",
    ".song", // Studio One: folder per song
    // --- Audio ------------------------------------------------------------
    // A session references its bounces, stems and samples by path.
    ".als",
    ".logicx",
    ".ptx",
    ".sesx",
    ".flp", // FL Studio, beside its rendered audio and sample references
    ".rpp",
    ".band",
    // --- Video ------------------------------------------------------------
    // A timeline references footage by path; sorting the footage takes every
    // clip offline.
    ".prproj",
    ".aep",
    ".drp",
    ".veg",
    ".fcpbundle",
    // --- 3D and compositing -----------------------------------------------
    // Blender's // paths are relative to the .blend, so a texture moved out
    // of its folder is a texture the file cannot find.
    ".blend",
    ".c4d",
    // --- Code projects, by their manifest ---------------------------------
    // A manifest names a directory layout, so the directory is the unit.
    "cargo.toml",
    "package.json",
    "pyproject.toml",
    "go.mod",
    "gemfile",
    "cmakelists.txt",
    "makefile",
];

/// Does this folder hold a project file at its top level?
///
/// Top level only, deliberately. A project file three directories down says
/// something about that directory, not about the one being scanned.
/// Does this name name a project? Dotted markers match as an extension, bare
/// markers as a whole filename.
///
/// Split out from `project_marker_in` because the same list has to answer two
/// questions, and answering only one of them was a real hole. Half the markers
/// name a *bundle* -- `.fcpbundle`, `.band`, `.logicx` are directories, and
/// the project IS the directory. Asking only "does this folder contain a
/// marker?" recognises `MyMovie.fcpbundle` sitting in Documents and misses it
/// when the user points sweep straight at it. Measured before this fix:
/// `sweep MyMovie.fcpbundle` grouped the library's three .mov files under
/// Media.
fn is_project_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    PROJECT_MARKERS.iter().any(|m| {
        if m.starts_with('.') {
            lower.ends_with(m)
        } else {
            lower == *m
        }
    })
}

fn project_marker_in(dir: &Path) -> Option<String> {
    let rd = fs::read_dir(dir).ok()?;
    for e in rd.flatten() {
        if is_project_name(&e.file_name().to_string_lossy()) {
            return Some(e.file_name().to_string_lossy().into_owned());
        }
    }
    None
}

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
    /// Leave files modified within this window alone. `None` disables it.
    ///
    /// Modified time, never accessed time. Spotlight, Time Machine and any
    /// backup agent touch atime just by looking, so a grace window keyed on
    /// atime would protect an entire folder forever after one reindex -- a
    /// guard that silently stops guarding. mtime changes when the person
    /// changes the file, which is the question being asked.
    pub grace: Option<std::time::Duration>,
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
    /// sweep leaves this off. It organises *files*. Moving a symlink versus
    /// its target are different operations its plan cannot express. stash turns
    /// it on, because "clear this folder" is meaningless if a directory stays
    /// behind. Added when stash became the second caller.
    pub whole_units: bool,
}

impl Default for ScanConfig {
    fn default() -> Self {
        Self {
            depth: 1,
            // A day. Long enough to cover "I downloaded this this morning
            // and I am still using it", short enough that yesterday's clutter
            // is fair game.
            grace: Some(std::time::Duration::from_secs(24 * 60 * 60)),
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
    /// The root is a project: a package directory, or a folder holding a
    /// project file at its top level. Its contents belong to one piece of
    /// work and reference each other by relative path.
    RefusedProjectRoot {
        root: PathBuf,
        marker: String,
    },
    RefusedRunningAsRoot,
    TooManyEntries {
        found: usize,
        cap: usize,
    },
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
                | ScanError::RefusedProjectRoot { .. }
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
            ScanError::RefusedProjectRoot { root, marker } => {
                // "X is in it" is false when the folder IS X. A refusal that
                // misdescribes what it saw teaches the user the wrong shape of
                // the rule, and they go looking for a stray file that is not
                // there.
                let itself = root
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().as_ref() == marker.as_str());
                let because = if itself {
                    "it is a project bundle".to_string()
                } else {
                    format!("{marker} is in it")
                };
                write!(
                    f,
                    "refused: {} looks like a project ({because}). Its files \
                     reference each other by relative path, so sorting them into type \
                     folders would break it. Sweep the folder that contains it instead",
                    crate::redact::path(root)
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
            // The OS text, not the Rust category. See ApplyError::Io.
            ScanError::Io(e) => write!(f, "io error: {e}"),
        }
    }
}

#[derive(Debug)]
pub struct ScanOutcome {
    pub root: PathBuf,
    /// The grace window this scan ran with, carried so the plan applies the
    /// same one. The scan reports what is there; deciding a file is too
    /// recent to move is a planning decision, and it belongs where the other
    /// leave-it-alone decisions are made.
    pub grace: Option<std::time::Duration>,
    pub entries: Vec<Entry>,
    /// Paths refused during the walk, for the "what was inspected" report.
    pub skipped_hidden: usize,
    pub skipped_symlink: usize,
    /// Entries refused by POLICY: a credential/noise directory by name
    /// (`.ssh`, `node_modules`, ...) or an absolute system location
    /// (`/System`, `~/Library`, ...). sweep could have looked and chose not
    /// to. This is restraint, not a failure.
    pub skipped_system: usize,
    /// Directories `read_dir` could not even open: permission denied, a
    /// path too long for the OS, or any other I/O failure. This is NOT a
    /// policy choice: sweep tried to look and could not. Unlike
    /// `skipped_system`, this means the directory's contents are completely
    /// unaccounted for and MUST be disclosed, or "Scanned N items" silently
    /// claims completeness it does not have.
    pub skipped_unreadable: usize,
    /// Directories not entered because they hold a project marker.
    ///
    /// Distinct from `skipped_system`: this is not policy about WHERE the
    /// directory is, it is a fact about what is in it. A folder holding a
    /// project file is one unit of work whose files reference each other, so
    /// sweep steps over it rather than into it.
    pub skipped_project: usize,
    /// True when the root sits inside a cloud-synced tree.
    pub root_is_synced: bool,
    /// The `allow_sync` this scan was actually run with. NOT derived from
    /// `root_is_synced`. A caller can pass `--allow-sync` on a root that
    /// turns out not to be synced at all. That consent must still be
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

/// Is this directory a package, asked of the OS?
///
/// macOS answers generically: a `.logicx` reports a content-type tree
/// containing `com.apple.package`, with no list involved. Measured, including
/// its limit -- a `.fcpbundle` reports a plain folder on a machine where Final
/// Cut is not installed, because the OS only knows formats some app
/// registered.
///
/// ADDITIVE, never a replacement. The suffix list is the machine-independent
/// floor; this widens coverage on machines that have the apps. Swapping the
/// list for this would fail open exactly where the user is least likely to
/// notice: a video project on a machine without the editor.
#[cfg(target_os = "macos")]
fn os_says_package(path: &Path) -> bool {
    use std::process::Command;
    Command::new("mdls")
        .args(["-name", "kMDItemContentTypeTree", "-raw"])
        .arg(path)
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("com.apple.package"))
        .unwrap_or(false)
}

#[cfg(not(target_os = "macos"))]
fn os_says_package(_path: &Path) -> bool {
    false
}

fn is_hidden(name: &str) -> bool {
    name.starts_with('.')
}

fn never_enter(name: &str) -> bool {
    NEVER_ENTER.iter().any(|d| d.eq_ignore_ascii_case(name))
}

/// Absolute system roots refused outright, regardless of `$HOME`. `/Library`
/// is the system-wide counterpart to `$HOME/Library` (LaunchDaemons, root-
/// owned Application Support, etc.). It is a different directory from any
/// user's home Library, but no less sensitive.
const SYSTEM_ROOTS: &[&str] = &["/System", "/Applications", "/Library"];

/// Whether `path` is a system location that is never organised.
///
/// This deliberately does NOT consult `$HOME`. An earlier version did, and it
/// meant a wrong `HOME` silently unprotected the real `~/Library`. The
/// environment became a safety input. It failed open. There is no way to
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

    // Refuse system locations outright, by absolute position. See
    // `is_refused_system_location`.
    // A project as the SCAN ROOT is the case depth alone cannot cover. Not
    // descending protects a project that sits inside the folder being swept;
    // it does nothing when the project IS that folder, which is what happens
    // when someone cd's into their track and runs the tidy tool. Its bounces
    // and renders are the immediate children then, and they would move.
    let root_name = root
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    if is_package(&root_name) || os_says_package(&root) {
        return Err(ScanError::RefusedProjectRoot {
            root: root.clone(),
            marker: "a package directory".to_string(),
        });
    }
    // The root is itself a project bundle. Being inside a project is being
    // inside a project regardless of which argument the user typed.
    if is_project_name(&root_name) {
        return Err(ScanError::RefusedProjectRoot {
            root: root.clone(),
            marker: root_name.clone(),
        });
    }
    if let Some(marker) = project_marker_in(&root) {
        return Err(ScanError::RefusedProjectRoot {
            root: root.clone(),
            marker,
        });
    }

    if is_refused_system_location(&root) {
        return Err(ScanError::RefusedSystemLocation(root.clone()));
    }

    let root_is_synced = is_synced(&root);
    if root_is_synced && !cfg.allow_sync {
        return Err(ScanError::RefusedSyncRoot(root.clone()));
    }

    let mut out = ScanOutcome {
        root: root.clone(),
        grace: cfg.grace,
        entries: Vec::new(),
        skipped_hidden: 0,
        skipped_symlink: 0,
        skipped_system: 0,
        skipped_project: 0,
        skipped_unreadable: 0,
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
    // Deterministic order. A plan must be byte-identical across runs.
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
            // A genuine I/O failure, not a policy refusal. sweep tried to
            // read this directory and could not. Counted separately from
            // `skipped_system` so a caller can tell "sweep declined to
            // look" apart from "sweep could not see". Its contents are
            // completely unaccounted for.
            out.skipped_unreadable += 1;
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

        // symlink_metadata does not follow. This is the TOCTOU-safe read.
        let meta = match fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };

        if meta.file_type().is_symlink() {
            if cfg.whole_units {
                // Move the link itself. No target is followed, so an escaping
                // or cyclic link is inert. It is just a small file to relocate.
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
            // A child directory holding a project marker is not entered. The
            // root check alone was not enough: it protects someone standing
            // IN their project, and does nothing for someone sweeping the
            // folder their projects live in with --depth. Verified before
            // this existed -- a Downloads folder holding a Godot project,
            // scanned at depth 2, produced an Images group of that project's
            // captures while its project.godot sat listed as ungrouped.
            //
            // Stepping over it, rather than refusing the whole scan, is the
            // difference between the root case and this one: the folder being
            // swept is ordinary and its own loose files are fair game. Only
            // the project inside it is off limits.
            if project_marker_in(&path).is_some() {
                out.skipped_project += 1;
                continue;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if !visited.insert((meta.dev(), meta.ino())) {
                    continue; // already seen: cycle
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

    // Every entry in both lists is tested by DERIVING the test from the list,
    // not by restating it. The project-file-extension space is large and
    // diverse -- one person's machine holds a fraction of it -- so the list is
    // the source of truth and these tests iterate it. Add a marker and it is
    // tested automatically; nobody has to remember. A marker that stops
    // working fails here, which is the failure the scenarios (only five types)
    // cannot see.

    #[test]
    fn every_project_marker_is_recognised_as_one() {
        use std::io::Write;
        for m in PROJECT_MARKERS {
            let dir = std::env::temp_dir().join(format!(
                "sweep_marker_{}_{}",
                m.trim_start_matches('.').replace(['.', '/'], "_"),
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("mkdir");
            // A dotted marker (.flp) is an extension; a bare one (cargo.toml)
            // is a whole filename. Build whichever this is.
            let fname = if m.starts_with('.') {
                format!("thing{m}")
            } else {
                (*m).to_string()
            };
            let mut f = fs::File::create(dir.join(&fname)).expect("write marker");
            let _ = f.write_all(b"x");

            assert_eq!(
                project_marker_in(&dir).as_deref(),
                Some(fname.as_str()),
                "PROJECT_MARKERS lists {m:?} but a folder holding {fname:?} was \
                 not recognised as a project. The list and the matcher have drifted"
            );
            let _ = fs::remove_dir_all(&dir);
        }
    }

    /// Half the markers name a directory, and the test above builds them all
    /// as files.
    ///
    /// A `.fcpbundle`, a `.band`, a `.logicx` IS the directory. The
    /// file-shaped test stayed green while `sweep MyMovie.fcpbundle` grouped
    /// the library's own media into a Media folder -- the oracle survived
    /// while its subject moved out from under it. This drives the real `scan`
    /// entry point, in both shapes, so neither can pass on the other's behalf.
    #[test]
    fn a_project_bundle_is_refused_as_a_directory_and_as_the_root() {
        for m in PROJECT_MARKERS.iter().filter(|m| m.starts_with('.')) {
            let base = std::env::temp_dir().join(format!(
                "sweep_bundle_{}_{}",
                m.trim_start_matches('.'),
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&base);
            let bundle = base.join(format!("thing{m}"));
            fs::create_dir_all(bundle.join("Media")).expect("mkdir bundle");
            fs::write(bundle.join("Media").join("a.mov"), b"x").expect("write");

            // Pointed straight at the bundle: it is the project.
            let inside = scan(&bundle, &ScanConfig::default());
            assert!(
                matches!(inside, Err(ScanError::RefusedProjectRoot { .. })),
                "scanning into a {m} bundle was not refused: {inside:?}"
            );

            // Pointed at the folder holding it: the bundle is a marker.
            let outside = scan(&base, &ScanConfig::default());
            assert!(
                matches!(outside, Err(ScanError::RefusedProjectRoot { .. })),
                "a folder holding a {m} bundle was not refused: {outside:?}"
            );

            let _ = fs::remove_dir_all(&base);
        }
    }

    #[test]
    fn every_package_suffix_is_recognised_as_one() {
        for suffix in PACKAGE_SUFFIXES {
            let name = format!("Thing{suffix}");
            assert!(
                is_package(&name),
                "PACKAGE_SUFFIXES lists {suffix:?} but is_package({name:?}) was false"
            );
            // Case-insensitively, since a real .APP or .App exists on disk.
            assert!(
                is_package(&name.to_uppercase()),
                "is_package is case-sensitive; {:?} was not recognised",
                name.to_uppercase()
            );
        }
    }

    #[test]
    fn a_marker_matches_only_as_a_whole_component() {
        // "makefile" must not match "notmakefile.txt", and ".flp" must not
        // match "flourish.flpng" -- the matcher checks the extension or the
        // whole filename, never a substring. Without this, a marker could
        // refuse folders it has no business refusing.
        let dir = std::env::temp_dir().join(format!("sweep_nomatch_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("mkdir");
        for innocent in ["notmakefile.txt", "cargo.tomlx", "my.flp.backup.zip"] {
            fs::write(dir.join(innocent), b"x").expect("write");
        }
        assert_eq!(
            project_marker_in(&dir),
            None,
            "a filename that merely contains a marker string triggered a refusal"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn is_refusal_separates_policy_from_io() {
        assert!(ScanError::RefusedRunningAsRoot.is_refusal());
        assert!(!ScanError::NotADirectory(PathBuf::from("/tmp/x")).is_refusal());
    }

    // is_refused_system_location is a pure function, tested directly against
    // synthetic absolute paths. No real /System, /Applications, or /Library
    // is ever touched. No real $HOME is ever used as the `home` argument.

    #[test]
    fn system_and_applications_and_library_stay_refused() {
        assert!(is_refused_system_location(Path::new(
            "/System/Library/CoreServices"
        )));
        assert!(is_refused_system_location(Path::new(
            "/Applications/Xcode.app"
        )));
        // The system-wide /Library, distinct from $HOME/Library. This is
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
        // `~/Library`. The environment becomes a safety input. It fails
        // open. Refusing to organise a folder you named Library is the smaller
        // harm. It is what happened before any of this was split apart.
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
        // No known $HOME (unset, empty, or relative: home_dir() returns
        // None for all three). There is no way to scope the iCloud
        // carve-out. This falls back to the pre-fix behaviour of
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
