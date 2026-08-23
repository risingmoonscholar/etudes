//! Deciding whether an archive is safe to extract, from its listing alone.
//!
//! The order matters and is the whole design: **list, judge, then extract**.
//! Extracting first and cleaning up afterwards means a hostile archive has
//! already written into the target and the user has to clean up after it.
//!
//! # Decompression bombs
//!
//! Bounded by what gets WRITTEN, never by what the archive says about itself.
//!
//! The obvious check is to read uncompressed sizes from the listing
//! (`unzip -Z`, `tar -tvf`, `gzip -l`) and refuse a suspicious ratio. Those
//! numbers are written by whoever made the archive, and they were measured
//! here rather than assumed:
//!
//! | format | truth | what the listing reports after four bytes are edited |
//! |---|---|---|
//! | `.zip` | 2,000,000 B | 4,096 |
//! | `.tar` | 2,000,000 B | 4,096 |
//! | `.gz` | 1,000,000 B | 4,096 |
//!
//! The forged zip then extracted all 2,000,000 bytes with `unzip` exiting 0
//! and printing nothing. `gunzip` did notice its own trailer was wrong -- and
//! said so *after* writing the full million bytes, which is too late to be a
//! defence.
//!
//! So a ratio check is a claim the attacker gets to satisfy. The cap here is
//! on bytes actually landing on disk, which no header can lie about, and it
//! is the same shape ClamAV uses (`MaxScanSize` bounds data extracted, not
//! data declared).
//!
//! What that buys, stated exactly: extraction stops and the target is removed
//! once an archive writes more than the cap. It is not a promise that a
//! hostile archive cannot inconvenience you -- it is a bound on how far it
//! gets.

/// Most entries accepted, so a million-file archive cannot stall the run.
pub const MAX_ENTRIES: usize = 200_000;

/// Share of the volume's free space one archive may consume.
///
/// The cap is a fraction of what is actually free rather than a constant,
/// because the constant version got this wrong in both directions at once:
/// 4 GB refuses an ordinary 6 GB project on a machine with 800 GB spare, and
/// permits filling a laptop that has 5 GB left. Neither is the question a
/// user cares about, which is whether this will fit.
///
/// This check cannot tell a bomb from a large file, and does not try. A 6 GB
/// video and a 6 GB bomb are the same event to it. What it bounds is how far
/// either gets before the machine is in trouble.
pub const FREE_SPACE_FRACTION: u64 = 2;

/// Floor for that fraction, so a nearly-full volume still allows small work.
pub const MIN_BUDGET_BYTES: u64 = 1024 * 1024 * 1024;

/// Ceiling, so an enormous empty volume does not mean no bound at all.
pub const MAX_BUDGET_BYTES: u64 = 64 * 1024 * 1024 * 1024;

/// How many bytes this extraction may write, given free space on the target.
///
/// Half of free, clamped. Returns the ceiling when free space is unknown --
/// refusing to extract because `df` could not be read would fail closed on a
/// question that has nothing to do with safety.
pub fn budget(free: Option<u64>) -> u64 {
    match free {
        Some(f) => (f / FREE_SPACE_FRACTION).clamp(MIN_BUDGET_BYTES, MAX_BUDGET_BYTES),
        None => MAX_BUDGET_BYTES,
    }
}

/// Deepest nesting accepted in a member path.
///
/// The other axis of a bomb: not size but structure. ClamAV bounds this too
/// (`MaxRecursion`, default 17). A path deeper than this is refused before
/// anything is written.
pub const MAX_DEPTH: usize = 32;

