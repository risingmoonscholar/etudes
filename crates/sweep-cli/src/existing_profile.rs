//! Plain, user-editable routing rules for `--scheme existing`.

use std::path::PathBuf;

const FILE_NAME: &str = "sweep-existing-profile.toml";

#[derive(Debug, Clone)]
pub struct ExistingProfile {
    pub routes: Vec<(String, String)>,
    pub digest: String,
}

/// Why the profile could not be used. Each message names the file.
#[derive(Debug)]
pub enum ProfileError {
    Missing(String),
    Invalid(String),
}

/// Where the routes profile lives, honoring ETUDE_STATE_DIR.
pub fn profile_location() -> PathBuf {
    profile_path()
}

fn profile_path() -> PathBuf {
    etude_core::journal::state_dir().join(FILE_NAME)
}

/// A route the plan rejected, in terms of the profile rather than of `--map`,
/// which the person did not type.
pub fn route_problem(why: &str) -> String {
    let why = why.strip_prefix("--map: ").unwrap_or(why);
    format!(
        "the profile at {}: {why}. Edit that file.",
        etude_cli_support::shown(&profile_path())
    )
}

impl ExistingProfile {
    /// Read the profile without creating or changing it. Opting into this scheme
    /// never writes configuration as a side effect, and an absent profile is
    /// refused: the person asked for folders they have, and none are named.
    pub fn load() -> Result<Self, ProfileError> {
        let path = profile_path();
        let shown = etude_cli_support::shown(&path);
        let source = match std::fs::read_to_string(&path) {
            Ok(source) => source,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(ProfileError::Missing(format!(
                    "no profile at {shown}, so no folders of yours are in use. Add routes to that file (see README), or drop --scheme existing."
                )));
            }
            Err(error) => {
                return Err(ProfileError::Invalid(format!(
                    "cannot read the profile at {shown}: {}.",
                    etude_cli_support::plain_io_error(&error)
                )));
            }
        };
        let routes = parse(&source)
            .map_err(|why| ProfileError::Invalid(format!("the profile at {shown}, {why}")))?;
        let digest = etude_core::plan::binding_digest(source.as_bytes());
        Ok(Self { routes, digest })
    }
}

fn parse_string(value: &str) -> Result<String, String> {
    let Some(body) = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
    else {
        return Err("a route value must be a quoted string".into());
    };
    let mut out = String::new();
    let mut chars = body.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => match chars.next() {
                Some('\\') => out.push('\\'),
                Some('"') => out.push('"'),
                _ => return Err("a route value has an unsupported escape".into()),
            },
            '"' => return Err("a route value has an unescaped quote".into()),
            ch if ch.is_control() => {
                return Err("a route value has a control character".into());
            }
            ch => out.push(ch),
        }
    }
    Ok(out)
}

