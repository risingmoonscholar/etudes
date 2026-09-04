//! unpack: stop thinking about archive formats.
//!
//! # Why this parses nothing
//!
//! macOS ships `unzip`, `tar`, `ditto` and `hdiutil`. They are older and far
//! better tested than any parser this could add, and adding `zip`, `tar`,
//! `flate2` and `sevenz` crates would multiply the dependency surface of a
//! series whose whole pitch is a readable one.
//!
//! So unpack dispatches. The product is the safe uniform wrapper around tools
//! you already have.
//!
//! # The order that makes it safe
//!
//! **List, judge, then extract.** Every archive is enumerated first and every
//! member path judged lexically. Extracting first and cleaning up afterwards
//! means a hostile archive has already written outside the target, and a bomb
//! has already filled the disk.

mod safety;

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
#[cfg(test)]
use std::sync::atomic::{AtomicU64, Ordering};

use safety::Unsafe;

// `unpack` deliberately targets the macOS system extractors.  Do not replace
// these with PATH lookups: the executable is part of the safety boundary.
const UNZIP: &str = "/usr/bin/unzip";
const TAR: &str = "/usr/bin/tar";
const GUNZIP: &str = "/usr/bin/gunzip";

const USAGE: &str = "\
unpack: stop thinking about archive formats

USAGE
    unpack ARCHIVE [--into DIR]    extract safely into its own directory
    unpack ARCHIVE --list          show what is inside, extract nothing
    --max-size N[G|M]              allow this extraction to write more
    --json                         machine-readable output (for agents)
    --version                      print the version and exit
    unpack help

Handles .zip .tar .tar.gz .tgz .tar.bz2 .tar.xz .gz using the tools already
on this machine. Nothing is parsed here.

.dmg is recognised and refused, by design rather than pending. Opening one
means asking the kernel to mount a stranger's filesystem image, which is a
much larger thing than running unzip against a stream, and no size check
covers it. `hdiutil attach` does it if you decide to.

Every archive is listed and judged BEFORE anything is written. Paths that
escape the target, absolute paths and drive paths are refused outright.

Extraction stops if it writes more than half the free space on the target
volume, and the target is removed. The limit is on bytes that land on disk,
never on the size an archive claims for itself: those numbers are written by
whoever built it, and forging four bytes makes unzip, tar and gzip all
understate a member by three orders of magnitude.

A large legitimate archive and a decompression bomb are the same event to
that check. It bounds damage, it does not detect intent -- so --max-size
raises the bound when you know what you are unpacking.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Zip,
    Tar,
    TarGz,
    TarBz2,
    TarXz,
    Gz,
    Dmg,
}

/// A private byte-for-byte copy of the archive we judged.
///
/// The callers below must use this name rather than the pathname supplied on
/// the command line. Copying the caller's file once binds the listings and
/// extractor to private bytes even if somebody replaces the original pathname
/// or rewrites the original inode after preflight.
struct ArchiveAnchor {
    dir: PathBuf,
    path: PathBuf,
}

