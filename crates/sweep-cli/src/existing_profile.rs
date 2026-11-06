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

/// A private, fully validated replacement prepared before any moves. The
/// create-new file also serializes sweep writers; Drop removes it on failure.
pub struct PendingRoutes {
    temp: PathBuf,
    path: PathBuf,
    original: Option<Vec<u8>>,
    replaced: bool,
    pub added: Vec<(String, String)>,
}

impl Drop for PendingRoutes {
    fn drop(&mut self) {
        if !self.replaced {
            let _ = std::fs::remove_file(&self.temp);
        }
    }
}

impl PendingRoutes {
    pub fn prepare(
        root: &std::path::Path,
        maps: &[etude_core::plan::MapSpec],
    ) -> Result<Self, String> {
        Self::prepare_at(root, maps, profile_path())
    }

    pub(crate) fn prepare_at(
        root: &std::path::Path,
        maps: &[etude_core::plan::MapSpec],
        path: PathBuf,
    ) -> Result<Self, String> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let parent = path.parent().ok_or("profile has no parent folder")?;
        std::fs::create_dir_all(parent).map_err(|e| etude_cli_support::plain_io_error(&e))?;
        let temp = parent.join(".sweep-existing-profile.pending");
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600)
            .open(&temp).map_err(|e| format!("cannot prepare routes at {}: {}. Check that no other sweep is saving routes, then remove any leftover pending file.", etude_cli_support::shown(&temp), etude_cli_support::plain_io_error(&e)))?;
        let mut pending = Self {
            temp,
            path: path.clone(),
            original: None,
            replaced: false,
            added: Vec::new(),
        };
        let original = match std::fs::read(&path) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                return Err(format!(
                    "cannot read profile: {}",
                    etude_cli_support::plain_io_error(&e)
                ));
            }
        };
        let mut routes = match &original {
            Some(bytes) => parse(std::str::from_utf8(bytes).map_err(|_| "profile must be UTF-8")?)?,
            None => Vec::new(),
        };
        // Normalize existing extension spellings using the reader's validator.
        routes = etude_core::plan::validate_existing_maps(&routes, root)?
            .into_iter()
            .map(|map| (map.ext, map.folder))
            .collect();
        let mut added = Vec::new();
        for map in maps {
            if map.created {
                return Err(format!(
                    "--save-routes needs an existing direct visible folder; {:?} is missing. Create it first.",
                    map.folder
                ));
            }
            if let Some((_, folder)) = routes.iter().find(|(ext, _)| ext == &map.ext) {
                if folder != &map.folder {
                    return Err(format!(
                        ".{} already routes to {:?}; cannot save {:?}. Choose the existing route or edit the profile.",
                        map.ext, folder, map.folder
                    ));
                }
            } else {
                let route = (map.ext.clone(), map.folder.clone());
                routes.push(route.clone());
                added.push(route);
            }
        }
        etude_core::plan::validate_existing_maps(&routes, root)?;
        let source = encode(&routes)?;
        pending.original = original;
        pending.added = added;
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|e| etude_cli_support::plain_io_error(&e))?;
        file.write_all(source.as_bytes())
            .and_then(|_| file.sync_all())
            .map_err(|e| etude_cli_support::plain_io_error(&e))?;
        Ok(pending)
    }

    pub fn save(mut self) -> Result<(), String> {
        let path = &self.path;
        let current = match std::fs::read(path) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(etude_cli_support::plain_io_error(&e)),
        };
        if current != self.original {
            return Err("the profile changed during apply; routes were not saved".into());
        }
        std::fs::rename(&self.temp, path).map_err(|e| etude_cli_support::plain_io_error(&e))?;
        // The next writer may now own the pending name. Never remove its file.
        self.replaced = true;
        std::fs::File::open(path.parent().unwrap())
            .and_then(|file| file.sync_all())
            .map_err(|e| {
                format!(
                    "profile replaced, but its folder could not be synced: {}",
                    etude_cli_support::plain_io_error(&e)
                )
            })?;
        for (ext, folder) in &self.added {
            etude_cli_support::envelope::print(format!(
                "Saved route .{ext} → {}",
                etude_cli_support::shown_text(folder)
            ));
        }
        etude_cli_support::envelope::print(format!(
            "Routes profile: {}",
            etude_cli_support::shown(path)
        ));

        Ok(())
    }
}

fn encode(routes: &[(String, String)]) -> Result<String, String> {
    let quote = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
    let mut source = String::from("schema_version = 1\n");
    for (ext, folder) in routes {
        source.push_str(&format!(
            "\n[[routes]]\nextension = {}\nfolder = {}\n",
            quote(ext),
            quote(folder)
        ));
    }
    if parse(&source)? != routes {
        return Err("routes cannot round-trip through the profile reader".into());
    }
    Ok(source)
}

#[cfg(test)]
mod writer_tests {
    use super::*;
    #[test]
    fn writer_round_trips_quotes_and_rejects_controls() {
        let routes = vec![("bpy".into(), "Blender \"Files\"".into())];
        assert_eq!(parse(&encode(&routes).unwrap()).unwrap(), routes);
        assert!(encode(&[("bpy".into(), "bad\nfolder".into())]).is_err());
        assert!(encode(&[("bad\next".into(), "Box".into())]).is_err());
        let out = std::process::Command::new("mktemp")
            .arg("-d")
            .output()
            .unwrap();
        assert!(out.status.success());
        let base = PathBuf::from(String::from_utf8(out.stdout).unwrap().trim());
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(base.clone());
        let path = base.join("profile.toml");
        let mut first = PendingRoutes::prepare_at(&base, &[], path.clone()).unwrap();
        std::fs::rename(&first.temp, &path).unwrap();
        first.replaced = true;
        let second = PendingRoutes::prepare_at(&base, &[], path).unwrap();
        drop(first);
        assert!(
            second.temp.exists(),
            "a completed writer must not remove the next writer's temporary file"
        );
    }
}