fn parse(source: &str) -> Result<Vec<(String, String)>, String> {
    let mut version = None;
    let mut routes = Vec::new();
    // A route being read, and the line its header is on.
    let mut current: Option<(usize, Option<String>, Option<String>)> = None;

    let finish_route = |current: &mut Option<(usize, Option<String>, Option<String>)>,
                        routes: &mut Vec<(String, String)>|
     -> Result<(), String> {
        if let Some((line, extension, folder)) = current.take() {
            let (Some(extension), Some(folder)) = (extension, folder) else {
                return Err(format!(
                    "line {line}: this route needs both extension and folder."
                ));
            };
            routes.push((extension, folder));
        }
        Ok(())
    };

    for (index, raw) in source.lines().enumerate() {
        let number = index + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == "[[routes]]" {
            if version != Some(1) {
                return Err(format!(
                    "line {number}: schema_version = 1 must come before any route."
                ));
            }
            finish_route(&mut current, &mut routes)?;
            current = Some((number, None, None));
            continue;
        }
        let (key, raw_value) = line
            .split_once('=')
            .ok_or_else(|| format!("line {number}: an entry must be key = value."))?;
        let key = key.trim();
        let raw_value = raw_value.trim();
        let Some(route) = current.as_mut() else {
            if key != "schema_version" || version.is_some() || raw_value != "1" {
                return Err(format!(
                    "line {number}: the file needs one schema_version = 1 entry before anything else."
                ));
            }
            version = Some(1);
            continue;
        };
        let value = parse_string(raw_value).map_err(|why| format!("line {number}: {why}."))?;
        let slot = match key {
            "extension" => &mut route.1,
            "folder" => &mut route.2,
            other => {
                return Err(format!(
                    "line {number}: unknown field \"{}\". Allowed: extension, folder.",
                    etude_cli_support::shown_text(other)
                ));
            }
        };
        if slot.replace(value).is_some() {
            return Err(format!(
                "line {number}: this route already has a value for {key}."
            ));
        }
    }
    finish_route(&mut current, &mut routes)?;
    if version != Some(1) {
        return Err("the file needs one schema_version = 1 entry.".into());
    }
    Ok(routes)
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn profile_is_plain_versioned_data_and_rejects_unknown_or_incomplete_fields() {
        assert_eq!(
            parse(
                "schema_version = 1\n[[routes]]\nextension = \"bpy\"\nfolder = \"Blender Files\"\n"
            )
            .unwrap(),
            vec![("bpy".into(), "Blender Files".into())]
        );
        assert!(parse("schema_version = 2\n").is_err());
        assert!(
            parse("schema_version = 1\n[[routes]]\nextension = \"bpy\"\nunknown = \"x\"\n")
                .is_err()
        );
        assert!(parse("schema_version = 1\n[[routes]]\nextension = \"bpy\"\n").is_err());
        assert_eq!(
            parse("schema_version = 1\n\n[[routes]]\ncolor = \"red\"\n").unwrap_err(),
            "line 4: unknown field \"color\". Allowed: extension, folder."
        );
    }
}

/// What kind of problem stopped routes from being prepared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrepareKind {
    /// The request itself conflicts with the saved profile.
    Arguments,
    /// A --map destination does not exist yet; `path` names it.
    MissingFolder,
    /// The profile exists but cannot be read or used as written.
    ProfileInvalid,
    /// The state folder cannot be written; `path` names it.
    Denied,
    NoSpace,
    WriteFailed,
}

/// Why routes could not be prepared. Nothing has moved when one is returned.
#[derive(Debug, PartialEq, Eq)]
pub struct PrepareError {
    pub kind: PrepareKind,
    /// What failed, as a sentence without its final period.
    pub what: String,
    /// One next step that works, as a sentence without its final period.
    pub next: String,
    pub path: Option<PathBuf>,
}

impl PrepareError {
    fn new(kind: PrepareKind, what: String, next: String) -> Self {
        Self {
            kind,
            what,
            next,
            path: None,
        }
    }
    fn at(mut self, path: &std::path::Path) -> Self {
        self.path = Some(path.to_path_buf());
        self
    }
}

/// Why prepared routes were not saved after a complete apply.
#[derive(Debug, PartialEq, Eq)]
pub enum SaveError {
    /// Someone changed the profile while files were moving.
    Changed,
    /// The profile could not be replaced.
    WriteFailed(String),
}

/// What a save did.
#[derive(Debug, PartialEq, Eq)]
pub struct Saved {
    /// False when every route was already saved, so the file was left alone.
    pub written: bool,
    /// Set when the profile was replaced but its folder could not be flushed.
    pub sync_problem: Option<String>,
}

const PENDING_PREFIX: &str = ".sweep-existing-profile.";
const PENDING_SUFFIX: &str = ".pending";

// Darwin SDK sys/fcntl.h. O_EXLOCK takes the lock in the same call that
// creates the file, so no other process can see it unlocked while its
// writer is alive.
#[cfg(target_os = "macos")]
const O_NOFOLLOW: i32 = 0x100;
#[cfg(target_os = "macos")]
const O_NONBLOCK: i32 = 0x4;
#[cfg(target_os = "macos")]
const O_EXLOCK: i32 = 0x20;

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn getuid() -> u32;
}