impl ArchiveAnchor {
    fn create(archive: &Path) -> Result<Self, String> {
        for _ in 0..128 {
            // A counter plus PID is visible to any process and makes this
            // supposedly private filename predictable.  Read enough bytes
            // from the kernel CSPRNG that another process cannot derive the
            // anchor pathname and replace the copy after preflight.
            let mut nonce = [0_u8; 16];
            std::fs::File::open("/dev/urandom")
                .and_then(|mut random| random.read_exact(&mut nonce))
                .map_err(|error| format!("could not generate private anchor name: {error}"))?;
            let dir = std::env::temp_dir().join(format!(
                "unpack-{}",
                nonce
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
            ));
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&dir) {
                Ok(()) => {
                    // `stem` supplies the output name for a bare .gz, so the
                    // anchored file keeps the original basename while its
                    // containing directory supplies the privacy boundary.
                    let path = dir.join(
                        archive
                            .file_name()
                            .unwrap_or(std::ffi::OsStr::new("archive")),
                    );
                    // Copy rather than hard-link: a link pins an inode, but
                    // an attacker holding a writable descriptor can still
                    // truncate and rewrite that inode after both preflights.
                    // The private copy is the one immutable input all later
                    // operations consume.
                    match std::fs::symlink_metadata(archive) {
                        Ok(metadata) if metadata.file_type().is_symlink() => {
                            let _ = std::fs::remove_dir_all(&dir);
                            return Err("the archive path is a symlink".into());
                        }
                        Ok(_) => {}
                        Err(error) => {
                            let _ = std::fs::remove_dir_all(&dir);
                            return Err(error.to_string());
                        }
                    }
                    if let Err(error) = std::fs::copy(archive, &path) {
                        let _ = std::fs::remove_dir_all(&dir);
                        return Err(error.to_string());
                    }
                    return Ok(Self { dir, path });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.to_string()),
            }
        }
        Err("could not create a private archive anchor".into())
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ArchiveAnchor {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

// This seam exists only to make the pathname-replacement regression
// deterministic.  It is compiled out of the shipped binary, so callers
// cannot influence either the extractor path or the preflight/extract gap.
#[cfg(test)]
static AFTER_PREFLIGHT: std::sync::Mutex<Option<Box<dyn FnOnce() + Send>>> =
    std::sync::Mutex::new(None);

#[cfg(test)]
fn after_preflight_for_test() {
    if let Some(action) = AFTER_PREFLIGHT.lock().expect("preflight hook lock").take() {
        action();
    }
}

#[cfg(not(test))]
fn after_preflight_for_test() {}

fn detect(path: &Path) -> Option<Format> {
    let n = path.file_name()?.to_string_lossy().to_ascii_lowercase();
    // Longest suffix first: .tar.gz must win over .gz.
    for (suffix, f) in [
        (".tar.gz", Format::TarGz),
        (".tgz", Format::TarGz),
        (".tar.bz2", Format::TarBz2),
        (".tbz", Format::TarBz2),
        (".tar.xz", Format::TarXz),
        (".txz", Format::TarXz),
        (".tar", Format::Tar),
        (".zip", Format::Zip),
        (".jar", Format::Zip),
        (".dmg", Format::Dmg),
        (".gz", Format::Gz),
    ] {
        if n.ends_with(suffix) {
            return Some(f);
        }
    }
    None
}

/// Strip every archive suffix to get the destination directory name.
fn stem(path: &Path) -> String {
    let mut n = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    for suffix in [
        ".tar.gz", ".tar.bz2", ".tar.xz", ".tgz", ".tbz", ".txz", ".tar", ".zip", ".jar", ".dmg",
        ".gz",
    ] {
        if n.to_ascii_lowercase().ends_with(suffix) {
            n.truncate(n.len() - suffix.len());
            break;
        }
    }
    if n.is_empty() { "unpacked".into() } else { n }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("help" | "--help" | "-h") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some("version" | "--version" | "-V") => {
            println!("unpack {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        // A leading flag is never an archive name. Without this, `unpack
        // --frobnicate` took the typo as a filename and reported "not a file"
        // with exit 3, so a caller could not tell a usage mistake from a
        // missing archive. Same defect stash had with `--version`, and the
        // same answer: refuse with 2, which is what the exit-code contract
        // reserves for a refusal.
        Some(p) if p.starts_with('-') => {
            // A real flag with no archive is a different mistake from a typo,
            // and saying "unknown option" to a flag that exists is its own
            // small lie.
            const KNOWN: &[&str] = &["--list", "--json", "--into"];
            if KNOWN.contains(&p) {
                eprintln!(
                    "unpack: {p} describes what to do with an archive, so it \
                     needs one.\n     unpack ARCHIVE {p}"
                );
            } else {
                eprintln!(
                    "unpack: unknown option {p}.\n\
                     Handles: --list, --json, --into DIR. Run `unpack help`."
                );
            }
            ExitCode::from(2)
        }
        Some(p) => run(&PathBuf::from(expand_tilde(p)), &args),
    }
}

fn expand_tilde(p: &str) -> String {
    match p.strip_prefix("~/") {
        Some(rest) => std::env::var("HOME")
            .map(|h| format!("{h}/{rest}"))
            .unwrap_or(p.into()),
        None => p.to_string(),
    }
}

fn flag(args: &[String], f: &str) -> bool {
    args.iter().any(|a| a == f)
}

fn value(args: &[String], f: &str) -> Option<String> {
    let i = args.iter().position(|a| a == f)?;
    args.get(i + 1).cloned()
}

fn run(archive: &Path, args: &[String]) -> ExitCode {
    if !archive.is_file() {
        eprintln!("unpack: not a file: {}", etude_core::redact::path(archive));
        return ExitCode::from(3);
    }
    let Some(fmt) = detect(archive) else {
        eprintln!(
            "unpack: unrecognised archive type.\n\
             Handles .zip .tar .tar.gz .tgz .tar.bz2 .tar.xz .gz"
        );
        return ExitCode::from(3);
    };

    if fmt == Format::Dmg {
        // Refused by design, not pending. Every other format here is a stream
        // handed to a userspace decompressor; a .dmg is a filesystem image
        // handed to the kernel's own HFS+/APFS drivers by `hdiutil attach`.
        // That is a qualitatively larger trust boundary and it is the one
        // place this tool would ask the kernel to parse a stranger's data.
        //
        // The size check that works for archives does not cover it. Bounding
        // bytes written says nothing about a malformed image reaching a
        // filesystem driver, and there is no userspace path to fall back on:
        // mounting is the only way to read one. So the honest answer is not
        // to, and to say why rather than leave it looking like a gap somebody
        // will helpfully fill in later.
        //
        // Exit 2, refused, not 3. This is a decision, not a failure.
        eprintln!(
            "unpack: .dmg is refused, on purpose and permanently.\n\n\
             Every other format here is handed to a decompressor as a stream.\n\
             A disk image is handed to the kernel's own filesystem drivers to\n\
             mount, which is a much larger thing to ask on behalf of a file\n\
             someone sent you, and no size check covers it.\n\n\
             To open one anyway:  hdiutil attach ARCHIVE.dmg\n\
             That is the same command unpack would run. Running it yourself\n\
             means the decision is yours rather than one this tool made\n\
             quietly on your behalf."
        );
        return ExitCode::from(2);
    }

    // Copy the input once, before either listing. From this point onward no
    // process is handed the caller-controlled pathname: both preflights and
    // extraction consume the same private bytes, even if that pathname is
    // replaced or rewritten in the meantime.
    let pinned = match ArchiveAnchor::create(archive) {
        Ok(pinned) => pinned,
        Err(e) => {
            eprintln!("unpack: could not secure the archive for checking ({e})");
            return ExitCode::from(3);
        }
    };

    // --- 1. list -----------------------------------------------------------
    let entries = match list(pinned.path(), fmt) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("unpack: could not read the archive ({e})");
            return ExitCode::from(3);
        }
    };
    if entries.is_empty() {
        eprintln!("unpack: the archive appears to be empty");
        return ExitCode::from(1);
    }
    if entries.len() > safety::MAX_ENTRIES {
        eprintln!(
            "unpack: refused, {} entries exceeds the {} cap",
            entries.len(),
            safety::MAX_ENTRIES
        );
        return ExitCode::from(2);
    }

    // --- 2. judge, before anything is written ------------------------------
    // Two judgements, and the second is the one a name-only listing could not
    // make. Lexical first (traversal, absolute, depth) on every member; then
    // TYPE, from the verbose listing's mode string -- symlink, hard link,
    // device node, setuid. A member is refused if either says so.
    let mut blocked: Vec<Unsafe> = entries.iter().filter_map(|p| safety::judge(p)).collect();
    let members = match list_members(pinned.path(), fmt) {
        Ok(members) => {
            if let Err(missing) = typed_rows_cover_entries(&entries, &members) {
                eprintln!(
                    "unpack: REFUSED. The plain listing names {missing:?}, but the typed\n\
                     listing has no corresponding member row. Refusing rather than extract\n\
                     an entry whose type was never checked."
                );
                return ExitCode::from(2);
            }
            members
        }
        Err(_) => {
            // The verbose listing failed where the plain one worked. That is
            // a parse disagreement between two reads of the same archive, and
            // codex's rule applies: never treat agreement as proof, and never
            // treat a failed check as a pass. Refuse rather than extract with
            // one of the two judgements missing.
            eprintln!(
                "unpack: REFUSED. This archive lists its names but not its member\n\
                 types, so whether it contains links or device nodes cannot be\n\
                 established. Refusing rather than extracting half-checked.\n       \
                 `unpack {} --list` shows the names it does report.",
                archive.display()
            );
            return ExitCode::from(2);
        }
    };
    for m in &members {
        if let Some(u) = m
            .mode
            .as_deref()
            .and_then(|mode| safety::judge_mode(mode, &m.path))
        {
            blocked.push(u);
        }
    }
    let junk = entries.iter().filter(|p| safety::is_junk(p)).count();

    if flag(args, "--list") && flag(args, "--json") {
        // Lets an agent inspect an archive and decide, without extracting.
        use etude_core::json as j;
        println!(
            "{}",
            j::obj(&[
                ("archive", j::path(archive)),
                ("entries", j::num(entries.len())),
                ("junk", j::num(junk)),
                ("safe", j::bool(blocked.is_empty())),
                (
                    "blocked",
                    j::arr(blocked.iter().map(|b| j::str(&b.to_string())))
                ),
                (
                    "wrapper",
                    safety::wrapper_dir(&entries)
                        .map(|w| j::str(&w))
                        .unwrap_or_else(|| "null".into())
                ),
                ("paths", j::arr(entries.iter().map(|p| j::str(p)))),
            ])
        );
        return ExitCode::SUCCESS;
    }

    if flag(args, "--list") {
        println!("\n{} entries in {}", entries.len(), stem(archive));
        for p in entries.iter().take(40) {
            println!("  {p}");
        }
        if entries.len() > 40 {
            println!("  … {} more", entries.len() - 40);
        }
        if !blocked.is_empty() {
            println!("\n  {} unsafe path(s) would be refused:", blocked.len());
            for b in &blocked {
                println!("    {b}");
            }
        }
        return ExitCode::SUCCESS;
    }

    if !blocked.is_empty() && flag(args, "--json") {
        use etude_core::json as j;
        println!(
            "{}",
            j::obj(&[
                ("archive", j::path(archive)),
                ("refused", j::bool(true)),
                ("reason", j::str("unsafe paths")),
                (
                    "blocked",
                    j::arr(blocked.iter().map(|b| j::str(&b.to_string())))
                ),
                ("extracted", j::num(0)),
            ])
        );
        return ExitCode::from(2);
    }

    if !blocked.is_empty() {
        // Refuse the whole archive. Extracting the safe subset of a hostile
        // archive gives the attacker a partial success and the user a mess.
        eprintln!(
            "\nunpack: REFUSED. This archive contains members this tool will not\n\
             create.\n"
        );
        for b in blocked.iter().take(10) {
            eprintln!("  {b}");
        }
        if blocked.len() > 10 {
            eprintln!("  … and {} more", blocked.len() - 10);
        }
        eprintln!(
            "\nNothing was extracted. Inspect it with: unpack {} --list",
            archive.display()
        );
        return ExitCode::from(2);
    }

    after_preflight_for_test();

    // --- 3. choose a destination that cannot explode into the cwd ----------
    let parent = archive.parent().unwrap_or(Path::new("."));
    let dest = match value(args, "--into") {
        Some(d) => PathBuf::from(expand_tilde(&d)),
        None => parent.join(stem(archive)),
    };
    if dest.exists() {
        eprintln!(
            "unpack: {} already exists. Pass --into DIR to choose another.",
            etude_core::redact::path(&dest)
        );
        return ExitCode::from(2);
    }

    // --- 4. extract --------------------------------------------------------
    // How much room is there, and therefore how much may this write? Asked of
    // the parent, since dest does not exist yet. This is the question no
    // header can answer, and the reason the bound is not a constant: a fixed
    // 4 GB refuses an ordinary project on a roomy disk and permits filling a
    // full one.
    let free = dest.parent().and_then(free_bytes);
    let budget = match max_size_flag(args) {
        Some(n) => n,
        None => safety::budget(free),
    };
    match free {
        Some(f) if f < MIN_FREE_BYTES => {
            eprintln!(
                "unpack: refused. Only {} free on this volume, and extracting needs room\n\
                 to work. Nothing was written.",
                human(f)
            );
            return ExitCode::from(2);
        }
        _ => {}
    }

    if let Err(e) = std::fs::create_dir_all(&dest) {
        // The OS text, not the Rust category -- "Permission denied
        // (os error 13)" tells a user what to do; "permission denied"
        // alone is the same words with the errno thrown away, and
        // "uncategorized error" tells them nothing at all.
        eprintln!("unpack: cannot create the target ({e})");
        return ExitCode::from(3);
    }
    match extract(pinned.path(), fmt, &dest, budget) {
        Err(e) => {
            let _ = std::fs::remove_dir_all(&dest);
            eprintln!("unpack: extraction failed ({e}). Nothing was left behind.");
            return ExitCode::from(3);
        }
        Ok(Err(breach)) => {
            // Remove what landed before the cap was hit. A partial extraction
            // left behind is the mess this tool exists to avoid, and it is
            // worse here than usual: the user did not choose to start it.
            let _ = std::fs::remove_dir_all(&dest);
            let Breach::Total(n) = breach;
            eprintln!(
                "unpack: stopped at {}, which is more than this extraction was given.\n\
                 Nothing was left behind.\n\n\
                 The budget is half the free space on the target volume, so a large\n\
                 archive on a roomy disk is fine and the same archive on a full one is\n\
                 not. This says nothing about the archive being hostile: a big project\n\
                 and a decompression bomb look identical from here, which is why the\n\
                 limit is on damage rather than on intent.\n\n\
                 To allow more:  unpack ARCHIVE --max-size {}G",
                human(n),
                (n / (1024 * 1024 * 1024)) + 2
            );
            return ExitCode::from(2);
        }
        Ok(Ok(())) => {}
    }

    // --- 5. tidy -----------------------------------------------------------
    let removed = remove_junk(&dest);
    let flattened = match safety::wrapper_dir(&entries) {
        Some(w) => flatten(&dest, &w),
        None => false,
    };

    if flag(args, "--json") {
        use etude_core::json as j;
        println!(
            "{}",
            j::obj(&[
                ("archive", j::path(archive)),
                ("refused", j::bool(false)),
                ("dest", j::path(&dest)),
                ("entries", j::num(entries.len() - junk)),
                ("flattened", j::bool(flattened)),
                ("junk_removed", j::num(removed)),
                ("paths_checked", j::num(members.len())),
            ])
        );
        return ExitCode::SUCCESS;
    }

    println!("\n  Extracted to {}/", dest.display());
    println!("  {} entries", entries.len() - junk);
    if flattened {
        println!("  Removed a duplicate wrapper folder");
    }
    if removed > 0 {
        println!("  Dropped {removed} metadata file(s)");
    }
    println!("  Checked {} paths before writing anything", members.len());
    ExitCode::SUCCESS
}

/// Enumerate member paths without extracting.
/// One member as the listing describes it: its mode string, if the format
/// reports one, and its path.
pub struct Member {
    pub mode: Option<String>,
    pub path: String,
}

/// Every plain-list member must have a typed row before extraction can start.
///
/// Counts rather than a set make duplicate archive names visible too: each
/// occurrence is a separate thing the type pass must actually have reached.
fn typed_rows_cover_entries(entries: &[String], members: &[Member]) -> Result<(), String> {
    let mut available: HashMap<&str, usize> = HashMap::new();
    for member in members {
        *available.entry(&member.path).or_default() += 1;
    }
    for entry in entries {
        let Some(count) = available.get_mut(entry.as_str()) else {
            return Err(entry.clone());
        };
        if *count == 0 {
            return Err(entry.clone());
        }
        *count -= 1;
    }
    // The comparison is deliberately two-way. Otherwise an extra parsed
    // typed row could inflate the reported number of paths checked despite
    // never being reached by the plain-list pass.
    for (path, count) in available {
        if count != 0 {
            return Err(path.to_string());
        }
    }
    Ok(())
}

/// Members with their modes, so a member's TYPE is visible.
///
/// The name-only listings this replaced (`-Z1`, `-tf`) could not see that a
/// member was a symlink, so `unpack` printed "Checked 2 paths before writing
/// anything" and then landed `shortcut -> /etc/passwd` in the target.
/// Measured on both formats before this existed.
fn list_members(archive: &Path, fmt: Format) -> Result<Vec<Member>, String> {
    if fmt == Format::Gz {
        return Ok(vec![Member {
            mode: None,
            path: stem(archive),
        }]);
    }
    let out = listing_command(fmt, archive, true)?
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err("listing failed".into());
    }
    let mut members = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Some(member) = member_from_typed_line(line) {
            members.push(member);
        }
    }
    Ok(members)
}