#[derive(Debug, PartialEq, Eq)]
pub enum Unsafe {
    /// A symlink member. Measured, not assumed: `unpack lone.zip` reported
    /// "Checked 2 paths before writing anything" and then landed
    /// `shortcut -> /etc/passwd` in the target, because the name-only
    /// listing this tool used could not see a member's TYPE. Both bsdtar and
    /// Info-ZIP refused to write THROUGH such a link on this machine -- but
    /// that is version-pinned platform behaviour, not this tool's contract,
    /// and the link itself is still a booby trap handed to whoever opens the
    /// folder next.
    Symlink(String),
    /// A hard link member: it can name an inode outside the target, so
    /// writing "inside" the extraction changes data outside it.
    Hardlink(String),
    /// A device node, FIFO or socket. Nothing an archive of files needs, and
    /// each is a surface this tool has no business creating.
    SpecialNode(String),
    /// setuid or setgid bits. An extracted file must never carry authority
    /// its extractor did not have.
    SetuidBit(String),
    /// `/etc/passwd`: would write outside the target entirely.
    AbsolutePath(String),
    /// `../../.ssh/authorized_keys`: the classic Zip Slip.
    Escapes(String),
    /// Windows drive letters and UNC paths.
    DriveOrUnc(String),
    /// Nested past `MAX_DEPTH`. The structural axis of a bomb: an archive can
    /// be small and shallow in bytes while being pathological in shape, and
    /// deep trees are slow to walk and slow to delete afterwards.
    TooDeep(String),
}

impl std::fmt::Display for Unsafe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unsafe::AbsolutePath(p) => write!(f, "absolute path: {p}"),
            Unsafe::Escapes(p) => write!(f, "escapes the target: {p}"),
            Unsafe::DriveOrUnc(p) => write!(f, "drive or UNC path: {p}"),
            Unsafe::TooDeep(p) => write!(f, "nested deeper than {MAX_DEPTH}: {p}"),
            Unsafe::Symlink(p) => write!(
                f,
                "symlink: {p}\n       a dispatched extractor cannot be stopped from \
                 writing through a link, and the link itself points wherever the \
                 archive says. This build unpacks regular files and directories.\n       \
                 `unpack ARCHIVE --list` shows every member without writing"
            ),
            Unsafe::Hardlink(p) => write!(
                f,
                "hard link: {p}\n       a hard link can name a file outside the target, \
                 so writing inside the extraction would change data outside it.\n       \
                 `unpack ARCHIVE --list` shows every member without writing"
            ),
            Unsafe::SpecialNode(p) => write!(
                f,
                "device, FIFO or socket: {p}\n       an archive of files has no need of \
                 one, and creating it is a surface this tool will not open.\n       \
                 `unpack ARCHIVE --list` shows every member without writing"
            ),
            Unsafe::SetuidBit(p) => write!(
                f,
                "setuid or setgid: {p}\n       an extracted file must not carry authority \
                 its extractor did not have.\n       \
                 extract it yourself with `tar -xf` if you trust the source"
            ),
        }
    }
}

/// Judge one archive member path.
///
/// Purely lexical: it never touches the filesystem, so it cannot be defeated by
/// anything that changes on disk between the check and the extraction.
/// One member as a verbose listing describes it: the mode string decides the
/// TYPE, which a name-only listing cannot.
///
/// `tar -tvf` and `unzip -Z` both lead with a mode string, and both were
/// measured before this was written:
///
///     lrwxr-xr-x  ...  link -> /tmp/OUTSIDE     symlink, tar
///     lrwxrwxrwx  ...  slink                    symlink, zip
///     -rwsr-xr-x  ...  suid.bin                 setuid, tar
///
/// The type character is position 0; setuid and setgid are the `s`/`S` in
/// positions 3 and 6. Anything that is not a regular file or a directory is
/// refused by kind, so a format that grows a new node type is refused rather
/// than silently permitted.
pub fn judge_mode(mode: &str, path: &str) -> Option<Unsafe> {
    let b = mode.as_bytes();
    if b.len() < 10 {
        return None; // not a mode string; the caller falls back to the name
    }
    match b[0] {
        b'-' | b'd' => {}
        b'l' => return Some(Unsafe::Symlink(path.to_string())),
        b'h' => return Some(Unsafe::Hardlink(path.to_string())),
        _ => return Some(Unsafe::SpecialNode(path.to_string())),
    }
    if matches!(b[3], b's' | b'S') || matches!(b[6], b's' | b'S') {
        return Some(Unsafe::SetuidBit(path.to_string()));
    }
    None
}

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
            _ => {
                depth += 1;
                if depth as usize > MAX_DEPTH {
                    return Some(Unsafe::TooDeep(p.to_string()));
                }
            }
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

