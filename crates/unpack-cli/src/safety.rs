//! Deciding whether an archive is safe to extract, from its listing alone.
//!
//! The order matters and is the whole design: **list, judge, then extract**.
//! Extracting first and cleaning up afterwards means a hostile archive has
//! already written into the target and the user has to clean up after it.
//!
//! # What is NOT protected against
//!
//! **Decompression bombs.** A 42 KB zip expanding to petabytes will fill the
//! disk. Catching it needs uncompressed sizes from the listing (`unzip -Z`,
//! `tar -tvf`) and a ratio check, and that is not implemented. The entry-count
//! cap below is the only size-shaped limit that exists. Said plainly here
//! rather than implied by a constant nobody reads.

/// Most entries accepted, so a million-file archive cannot stall the run.
pub const MAX_ENTRIES: usize = 200_000;

#[derive(Debug, PartialEq, Eq)]
pub enum Unsafe {
    /// `/etc/passwd`: would write outside the target entirely.
    AbsolutePath(String),
    /// `../../.ssh/authorized_keys`: the classic Zip Slip.
    Escapes(String),
    /// Windows drive letters and UNC paths.
    DriveOrUnc(String),
}

impl std::fmt::Display for Unsafe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unsafe::AbsolutePath(p) => write!(f, "absolute path: {p}"),
            Unsafe::Escapes(p) => write!(f, "escapes the target: {p}"),
            Unsafe::DriveOrUnc(p) => write!(f, "drive or UNC path: {p}"),
        }
    }
}

/// Judge one archive member path.
///
/// Purely lexical: it never touches the filesystem, so it cannot be defeated by
/// anything that changes on disk between the check and the extraction.
pub fn judge(path: &str) -> Option<Unsafe> {
    let p = path.trim();
    if p.is_empty() {
        return None;
    }
    if p.starts_with('/') {
        return Some(Unsafe::AbsolutePath(p.to_string()));
    }
    // C:\ or \\server\share
    let bytes = p.as_bytes();
    if p.starts_with("\\\\")
        || (bytes.len() > 2 && bytes[1] == b':' && (bytes[2] == b'\\' || bytes[2] == b'/'))
    {
        return Some(Unsafe::DriveOrUnc(p.to_string()));
    }

    // Walk the components and track depth. Depth below zero at any point means
    // the entry reaches above the extraction root, even if it comes back down.
    let mut depth: i32 = 0;
    for comp in p.split(['/', '\\']) {
        match comp {
            "" | "." => {}
            ".." => {
                depth -= 1;
                if depth < 0 {
                    return Some(Unsafe::Escapes(p.to_string()));
                }
            }
            _ => depth += 1,
        }
    }
    None
}

/// Should this entry be dropped rather than extracted?
///
/// Archive cruft that is never wanted and clutters every extraction.
pub fn is_junk(path: &str) -> bool {
    path.split('/')
        .any(|c| c == "__MACOSX" || c == ".DS_Store" || c == "Thumbs.db" || c.starts_with("._"))
}

/// Given every member path, find the single wrapper directory to strip.
///
/// Returns `Some(name)` only when *every* non-junk entry sits under one common
/// top-level directory. That is the "conference-assets.zip containing
/// conference-assets/" case, which otherwise gives you
/// `conference-assets/conference-assets/`.
pub fn wrapper_dir(paths: &[String]) -> Option<String> {
    // An archive listing contains bare directory entries as well as file
    // entries: `conference-assets` sits alongside `conference-assets/notes.md`.
    // A top-level entry is a DIRECTORY when some other entry lives under it,
    // and only a top-level *file* rules out a wrapper. Without this, every
    // listing that names its own directories looked like a loose file and
    // nothing was ever flattened.
    let is_dir_entry = |e: &str| {
        let prefix = format!("{e}/");
        paths.iter().any(|o| o.starts_with(&prefix))
    };

    let mut top: Option<String> = None;
    let mut saw_any = false;
    for p in paths.iter().filter(|p| !is_junk(p)) {
        let clean = p.trim_start_matches("./");
        let first = clean.split('/').next()?.to_string();
        if first.is_empty() {
            return None;
        }
        // A bare file at top level means there is no single wrapper.
        if !clean.contains('/') && !is_dir_entry(clean) {
            return None;
        }
        saw_any = true;
        match &top {
            None => top = Some(first),
            Some(t) if *t == first => {}
            Some(_) => return None,
        }
    }
    if saw_any { top } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zip_slip_is_refused() {
        // The canonical CVE class. Every variant must be caught lexically.
        assert!(matches!(
            judge("../../../etc/passwd"),
            Some(Unsafe::Escapes(_))
        ));
        assert!(matches!(judge("a/../../b"), Some(Unsafe::Escapes(_))));
        assert!(matches!(
            judge("..\\..\\windows\\system32"),
            Some(Unsafe::Escapes(_))
        ));
        assert!(matches!(
            judge("/etc/passwd"),
            Some(Unsafe::AbsolutePath(_))
        ));
        assert!(matches!(
            judge("C:\\Windows\\evil.dll"),
            Some(Unsafe::DriveOrUnc(_))
        ));
        assert!(matches!(
            judge("\\\\server\\share\\x"),
            Some(Unsafe::DriveOrUnc(_))
        ));
    }

    #[test]
    fn a_path_that_dips_and_returns_is_still_refused() {
        // "a/../b" is fine. It never leaves. "a/../../b" is not.
        assert_eq!(judge("a/../b"), None, "a harmless path was refused");
        assert!(judge("a/../../b").is_some(), "an escaping path was allowed");
    }

    #[test]
    fn ordinary_paths_pass() {
        assert_eq!(judge("assets/logo.png"), None);
        assert_eq!(judge("./README.md"), None);
        assert_eq!(judge("deep/nested/dir/file.txt"), None);
    }

    #[test]
    fn mac_and_windows_cruft_is_recognised() {
        assert!(is_junk("__MACOSX/._file"));
        assert!(is_junk("a/b/.DS_Store"));
        assert!(is_junk("._resourcefork"));
        assert!(!is_junk("assets/real-file.png"));
    }

    #[test]
    fn a_real_zip_listing_with_bare_directory_entries_is_flattened() {
        // Regression: `unzip -Z1` emits directory entries too. Treating
        // `conference-assets` as a loose top-level file meant nothing was ever
        // flattened, and every extraction nested one level too deep.
        let paths: Vec<String> = [
            "conference-assets",
            "conference-assets/.DS_Store",
            "conference-assets/img",
            "conference-assets/img/logo.png",
            "conference-assets/notes.md",
            "__MACOSX",
            "__MACOSX/._notes.md",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(wrapper_dir(&paths), Some("conference-assets".into()));
    }

    #[test]
    fn a_single_wrapper_directory_is_detected() {
        let paths: Vec<String> = ["proj/a.txt", "proj/b/c.txt", "__MACOSX/._proj"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(wrapper_dir(&paths), Some("proj".into()));
    }

    #[test]
    fn two_top_level_entries_are_not_a_wrapper() {
        let paths: Vec<String> = ["a/x.txt", "b/y.txt"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(wrapper_dir(&paths), None, "flattening here would collide");
    }

    #[test]
    fn a_loose_file_at_top_level_means_no_wrapper() {
        // Stripping here would drop README.md into the parent directory.
        // That is exactly the "exploding into the current directory" this
        // prevents.
        let paths: Vec<String> = ["proj/a.txt", "README.md"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(wrapper_dir(&paths), None);
    }
}