/// Construct either of the two archive listing commands.
///
/// These are deliberately distinct from `extractor_command`: their output is
/// the evidence used for the safety decision, and their argv must not inherit
/// extraction flags by accident.
fn listing_command(fmt: Format, archive: &Path, typed: bool) -> Result<Command, String> {
    let (bin, args): (&str, &[&str]) = match (fmt, typed) {
        (Format::Zip, false) => (UNZIP, &["-Z1"]),
        (Format::Zip, true) => (UNZIP, &["-Z"]),
        (Format::Tar, false) => (TAR, &["-tf"]),
        (Format::Tar, true) => (TAR, &["-tvf"]),
        (Format::TarGz, false) => (TAR, &["-tzf"]),
        (Format::TarGz, true) => (TAR, &["-tvzf"]),
        (Format::TarBz2, false) => (TAR, &["-tjf"]),
        (Format::TarBz2, true) => (TAR, &["-tvjf"]),
        (Format::TarXz, false) => (TAR, &["-tJf"]),
        (Format::TarXz, true) => (TAR, &["-tvJf"]),
        (Format::Gz, _) => {
            return Err("gzip has no member listing command".into());
        }
        (Format::Dmg, _) => return Err("dmg unsupported".into()),
    };
    let mut command = Command::new(bin);
    command.args(args).arg(archive).stderr(Stdio::null());
    Ok(command)
}

