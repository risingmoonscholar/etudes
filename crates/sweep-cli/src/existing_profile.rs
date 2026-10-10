//! Plain, user-editable routing rules for `--scheme existing`.

use std::path::PathBuf;

const FILE_NAME: &str = "sweep-existing-profile.toml";
const EMPTY_PROFILE: &str = "schema_version = 1\n";

#[derive(Debug, Clone)]
pub struct ExistingProfile {
    pub routes: Vec<(String, String)>,
    pub digest: String,
}

impl ExistingProfile {
    /// Read the profile without creating or changing it. A missing profile is
    /// the valid empty version-1 profile, so opting into this scheme never
    /// writes configuration as a side effect.
    pub fn load() -> Result<Self, String> {
        let path: PathBuf = etude_core::journal::state_dir().join(FILE_NAME);
        let source = match std::fs::read_to_string(&path) {
            Ok(source) => source,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => EMPTY_PROFILE.into(),
            Err(_) => return Err("could not read the existing-folder profile".into()),
        };
        let routes = parse(&source)?;
        let digest = etude_core::plan::binding_digest(source.as_bytes());
        Ok(Self { routes, digest })
    }
}

fn parse_string(value: &str) -> Result<String, String> {
    let Some(body) = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
    else {
        return Err("profile route values must be quoted strings".into());
    };
    let mut out = String::new();
    let mut chars = body.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => match chars.next() {
                Some('\\') => out.push('\\'),
                Some('"') => out.push('"'),
                _ => return Err("profile route contains an unsupported escape".into()),
            },
            '"' => return Err("profile route contains an unescaped quote".into()),
            ch if ch.is_control() => {
                return Err("profile route contains a control character".into());
            }
            ch => out.push(ch),
        }
    }
    Ok(out)
}

fn parse(source: &str) -> Result<Vec<(String, String)>, String> {
    let mut version = None;
    let mut routes = Vec::new();
    let mut current: Option<(Option<String>, Option<String>)> = None;

    let finish_route = |current: &mut Option<(Option<String>, Option<String>)>,
                        routes: &mut Vec<(String, String)>|
     -> Result<(), String> {
        if let Some((extension, folder)) = current.take() {
            let (Some(extension), Some(folder)) = (extension, folder) else {
                return Err("each profile route needs both extension and folder".into());
            };
            routes.push((extension, folder));
        }
        Ok(())
    };

    for raw in source.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == "[[routes]]" {
            if version != Some(1) {
                return Err("profile must declare schema_version = 1 before routes".into());
            }
            finish_route(&mut current, &mut routes)?;
            current = Some((None, None));
            continue;
        }
        let (key, raw_value) = line
            .split_once('=')
            .ok_or_else(|| "profile entries must use key = value".to_string())?;
        let key = key.trim();
        let raw_value = raw_value.trim();
        if current.is_none() {
            if key != "schema_version" || version.is_some() || raw_value != "1" {
                return Err("profile requires one schema_version = 1 entry".into());
            }
            version = Some(1);
            continue;
        }
        let route = current.as_mut().expect("route section was opened");
        let value = parse_string(raw_value)?;
        let slot = match key {
            "extension" => &mut route.0,
            "folder" => &mut route.1,
            _ => return Err("profile route has an unknown field".into()),
        };
        if slot.replace(value).is_some() {
            return Err("profile route repeats a field".into());
        }
    }
    finish_route(&mut current, &mut routes)?;
    if version != Some(1) {
        return Err("profile requires schema_version = 1".into());
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
    }
}