/// A private, fully validated profile prepared before any moves. Each writer
/// gets its own pending file, locked from the moment it exists; Drop removes
/// it on failure, before the lock is released.
pub struct PendingRoutes {
    temp: Option<(PathBuf, std::fs::File)>,
    path: PathBuf,
    original: Option<Vec<u8>>,
    replaced: bool,
    pub added: Vec<(String, String)>,
    pub requested: Vec<(String, String)>,
    /// Tests only: make the folder flush after the rename fail.
    #[cfg(test)]
    pub(crate) fail_folder_sync: bool,
}

impl Drop for PendingRoutes {
    fn drop(&mut self) {
        if !self.replaced
            && let Some((temp, _)) = &self.temp
        {
            let _ = std::fs::remove_file(temp);
        }
        // The file, and its lock, close after the name is gone.
        self.temp = None;
    }
}

fn prepare_io(what: String, dir: &std::path::Path, error: &std::io::Error) -> PrepareError {
    let what = format!("{what}: {}", etude_cli_support::plain_io_error(error));
    let shown_dir = etude_cli_support::shown(dir);
    let (kind, next) = match error.kind() {
        std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ReadOnlyFilesystem => (
            PrepareKind::Denied,
            format!(
                "Make it writable with chmod u+w -- {}, or drop --save-routes",
                etude_cli_support::quoted_path(dir)
            ),
        ),
        std::io::ErrorKind::StorageFull | std::io::ErrorKind::QuotaExceeded => (
            PrepareKind::NoSpace,
            "Free some space on that disk, or drop --save-routes".to_owned(),
        ),
        std::io::ErrorKind::NotADirectory | std::io::ErrorKind::AlreadyExists => (
            PrepareKind::WriteFailed,
            "A file is in the way of that folder. Move it aside, or drop --save-routes".to_owned(),
        ),
        _ => (
            PrepareKind::WriteFailed,
            format!("Check {shown_dir}, or drop --save-routes"),
        ),
    };
    PrepareError::new(kind, what, next).at(dir)
}

/// Remove pending files that a killed sweep left behind. A file is removed
/// only when it is provably sweep's: our name pattern, a regular file of this
/// user, and lockable, which means no live writer holds it. Anything else is
/// left alone; it cannot block this run, because every writer has its own name.
/// Only macOS can take the lock in the open call, so elsewhere nothing is
/// removed: without the lock a live writer's file would look like a leftover.
#[cfg(not(target_os = "macos"))]
fn remove_stale_pending(_dir: &std::path::Path) {}

#[cfg(target_os = "macos")]
fn remove_stale_pending(dir: &std::path::Path) {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let uid = unsafe { getuid() };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with(PENDING_PREFIX) || !name.ends_with(PENDING_SUFFIX) {
            continue;
        }
        let path = entry.path();
        let Ok(before) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if !before.file_type().is_file() || before.uid() != uid {
            continue;
        }
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        options.custom_flags(O_NOFOLLOW | O_NONBLOCK | O_EXLOCK);
        // Held by a live writer: the open would block, so it fails here.
        let Ok(file) = options.open(&path) else {
            continue;
        };
        let same = file
            .metadata()
            .is_ok_and(|now| now.ino() == before.ino() && now.dev() == before.dev());
        if same {
            let _ = std::fs::remove_file(&path);
        }
        drop(file);
    }
}

impl PendingRoutes {
    pub fn prepare(
        root: &std::path::Path,
        maps: &[etude_core::plan::MapSpec],
    ) -> Result<Self, PrepareError> {
        Self::prepare_at(root, maps, profile_path())
    }

    pub fn profile(&self) -> &std::path::Path {
        &self.path
    }

    pub(crate) fn prepare_at(
        root: &std::path::Path,
        maps: &[etude_core::plan::MapSpec],
        path: PathBuf,
    ) -> Result<Self, PrepareError> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let shown = etude_cli_support::shown(&path);
        let parent = path
            .parent()
            .ok_or_else(|| {
                PrepareError::new(
                    PrepareKind::WriteFailed,
                    format!("the routes profile {shown} has no folder"),
                    "Set ETUDE_STATE_DIR to a folder, or drop --save-routes".into(),
                )
            })?
            .to_path_buf();
        let requested: Vec<(String, String)> = maps
            .iter()
            .map(|map| (map.ext.clone(), map.folder.clone()))
            .collect();