/// Parse one `tar -t[v...]` or `unzip -Z` member row.
///
/// Both programs put the member name after a time or year column. The first
/// such column is structural; searching from the end would instead consume a
/// perfectly ordinary name such as `notes 12:30`, leaving the two listings to
/// disagree. Slice the original row so every filename space is kept.
fn member_from_typed_line(line: &str) -> Option<Member> {
    let t = line.trim_start();
    if t.trim().is_empty() {
        return None;
    }
    let first = t.split_whitespace().next().unwrap_or("");
    // A mode string is 10 chars and starts with a type character. Lines that
    // are not member rows (headers, totals) simply have no mode and no path
    // we can trust, so they are skipped rather than guessed at.
    let is_mode = first.len() == 10
        && matches!(
            first.as_bytes()[0],
            b'-' | b'd' | b'l' | b'h' | b'c' | b'b' | b'p' | b's'
        );
    if !is_mode {
        return None;
    }
    let rest = t[first.len()..].trim_start();
    let mut p = after_first_time_column(rest)?.to_string();
    // The tools append a symlink target after the name. A regular filename
    // may itself contain ` -> `, so only a symlink row gets that suffix cut.
    if first.starts_with('l')
        && let Some(cut) = p.rfind(" -> ")
    {
        p.truncate(cut);
    }
    let p = p.trim_end_matches('/').to_string();
    if p.is_empty() {
        return None;
    }
    Some(Member {
        mode: Some(first.to_string()),
        path: p,
    })
}