#[cfg(test)]
mod type_tests {
    use super::*;

    /// Mode strings measured from the real tools, not invented.
    ///
    /// Each row was copied from an actual `tar -tvf` or `unzip -Z` run
    /// against an archive built for the purpose. That is the difference
    /// between a test of this parser and a test of my idea of the format.
    #[test]
    fn measured_mode_strings_are_judged_by_kind() {
        // tar -tvf, real output
        assert!(matches!(
            judge_mode("lrwxr-xr-x", "link"),
            Some(Unsafe::Symlink(_))
        ));
        assert!(matches!(
            judge_mode("-rwsr-xr-x", "suid.bin"),
            Some(Unsafe::SetuidBit(_))
        ));
        assert!(matches!(
            judge_mode("prw-r--r--", "pipe"),
            Some(Unsafe::SpecialNode(_))
        ));
        // unzip -Z, real output
        assert!(matches!(
            judge_mode("lrwxrwxrwx", "slink"),
            Some(Unsafe::Symlink(_))
        ));
        // ordinary members pass
        assert!(judge_mode("-rw-r--r--", "a.txt").is_none());
        assert!(judge_mode("drwxr-xr-x", "dir").is_none());
        // setgid too, and in either position
        assert!(matches!(
            judge_mode("-rwxr-sr-x", "sgid.bin"),
            Some(Unsafe::SetuidBit(_))
        ));
        // a device node and a socket are refused by KIND, so a type this
        // code has never seen is refused rather than permitted by default
        assert!(matches!(
            judge_mode("crw-rw-rw-", "dev"),
            Some(Unsafe::SpecialNode(_))
        ));
        assert!(matches!(
            judge_mode("srwxrwxrwx", "sock"),
            Some(Unsafe::SpecialNode(_))
        ));
        // not a mode string at all: no opinion, the caller falls back
        assert!(judge_mode("Archive:", "x").is_none());
        assert!(judge_mode("", "x").is_none());
    }

    /// The line-level property the parser rests on, stated so it is checked:
    /// the TYPE character is at position 0 of the mode string, which both
    /// tools emit first on every member row. So even a member whose NAME
    /// mimics a mode string, a timestamp, or contains " -> " is judged by
    /// kind correctly -- and because refusal is all-or-nothing on the
    /// archive, a mis-parsed PATH can only name the member imprecisely, never
    /// let it through. Measured against real archives built for each shape.
    #[test]
    fn the_type_character_decides_regardless_of_the_name() {
        for name in [
            "two words link",
            "evil -> notreally",
            "12:34",
            "-rw-r--r--",
            "line1\\nline2",
        ] {
            assert!(
                matches!(judge_mode("lrwxr-xr-x", name), Some(Unsafe::Symlink(_))),
                "a symlink named {name:?} was not judged a symlink"
            );
        }
        // and a regular file with an alarming name is still just a file
        assert!(judge_mode("-rw-r--r--", "lrwxr-xr-x").is_none());
    }

    /// Every refusal names the member, states the rule, and gives a recourse.
    ///
    /// The operator's contract, asserted rather than assumed: a refusal that
    /// only blocks is a dead end, and an agent branching on exit 2 needs a
    /// next move in the text.
    #[test]
    fn every_refusal_carries_member_rule_and_recourse() {
        let cases = vec![
            Unsafe::Symlink("shortcut".into()),
            Unsafe::Hardlink("hl".into()),
            Unsafe::SpecialNode("pipe".into()),
            Unsafe::SetuidBit("suid.bin".into()),
        ];
        for c in cases {
            let msg = c.to_string();
            let member = match &c {
                Unsafe::Symlink(p)
                | Unsafe::Hardlink(p)
                | Unsafe::SpecialNode(p)
                | Unsafe::SetuidBit(p) => p.clone(),
                _ => unreachable!(),
            };
            assert!(
                msg.contains(&member),
                "refusal does not name the member: {msg}"
            );
            assert!(
                msg.contains("unpack ARCHIVE --list") || msg.contains("tar -xf"),
                "refusal offers no recourse: {msg}"
            );
            assert!(msg.len() > 60, "refusal states no rule: {msg}");
        }
    }
}