        // Read and check the whole profile before writing anything.
        let original = match std::fs::read(&path) {
            Ok(bytes) => Some(bytes),
            // No profile can be there; making its folder below says why.
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) =>
            {
                None
            }
            Err(e) => {
                return Err(PrepareError::new(
                    PrepareKind::ProfileInvalid,
                    format!(
                        "cannot read the routes profile at {shown}: {}",
                        etude_cli_support::plain_io_error(&e)
                    ),
                    format!(
                        "Make it readable with chmod u+r -- {}, or drop --save-routes",
                        etude_cli_support::quoted_path(&path)
                    ),
                )
                .at(&path));
            }
        };
        let existing = match &original {
            Some(bytes) => {
                let invalid = |what: String| {
                    PrepareError::new(
                        PrepareKind::ProfileInvalid,
                        what,
                        "Edit that file, or drop --save-routes".into(),
                    )
                    .at(&path)
                };
                let text = std::str::from_utf8(bytes).map_err(|_| {
                    invalid(format!("the routes profile at {shown} is not UTF-8 text"))
                })?;
                let routes = parse(text).map_err(|why| {
                    invalid(format!(
                        "the routes profile at {shown}, {}",
                        why.trim_end_matches('.')
                    ))
                })?;
                // The whole file, but never its folders: a route saved for
                // another tree does not need its folder here.
                etude_core::plan::check_profile_routes(&routes).map_err(|why| {
                    invalid(format!(
                        "the routes profile at {shown}: {}",
                        plain_route_problem(&why)
                    ))
                })?
            }
            None => Vec::new(),
        };

        let mut added = Vec::new();
        for map in maps {
            let folder = etude_cli_support::shown_text(&map.folder);
            if map.created {
                let missing = root.join(&map.folder);
                return Err(PrepareError::new(
                    PrepareKind::MissingFolder,
                    format!(
                        "--save-routes saves only folders that already exist, and {} does not",
                        etude_cli_support::shown(&missing)
                    ),
                    format!(
                        "Create it first: mkdir -- {}",
                        etude_cli_support::quoted_path(&missing)
                    ),
                )
                .at(&missing));
            }
            if let Some((_, saved)) = existing.iter().find(|(ext, _)| ext == &map.ext) {
                if saved != &map.folder {
                    return Err(PrepareError::new(
                        PrepareKind::Arguments,
                        format!(
                            ".{} already routes to {} in the routes profile at {shown}, so {folder} cannot be saved",
                            map.ext,
                            etude_cli_support::shown_text(saved)
                        ),
                        "Use that folder, or edit the profile".into(),
                    ));
                }
            } else if let Some((ext, saved)) = existing
                .iter()
                .find(|(_, saved)| saved.eq_ignore_ascii_case(&map.folder))
            {
                return Err(PrepareError::new(
                    PrepareKind::Arguments,
                    format!(
                        "{} already holds the .{ext} route in the routes profile at {shown}, and each route needs its own folder",
                        etude_cli_support::shown_text(saved)
                    ),
                    "Use another folder, or edit the profile".into(),
                ));
            } else {
                added.push((map.ext.clone(), map.folder.clone()));
            }
        }
        // Only the routes being added are checked against this folder.
        etude_core::plan::validate_existing_maps(&added, root).map_err(|why| {
            PrepareError::new(
                PrepareKind::Arguments,
                format!(
                    "cannot save {}: {}",
                    route_list(&added),
                    plain_route_problem(&why)
                ),
                "Change the --map folder, or drop --save-routes".into(),
            )
        })?;
        let merged: Vec<(String, String)> = existing.iter().chain(&added).cloned().collect();
        etude_core::plan::check_profile_routes(&merged).map_err(|why| {
            PrepareError::new(
                PrepareKind::Arguments,
                format!(
                    "cannot add {} to the routes profile at {shown}: {}",
                    route_list(&added),
                    plain_route_problem(&why)
                ),
                "Use another folder, or edit the profile".into(),
            )
        })?;

        let mut pending = Self {
            temp: None,
            path: path.clone(),
            original,
            replaced: false,
            added,
            requested,
            #[cfg(test)]
            fail_folder_sync: false,
        };
        if pending.added.is_empty() {
            // Every route is already saved: leave the person's file alone.
            return Ok(pending);
        }
        // Appending keeps every byte the person wrote, comments included.
        let source = match &pending.original {
            Some(bytes) => append(std::str::from_utf8(bytes).unwrap_or_default(), &pending.added),
            None => encode(&pending.added),
        }
        .map_err(|why| {
            PrepareError::new(
                PrepareKind::ProfileInvalid,
                format!(
                    "the routes profile at {shown} cannot take {} without changing what it already says: {why}",
                    route_list(&pending.added)
                ),
                "Add the route to that file by hand, or drop --save-routes".into(),
            )
            .at(&path)
        })?;

        std::fs::create_dir_all(&parent).map_err(|e| {
            prepare_io(
                format!(
                    "cannot make the folder for the routes profile, {}",
                    etude_cli_support::shown(&parent)
                ),
                &parent,
                &e,
            )
        })?;
        remove_stale_pending(&parent);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let temp = parent.join(format!(
            "{PENDING_PREFIX}{}-{nanos}{PENDING_SUFFIX}",
            std::process::id()
        ));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true).mode(0o600);
        #[cfg(target_os = "macos")]
        options.custom_flags(O_NOFOLLOW | O_EXLOCK);
        let what = || {
            format!(
                "cannot write a new routes profile in {}",
                etude_cli_support::shown(&parent)
            )
        };
        let file = options
            .open(&temp)
            .map_err(|e| prepare_io(what(), &parent, &e))?;
        pending.temp = Some((temp, file));
        let (_, file) = pending.temp.as_mut().expect("just set");
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .and_then(|_| file.write_all(source.as_bytes()))
            .and_then(|_| file.sync_all())
            .map_err(|e| prepare_io(what(), &parent, &e))?;
        Ok(pending)
    }

    pub fn save(mut self) -> Result<Saved, SaveError> {
        let Some((temp, _)) = &self.temp else {
            return Ok(Saved {
                written: false,
                sync_problem: None,
            });
        };
        let temp = temp.clone();
        let parent = self
            .path
            .parent()
            .expect("checked in prepare")
            .to_path_buf();
        let io = |e: std::io::Error| SaveError::WriteFailed(etude_cli_support::plain_io_error(&e));
        // Hold the folder for the compare and the rename, so two sweeps saving
        // at once cannot both pass the check and lose one's routes.
        let folder = std::fs::File::open(&parent).map_err(io)?;
        folder.lock().map_err(io)?;
        let current = match std::fs::read(&self.path) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(io(e)),
        };
        if current != self.original {
            return Err(SaveError::Changed);
        }
        std::fs::rename(&temp, &self.path).map_err(io)?;
        // The pending name is gone; the file is now the profile.
        self.replaced = true;
        self.temp = None;
        let synced = folder.sync_all();
        #[cfg(test)]
        let synced = if self.fail_folder_sync {
            Err(std::io::Error::other("synthetic flush failure"))
        } else {
            synced
        };
        let sync_problem = synced.err().map(|e| etude_cli_support::plain_io_error(&e));
        drop(folder);
        Ok(Saved {
            written: true,
            sync_problem,
        })
    }
}

