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

Handles .zip .tar .tar.gz .tgz .tar.bz2 .tar.xz .gz .dmg using the tools
already on this machine. Nothing is parsed here.

Every archive is listed and judged BEFORE anything is written. Paths that
escape the target, absolute paths and drive paths are refused outright.";

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
             Handles .zip .tar .tar.gz .tgz .tar.bz2 .tar.xz .gz .dmg"
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
    if let Err(e) = std::fs::create_dir_all(&dest) {
        eprintln!("unpack: cannot create the target ({})", e.kind());
        return ExitCode::from(3);
    }
    if let Err(e) = extract(archive, fmt, &dest) {
        let _ = std::fs::remove_dir_all(&dest);
        eprintln!("unpack: extraction failed ({e}). Nothing was left behind.");
        return ExitCode::from(3);
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
        .map_err(|e| e.kind().to_string())?;
    if !out.status.success() {
        return Err("listing failed".into());
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim_end_matches('/').to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

fn extract(archive: &Path, fmt: Format, dest: &Path) -> Result<(), String> {
    let status = match fmt {
        // -o overwrite inside our own fresh dir, -q quiet.
        Format::Zip => Command::new("unzip")
            .args(["-oq"])
            .arg(archive)
            .arg("-d")
            .arg(dest)
            .status(),
        Format::Gz => {
            let out_path = dest.join(stem(archive));
            let f = std::fs::File::create(&out_path).map_err(|e| e.kind().to_string())?;
            Command::new("gunzip")
                .arg("-c")
                .arg(archive)
                .stdout(f)
                .status()
        }
        _ => {
            let flag = match fmt {
                Format::Tar => "-xf",
                Format::TarGz => "-xzf",
                Format::TarBz2 => "-xjf",
                Format::TarXz => "-xJf",
                _ => unreachable!("dmg and zip handled above"),
            };
            Command::new("tar")
                .arg(flag)
                .arg(archive)
                .arg("-C")
                .arg(dest)
                .status()
        }
    }
    .map_err(|e| e.kind().to_string())?;

    if status.success() {
        Ok(())
    } else {
        Err("extractor reported failure".into())
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