/// Return the path portion after the first structural time or year token.
///
/// The slice is taken from the original row, rather than joined from tokens,
/// so filenames containing repeated or leading spaces stay byte-for-byte
/// comparable with the plain `-Z1` / `-tf` listing. The one space immediately
/// after the structural timestamp is a column separator; any further spaces
/// are part of the filename.
fn after_first_time_column(rest: &str) -> Option<&str> {
    let mut start = None;
    let mut previous = None;
    let mut before_previous = None;
    for (end, ch) in rest.char_indices() {
        if ch.is_whitespace() {
            let Some(begin) = start.take() else {
                continue;
            };
            let token = &rest[begin..end];
            let is_time = token.len() == 5
                && token.as_bytes()[2] == b':'
                && token
                    .bytes()
                    .enumerate()
                    .all(|(i, b)| i == 2 || b.is_ascii_digit());
            // BSD tar prints an old timestamp as `Jan  1  2024`. GNU tar and
            // zip both retain a time column, so a hyphen in an owner or group
            // name is never evidence that a preceding four-digit size is a
            // year.
            let is_year = token.len() == 4
                && token.bytes().all(|b| b.is_ascii_digit())
                && previous.is_some_and(|day: &str| {
                    (1..=2).contains(&day.len()) && day.bytes().all(|b| b.is_ascii_digit())
                })
                && before_previous.is_some_and(|month: &str| {
                    month.len() == 3 && month.bytes().all(|b| b.is_ascii_alphabetic())
                });
            if is_time || is_year {
                let after_timestamp = &rest[end..];
                let separator = after_timestamp.chars().next()?;
                return Some(&after_timestamp[separator.len_utf8()..]);
            }
            before_previous = previous;
            previous = Some(token);
        } else if start.is_none() {
            start = Some(end);
        }
    }
    None
}

