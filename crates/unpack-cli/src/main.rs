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

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use safety::Unsafe;

const USAGE: &str = "\
unpack: stop thinking about archive formats

USAGE
    unpack ARCHIVE [--into DIR]    extract safely into its own directory
    unpack ARCHIVE --list          show what is inside, extract nothing
    --json                         machine-readable output (for agents)
    --version                      print the version and exit
    unpack help

Handles .zip .tar .tar.gz .tgz .tar.bz2 .tar.xz .gz using the tools already
on this machine. Nothing is parsed here.

.dmg is recognised and refused. Extracting one means asking the kernel to
mount a stranger's filesystem image, which is a different and larger risk
than running unzip against a stream, and it is not implemented.

Every archive is listed and judged BEFORE anything is written. Paths that
escape the target, absolute paths and drive paths are refused outright.

Extraction stops if an archive writes more than its caps allow, and the
target is removed. The limit is on bytes that land on disk, never on the
size an archive claims for itself: those numbers are written by whoever
built it and forging four bytes makes unzip, tar and gzip all understate a
member by three orders of magnitude.";

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
        eprintln!(
            "unpack: .dmg is not implemented yet.\n\
             Mounting a disk image needs different handling from extracting an\n\
             archive, and pretending otherwise would be worse than saying so."
        );
        return ExitCode::from(3);
    }

    // --- 1. list -----------------------------------------------------------
    let entries = match list(archive, fmt) {
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
    let blocked: Vec<Unsafe> = entries.iter().filter_map(|p| safety::judge(p)).collect();
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
        eprintln!("\nunpack: REFUSED. This archive tries to write outside the target.\n");
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
    // Will it fit? The one question no header can answer, and the reason a
    // size cap alone is not enough: a machine with 200 MB free does not care
    // that the cap is 4 GB. Checked against the parent, since dest does not
    // exist yet.
    if let Some(free) = dest.parent().and_then(free_bytes)
        && free < MIN_FREE_BYTES
    {
        eprintln!(
            "unpack: refused. Only {} free on this volume, and extracting needs room\n\
             to work. Nothing was written.",
            human(free)
        );
        return ExitCode::from(2);
    }

    if let Err(e) = std::fs::create_dir_all(&dest) {
        // The OS text, not the Rust category -- "Permission denied
        // (os error 13)" tells a user what to do; "permission denied"
        // alone is the same words with the errno thrown away, and
        // "uncategorized error" tells them nothing at all.
        eprintln!("unpack: cannot create the target ({e})");
        return ExitCode::from(3);
    }
    match extract(archive, fmt, &dest) {
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
            match breach {
                Breach::Total(n) => eprintln!(
                    "unpack: refused. This archive wrote {} past the {} cap and was stopped.\n\
                     Nothing was left behind. The archive's own listing does not have to\n\
                     admit its size, so the limit is on what actually lands on disk.",
                    human(n),
                    human(safety::MAX_TOTAL_BYTES)
                ),
                Breach::Member(n) => eprintln!(
                    "unpack: refused. One file in this archive reached {}, past the {} cap\n\
                     for a single member, and extraction was stopped. Nothing was left behind.",
                    human(n),
                    human(safety::MAX_MEMBER_BYTES)
                ),
            }
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
                ("paths_checked", j::num(entries.len())),
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
    println!("  Checked {} paths before writing anything", entries.len());
    ExitCode::SUCCESS
}

/// Enumerate member paths without extracting.
fn list(archive: &Path, fmt: Format) -> Result<Vec<String>, String> {
    let (bin, args): (&str, Vec<&str>) = match fmt {
        Format::Zip => ("unzip", vec!["-Z1"]),
        Format::Tar => ("tar", vec!["-tf"]),
        Format::TarGz => ("tar", vec!["-tzf"]),
        Format::TarBz2 => ("tar", vec!["-tjf"]),
        Format::TarXz => ("tar", vec!["-tJf"]),
        // A bare .gz holds exactly one file, named by stripping .gz.
        Format::Gz => return Ok(vec![stem(archive)]),
        Format::Dmg => return Err("dmg unsupported".into()),
    };
    let out = Command::new(bin)
        .args(&args)
        .arg(archive)
        .stderr(Stdio::null())
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

/// Total bytes and largest single file under `dir`.
fn written(dir: &Path) -> (u64, u64) {
    fn walk(dir: &Path, total: &mut u64, largest: &mut u64) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            // symlink_metadata: a symlink's target is not what this extraction
            // wrote, and following one could count something outside dest.
            let Ok(md) = e.metadata() else { continue };
            if md.is_dir() {
                walk(&e.path(), total, largest);
            } else {
                let n = md.len();
                *total += n;
                if n > *largest {
                    *largest = n;
                }
            }
        }
    }
    let (mut t, mut l) = (0, 0);
    walk(dir, &mut t, &mut l);
    (t, l)
}

/// Why an extraction was abandoned partway.
pub enum Breach {
    Total(u64),
    Member(u64),
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
fn run_bounded(mut cmd: Command, dest: &Path) -> Result<Result<(), Breach>, String> {
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) => {
                // Check once more after exit: a fast extraction can finish
                // between two polls, and an unchecked archive is the thing
                // this function exists to prevent.
                let (total, largest) = written(dest);
                if largest > safety::MAX_MEMBER_BYTES {
                    return Ok(Err(Breach::Member(largest)));
                }
                if total > safety::MAX_TOTAL_BYTES {
                    return Ok(Err(Breach::Total(total)));
                }
                return if status.success() {
                    Ok(Ok(()))
                } else {
                    Err("extractor reported failure".into())
                };
            }
            None => {
                let (total, largest) = written(dest);
                if total > safety::MAX_TOTAL_BYTES || largest > safety::MAX_MEMBER_BYTES {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Ok(Err(if largest > safety::MAX_MEMBER_BYTES {
                        Breach::Member(largest)
                    } else {
                        Breach::Total(total)
                    }));
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    }
}

fn extract(archive: &Path, fmt: Format, dest: &Path) -> Result<Result<(), Breach>, String> {
    let cmd = match fmt {
        // -o overwrite inside our own fresh dir, -q quiet.
        Format::Zip => {
            let mut c = Command::new("unzip");
            c.args(["-oq"]).arg(archive).arg("-d").arg(dest);
            c
        }
        Format::Gz => {
            let out_path = dest.join(stem(archive));
            let f = std::fs::File::create(&out_path).map_err(|e| e.to_string())?;
            let mut c = Command::new("gunzip");
            c.arg("-c").arg(archive).stdout(f);
            c
        }
        _ => {
            let flag = match fmt {
                Format::Tar => "-xf",
                Format::TarGz => "-xzf",
                Format::TarBz2 => "-xjf",
                Format::TarXz => "-xJf",
                _ => unreachable!("dmg and zip handled above"),
            };
            let mut c = Command::new("tar");
            c.arg(flag).arg(archive).arg("-C").arg(dest);
            c
        }
    };
    run_bounded(cmd, dest)
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
}
