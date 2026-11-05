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