fn list(archive: &Path, fmt: Format) -> Result<Vec<String>, String> {
    if fmt == Format::Gz {
        return Ok(vec![stem(archive)]);
    }
    let out = listing_command(fmt, archive, false)?
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err("listing failed".into());
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim_end_matches('/').to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

/// `--max-size N[G|M]`: the caller's own bound, replacing the free-space one.
///
/// Exists because the default WILL refuse legitimate work -- a 6 GB project is
/// indistinguishable from a 6 GB bomb from here. A refusal with no way past it
/// is a wall rather than a safety feature, so the way past it is named in the
/// refusal itself.
fn max_size_flag(args: &[String]) -> Option<u64> {
    let i = args.iter().position(|a| a == "--max-size")?;
    let raw = args.get(i + 1)?;
    let (digits, mult) = match raw.chars().last() {
        Some('G' | 'g') => (&raw[..raw.len() - 1], 1024 * 1024 * 1024),
        Some('M' | 'm') => (&raw[..raw.len() - 1], 1024 * 1024),
        _ => (raw.as_str(), 1),
    };
    digits.parse::<u64>().ok().map(|n| n * mult)
}

/// Least free space extraction will start with.
///
/// Not a guess at what the archive needs -- nothing can know that before
/// writing it. It refuses the case where any extraction is a bad idea.
const MIN_FREE_BYTES: u64 = 256 * 1024 * 1024;

/// Bytes as something a person can read at a glance.
fn human(n: u64) -> String {
    const K: u64 = 1024;
    match n {
        n if n >= K * K * K => format!("{:.1} GB", n as f64 / (K * K * K) as f64),
        n if n >= K * K => format!("{:.1} MB", n as f64 / (K * K) as f64),
        n if n >= K => format!("{:.1} KB", n as f64 / K as f64),
        n => format!("{n} bytes"),
    }
}

/// Free bytes on the volume holding `p`, via `df`, or None if it cannot be read.
///
/// Asks the question a header cannot answer: will this fit. `df` rather than
/// statvfs because unpack's whole design is to use what the machine already
/// ships rather than take a dependency, and it is already shelling out to
/// unzip and tar.
fn free_bytes(p: &Path) -> Option<u64> {
    let out = Command::new("df").arg("-Pk").arg(p).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    // POSIX -P guarantees one line per filesystem, columns:
    // Filesystem 1024-blocks Used Available Capacity Mounted-on
    let avail_kb: u64 = text
        .lines()
        .nth(1)?
        .split_whitespace()
        .nth(3)?
        .parse()
        .ok()?;
    Some(avail_kb * 1024)
}

/// Total bytes written under `dir`.
///
/// One number, because there is one bound. An earlier version also tracked
/// the largest single file for a separate per-member cap; the two answered
/// the same question and disagreed about which one a user had hit.
fn written(dir: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut total = 0;
    for e in rd.flatten() {
        // symlink_metadata, not metadata: a symlink's target is not what this
        // extraction wrote, and following one could count something outside
        // dest entirely.
        let Ok(md) = e.path().symlink_metadata() else {
            continue;
        };
        if md.is_dir() {
            total += written(&e.path());
        } else {
            total += md.len();
        }
    }
    total
}

/// Why an extraction was abandoned partway.
/// One bound, not two. A per-member cap and a per-archive cap answered the
/// same question twice and disagreed about which one a user had hit; what
/// matters is how much this extraction wrote, whether that was one file or a
/// million.
pub enum Breach {
    Total(u64),
}

/// Run an extractor, watching what it writes and killing it if it goes past
/// the caps.
///
/// The cap is enforced HERE rather than from the listing, because every
/// declared size is written by whoever built the archive. Measured, not
/// assumed: forging four bytes made `unzip -Z`, `tar -tvf` and `gzip -l` each
/// report 4,096 for members that expand to millions of bytes, and the forged
/// zip then extracted in full with unzip exiting 0.
///
/// Polling rather than a stream wrapper, because three of the four formats
/// are extracted by a child process that writes files directly. A poll can
/// overshoot by whatever lands between two checks, so this is a bound on the
/// order of the cap, not to the byte.
fn run_bounded(mut cmd: Command, dest: &Path, budget: u64) -> Result<Result<(), Breach>, String> {
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) => {
                // Check once more after exit: a fast extraction can finish
                // between two polls, and an unchecked archive is the thing
                // this function exists to prevent.
                let total = written(dest);
                if total > budget {
                    return Ok(Err(Breach::Total(total)));
                }
                return if status.success() {
                    Ok(Ok(()))
                } else {
                    Err("extractor reported failure".into())
                };
            }
            None => {
                let total = written(dest);
                if total > budget {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Ok(Err(Breach::Total(total)));
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    }
}

fn extract(
    archive: &Path,
    fmt: Format,
    dest: &Path,
    budget: u64,
) -> Result<Result<(), Breach>, String> {
    let mut cmd = extractor_command(fmt, archive, dest);
    if fmt == Format::Gz {
        let out_path = dest.join(stem(archive));
        let f = std::fs::File::create(&out_path).map_err(|e| e.to_string())?;
        cmd.stdout(f);
    }
    run_bounded(cmd, dest, budget)
}

/// Construct the complete extractor command.
///
/// Tests inspect this `Command` at the process boundary; a helper that merely
/// returns a vector would not prove what is actually passed to the child.
fn extractor_command(fmt: Format, archive: &Path, dest: &Path) -> Command {
    match fmt {
        // -o overwrites only inside our freshly-created destination; -q is
        // presentation only. In particular, do not add unzip's -j (junk
        // paths): path containment is part of the archive contract.
        Format::Zip => {
            let mut command = Command::new(UNZIP);
            command.args(["-o", "-q"]).arg(archive).arg("-d").arg(dest);
            command
        }
        Format::Gz => {
            let mut command = Command::new(GUNZIP);
            command.arg("-c").arg(archive);
            command
        }
        _ => {
            let flag = match fmt {
                Format::Tar => "-xf",
                Format::TarGz => "-xzf",
                Format::TarBz2 => "-xjf",
                Format::TarXz => "-xJf",
                _ => unreachable!("dmg and zip handled above"),
            };
            let mut command = Command::new(TAR);
            command.arg(flag).arg(archive).arg("-C").arg(dest);
            command
        }
    }
}

fn remove_junk(dest: &Path) -> usize {
    fn walk(dir: &Path, n: &mut usize) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let p = e.path();
            let is_dir = std::fs::symlink_metadata(&p)
                .map(|m| m.is_dir())
                .unwrap_or(false);
            if safety::is_junk(&name) {
                let ok = if is_dir {
                    std::fs::remove_dir_all(&p).is_ok()
                } else {
                    std::fs::remove_file(&p).is_ok()
                };
                if ok {
                    *n += 1;
                }
                continue;
            }
            if is_dir {
                walk(&p, n);
            }
        }
    }
    let mut n = 0;
    walk(dest, &mut n);
    n
}