/// A plan validator's words, without the --map prefix the person did not
/// type here, and with folder names shown plainly rather than debug-quoted.
fn plain_route_problem(why: &str) -> String {
    let why = why.strip_prefix("--map: ").unwrap_or(why);
    let why = why.strip_prefix("existing-folder profile: ").unwrap_or(why);
    etude_cli_support::shown_text(why.trim_end_matches('.'))
}

fn route_list(routes: &[(String, String)]) -> String {
    routes
        .iter()
        .map(|(ext, folder)| format!(".{ext} → {}", etude_cli_support::shown_text(folder)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn route_block(ext: &str, folder: &str) -> String {
    let quote = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
    format!(
        "\n[[routes]]\nextension = {}\nfolder = {}\n",
        quote(ext),
        quote(folder)
    )
}

/// The lines that add one route, for a person to paste into the profile.
pub fn route_lines(ext: &str, folder: &str) -> String {
    route_block(ext, folder).trim().to_owned()
}

fn encode(routes: &[(String, String)]) -> Result<String, String> {
    append("schema_version = 1\n", routes)
}

/// The original text, unchanged, with the new routes after it. The reader
/// must see exactly the old routes followed by the new ones.
fn append(original: &str, routes: &[(String, String)]) -> Result<String, String> {
    let before = parse(original)?;
    let mut source = original.to_owned();
    if !source.is_empty() && !source.ends_with('\n') {
        source.push('\n');
    }
    for (ext, folder) in routes {
        source.push_str(&route_block(ext, folder));
    }
    let expected: Vec<(String, String)> =
        before.into_iter().chain(routes.iter().cloned()).collect();
    if parse(&source)? != expected {
        return Err("the routes do not read back the same".into());
    }
    Ok(source)
}

#[cfg(test)]
mod writer_tests {
    use super::*;

    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn temp_dir() -> (PathBuf, Cleanup) {
        let out = std::process::Command::new("mktemp")
            .arg("-d")
            .output()
            .unwrap();
        assert!(out.status.success());
        let base = PathBuf::from(String::from_utf8(out.stdout).unwrap().trim());
        (base.clone(), Cleanup(base))
    }
    fn map(ext: &str, folder: &str) -> etude_core::plan::MapSpec {
        etude_core::plan::MapSpec {
            ext: ext.into(),
            folder: folder.into(),
            created: false,
        }
    }
    fn pending_files(dir: &std::path::Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| n.ends_with(PENDING_SUFFIX))
            .collect()
    }

    #[test]
    fn writer_round_trips_quotes_and_rejects_controls() {
        let routes = vec![("bpy".into(), "Blender \"Files\"".into())];
        assert_eq!(parse(&encode(&routes).unwrap()).unwrap(), routes);
        assert!(encode(&[("bpy".into(), "bad\nfolder".into())]).is_err());
        assert!(encode(&[("bad\next".into(), "Box".into())]).is_err());
    }

    #[test]
    fn concurrent_writers_keep_their_own_pending_files() {
        let (base, _cleanup) = temp_dir();
        std::fs::create_dir(base.join("Box")).unwrap();
        std::fs::create_dir(base.join("Other")).unwrap();
        let path = base.join("state/profile.toml");
        let first = PendingRoutes::prepare_at(&base, &[map("bpy", "Box")], path.clone()).unwrap();
        let second =
            PendingRoutes::prepare_at(&base, &[map("xyz", "Other")], path.clone()).unwrap();
        assert_eq!(pending_files(&base.join("state")).len(), 2);
        first.save().unwrap();
        assert_eq!(
            pending_files(&base.join("state")).len(),
            1,
            "a completed writer must not remove the next writer's file"
        );
        // The second saw no profile; the first wrote one meanwhile.
        assert_eq!(second.save(), Err(SaveError::Changed));
        assert!(pending_files(&base.join("state")).is_empty());
    }

    // The lock that tells a leftover from a live file is macOS-only.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_killed_run_leftover_is_removed_but_a_held_file_is_not() {
        let (base, _cleanup) = temp_dir();
        std::fs::create_dir(base.join("Box")).unwrap();
        let state = base.join("state");
        std::fs::create_dir(&state).unwrap();
        let path = state.join("profile.toml");
        // What a killed run leaves: the name, no lock.
        std::fs::write(state.join(".sweep-existing-profile.pending"), b"x").unwrap();
        std::fs::write(state.join(".sweep-existing-profile.1-2.pending"), b"x").unwrap();
        // Not sweep's pattern: never touched.
        std::fs::write(state.join("notes.pending"), b"x").unwrap();
        let held = PendingRoutes::prepare_at(&base, &[map("bpy", "Box")], path.clone()).unwrap();
        let mut left = pending_files(&state);
        left.sort();
        let held_name = held
            .temp
            .as_ref()
            .unwrap()
            .0
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        assert_eq!(left, vec![held_name.clone(), "notes.pending".to_owned()]);
        // A live writer's file survives another writer's cleanup.
        let other = PendingRoutes::prepare_at(&base, &[map("bpy", "Box")], path).unwrap();
        assert!(state.join(&held_name).exists());
        drop(other);
        drop(held);
        assert_eq!(pending_files(&state), vec!["notes.pending".to_owned()]);
    }

    #[test]
    fn saved_routes_elsewhere_do_not_block_and_nothing_new_writes_nothing() {
        let (base, _cleanup) = temp_dir();
        std::fs::create_dir(base.join("Box")).unwrap();
        let path = base.join("profile.toml");
        let original = "# my notes\nschema_version = 1\n\n# from another folder\n[[routes]]\nextension = \"blendx\"\nfolder = \"Models\"\n";
        std::fs::write(&path, original).unwrap();
        // Models is not here, but only the new route is checked against this folder.
        let pending = PendingRoutes::prepare_at(&base, &[map("bpy", "Box")], path.clone()).unwrap();
        assert_eq!(pending.added, vec![("bpy".to_owned(), "Box".to_owned())]);
        assert!(pending.save().unwrap().written);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.starts_with(original),
            "the person's text is kept: {text}"
        );
        assert_eq!(parse(&text).unwrap().len(), 2);
        // Nothing new: the file is not rewritten.
        let before = std::fs::metadata(&path).unwrap();
        let pending = PendingRoutes::prepare_at(&base, &[map("bpy", "Box")], path.clone()).unwrap();
        assert!(pending.added.is_empty());
        assert!(!pending.save().unwrap().written);
        let after = std::fs::metadata(&path).unwrap();
        use std::os::unix::fs::MetadataExt;
        assert_eq!(
            (before.ino(), before.mtime_nsec()),
            (after.ino(), after.mtime_nsec())
        );
        assert!(pending_files(&base).is_empty());
    }

    #[test]
    fn prepare_errors_are_classified_and_name_folders_plainly() {
        let (base, _cleanup) = temp_dir();
        std::fs::create_dir(base.join("Box")).unwrap();
        std::fs::create_dir(base.join("Other")).unwrap();
        let path = base.join("profile.toml");
        std::fs::write(
            &path,
            "schema_version = 1\n[[routes]]\nextension = \"bpy\"\nfolder = \"Other\"\n",
        )
        .unwrap();
        let error = PendingRoutes::prepare_at(&base, &[map("bpy", "Box")], path.clone())
            .err()
            .unwrap();
        assert_eq!(error.kind, PrepareKind::Arguments, "{error:?}");
        let text = &error.what;
        assert!(
            text.contains("routes to Other") && text.contains("so Box cannot"),
            "{text}"
        );
        assert!(!text.contains('"'), "{text}");
        let mut missing = map("bpy", "New");
        missing.created = true;
        let error = PendingRoutes::prepare_at(&base, &[missing], path.clone())
            .err()
            .unwrap();
        assert_eq!(error.kind, PrepareKind::MissingFolder, "{error:?}");
        assert!(error.next.contains("mkdir -- "), "{}", error.next);
        assert!(!error.what.ends_with('.'));
        std::fs::write(&path, "schema_version = 2\n").unwrap();
        let error = PendingRoutes::prepare_at(&base, &[map("bpy", "Box")], path.clone())
            .err()
            .unwrap();
        assert_eq!(error.kind, PrepareKind::ProfileInvalid, "{error:?}");
        assert!(
            error.what.contains("schema_version = 1 entry"),
            "{}",
            error.what
        );
        assert!(pending_files(&base).is_empty());
    }
}