/// Move `dest/wrapper/*` up into `dest`, then remove the wrapper.
fn flatten(dest: &Path, wrapper: &str) -> bool {
    let inner = dest.join(wrapper);
    if !inner.is_dir() {
        return false;
    }
    let Ok(rd) = std::fs::read_dir(&inner) else {
        return false;
    };
    for e in rd.flatten() {
        let to = dest.join(e.file_name());
        if to.exists() {
            return false; // collision, leave the structure alone
        }
        if std::fs::rename(e.path(), &to).is_err() {
            return false;
        }
    }
    std::fs::remove_dir(&inner).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn the_longest_suffix_wins_so_tar_gz_is_not_gz() {
        assert_eq!(detect(Path::new("a.tar.gz")), Some(Format::TarGz));
        assert_eq!(detect(Path::new("a.tgz")), Some(Format::TarGz));
        assert_eq!(detect(Path::new("a.gz")), Some(Format::Gz));
        assert_eq!(detect(Path::new("a.zip")), Some(Format::Zip));
        assert_eq!(
            detect(Path::new("a.TAR.GZ")),
            Some(Format::TarGz),
            "case ignored"
        );
        assert_eq!(detect(Path::new("notes.txt")), None);
    }

    #[test]
    fn the_stem_drops_every_archive_suffix() {
        assert_eq!(
            stem(Path::new("conference-assets.zip")),
            "conference-assets"
        );
        assert_eq!(stem(Path::new("release.tar.gz")), "release");
        assert_eq!(stem(Path::new("data.tgz")), "data");
        assert_eq!(stem(Path::new("dump.sql.gz")), "dump.sql");
    }

    #[test]
    fn every_plain_member_must_have_its_own_typed_row() {
        let entries = vec!["ordinary.txt".into(), "ordinary.txt".into(), "link".into()];
        let typed = vec![
            Member {
                mode: Some("-rw-r--r--".into()),
                path: "ordinary.txt".into(),
            },
            Member {
                mode: Some("lrwxrwxrwx".into()),
                path: "link".into(),
            },
        ];
        assert_eq!(
            typed_rows_cover_entries(&entries, &typed),
            Err("ordinary.txt".into())
        );
    }

    #[test]
    fn an_extra_typed_row_is_not_counted_as_checked() {
        let entries = vec!["ordinary.txt".into()];
        let typed = vec![
            Member {
                mode: Some("-rw-r--r--".into()),
                path: "ordinary.txt".into(),
            },
            Member {
                mode: Some("-rw-r--r--".into()),
                path: "unlisted.txt".into(),
            },
        ];
        assert_eq!(
            typed_rows_cover_entries(&entries, &typed),
            Err("unlisted.txt".into())
        );
    }

    #[test]
    fn extractor_commands_are_pinned_at_the_process_boundary() {
        let archive = Path::new("/tmp/archive.tar.gz");
        let dest = Path::new("/tmp/dest");
        for (format, bin, expected) in [
            (
                Format::Zip,
                UNZIP,
                vec!["-o", "-q", "/tmp/archive.tar.gz", "-d", "/tmp/dest"],
            ),
            (
                Format::Tar,
                TAR,
                vec!["-xf", "/tmp/archive.tar.gz", "-C", "/tmp/dest"],
            ),
            (
                Format::TarGz,
                TAR,
                vec!["-xzf", "/tmp/archive.tar.gz", "-C", "/tmp/dest"],
            ),
            (
                Format::TarBz2,
                TAR,
                vec!["-xjf", "/tmp/archive.tar.gz", "-C", "/tmp/dest"],
            ),
            (
                Format::TarXz,
                TAR,
                vec!["-xJf", "/tmp/archive.tar.gz", "-C", "/tmp/dest"],
            ),
            (Format::Gz, GUNZIP, vec!["-c", "/tmp/archive.tar.gz"]),
        ] {
            let command = extractor_command(format, archive, dest);
            assert_eq!(command.get_program(), OsStr::new(bin));
            assert_eq!(
                command.get_args().collect::<Vec<_>>(),
                expected.iter().map(OsStr::new).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn plain_and_typed_listing_commands_are_pinned_at_the_process_boundary() {
        let archive = Path::new("/tmp/archive.tar.gz");
        for (format, bin, plain, typed) in [
            (Format::Zip, UNZIP, vec!["-Z1"], vec!["-Z"]),
            (Format::Tar, TAR, vec!["-tf"], vec!["-tvf"]),
            (Format::TarGz, TAR, vec!["-tzf"], vec!["-tvzf"]),
            (Format::TarBz2, TAR, vec!["-tjf"], vec!["-tvjf"]),
            (Format::TarXz, TAR, vec!["-tJf"], vec!["-tvJf"]),
        ] {
            for (typed_row, flags) in [(false, plain), (true, typed)] {
                let command = listing_command(format, archive, typed_row).unwrap();
                assert_eq!(command.get_program(), OsStr::new(bin));
                assert_eq!(
                    command.get_args().collect::<Vec<_>>(),
                    flags
                        .iter()
                        .map(OsStr::new)
                        .chain(std::iter::once(archive.as_os_str()))
                        .collect::<Vec<_>>()
                );
            }
        }
    }

    #[test]
    fn typed_zip_rows_keep_spaced_and_time_like_names() {
        let member = member_from_typed_line(
            "-rw-r--r--  3.0 unx        7 tx defN 31-Aug-26 12:30  café  notes 12:31.txt",
        )
        .expect("a zipinfo member row");
        assert_eq!(member.mode.as_deref(), Some("-rw-r--r--"));
        assert_eq!(member.path, " café  notes 12:31.txt");
    }

    #[test]
    fn typed_rows_support_bsd_tar_old_dates_and_literal_arrows() {
        let old = member_from_typed_line(
            "-rw-r--r--  0 festus wheel       1 Jan  1  2024 old file -> literal.txt",
        )
        .expect("a BSD tar member row with an old timestamp");
        assert_eq!(old.path, "old file -> literal.txt");

        let link = member_from_typed_line(
            "lrwxr-xr-x  0 festus wheel       0 Aug 31 18:50 link -> named -> /etc/passwd",
        )
        .expect("a BSD tar symlink row");
        assert_eq!(link.path, "link -> named");
    }

    #[test]
    fn typed_tar_rows_do_not_mistake_a_hyphenated_group_and_size_for_a_date() {
        let member = member_from_typed_line(
            "-rw-r--r--  0 festus staff-x    1234 Jan  1  2024 ordinary.txt",
        )
        .expect("a BSD tar row with a hyphenated group");
        assert_eq!(member.path, "ordinary.txt");
    }

    #[test]
    fn archive_anchor_keeps_private_bytes_when_the_source_inode_is_rewritten() {
        static TEST_NEXT: AtomicU64 = AtomicU64::new(0);
        let serial = TEST_NEXT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "unpack-anchor-copy-{}-{serial}",
            std::process::id()
        ));
        std::fs::create_dir(&root).expect("create test directory");
        let archive = root.join("input.zip");
        std::fs::write(&archive, b"checked bytes").expect("write archive");

        let anchor = ArchiveAnchor::create(&archive).expect("copy archive");
        std::fs::write(&archive, b"rewritten bytes").expect("rewrite archive inode");

        assert_eq!(
            std::fs::read(anchor.path()).expect("read private copy"),
            b"checked bytes"
        );
        drop(anchor);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn run_extracts_the_anchored_zip_when_the_input_path_is_replaced() {
        static TEST_NEXT: AtomicU64 = AtomicU64::new(0);
        let serial = TEST_NEXT.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("unpack-anchor-run-{}-{serial}", std::process::id()));
        std::fs::create_dir(&root).expect("create test directory");
        let source = root.join("checked.txt");
        let replacement_source = root.join("replacement.txt");
        let archive = root.join("input.zip");
        let replacement = root.join("replacement.zip");
        let dest = root.join("out");
        std::fs::write(&source, b"checked archive").expect("write checked content");
        std::fs::write(&replacement_source, b"replacement archive")
            .expect("write replacement content");
        for (input, output) in [(&source, &archive), (&replacement_source, &replacement)] {
            let status = Command::new("/usr/bin/zip")
                .args(["-q", "-j"])
                .arg(output)
                .arg(input)
                .status()
                .expect("build zip fixture");
            assert!(status.success(), "zip fixture creation failed");
        }

        *AFTER_PREFLIGHT.lock().expect("preflight hook lock") = Some(Box::new({
            let archive = archive.clone();
            let replacement = replacement.clone();
            move || std::fs::rename(replacement, archive).expect("replace archive after listings")
        }));
        let result = run(
            &archive,
            &[
                "--into".into(),
                dest.display().to_string(),
                "--max-size".into(),
                "1G".into(),
            ],
        );

        assert_eq!(result, ExitCode::SUCCESS);
        assert_eq!(
            std::fs::read(dest.join("checked.txt")).expect("read extracted file"),
            b"checked archive"
        );
        assert!(!dest.join("replacement.txt").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn run_preserves_the_bare_gz_output_name_through_the_anchor() {
        static TEST_NEXT: AtomicU64 = AtomicU64::new(0);
        let serial = TEST_NEXT.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("unpack-anchor-gz-{}-{serial}", std::process::id()));
        std::fs::create_dir(&root).expect("create test directory");
        let source = root.join("dump.sql");
        let archive = root.join("dump.sql.gz");
        let dest = root.join("out");
        std::fs::write(&source, b"select 1;\n").expect("write gzip fixture");
        let compressed = Command::new("/usr/bin/gzip")
            .args(["-c"])
            .arg(&source)
            .output()
            .expect("compress gzip fixture");
        assert!(compressed.status.success(), "gzip fixture creation failed");
        std::fs::write(&archive, compressed.stdout).expect("write gzip archive");

        let result = run(
            &archive,
            &[
                "--into".into(),
                dest.display().to_string(),
                "--max-size".into(),
                "1G".into(),
            ],
        );

        assert_eq!(result, ExitCode::SUCCESS);
        assert_eq!(
            std::fs::read(dest.join("dump.sql")).expect("read extracted file"),
            b"select 1;\n"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
