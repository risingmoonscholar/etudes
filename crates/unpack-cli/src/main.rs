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

mod quarantine;
mod safety;

use std::collections::HashMap;
use std::io::{BufRead, Read, Seek};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
#[cfg(unix)]
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
#[cfg(test)]
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use etude_cli_support::contract::{ARCHIVE_SUFFIXES, ArchiveFormat as Format};
use etude_cli_support::reason::{Code as ReasonCode, Reason, Subject};
use etude_cli_support::{counted, plain_io_error, quoted_path, shown, shown_text};
use safety::Unsafe;

// `unpack` deliberately targets the macOS system extractors.  Do not replace
// these with PATH lookups: the executable is part of the safety boundary.
const UNZIP: &str = "/usr/bin/unzip";
const TAR: &str = "/usr/bin/tar";
const GUNZIP: &str = "/usr/bin/gunzip";

#[cfg(unix)]
static SIGTERM_REQUESTED: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn note_sigterm(_signal: i32) {
    SIGTERM_REQUESTED.store(true, AtomicOrdering::Relaxed);
}

#[cfg(unix)]
fn install_sigterm_handler() {
    unsafe extern "C" {
        fn signal(signal: i32, handler: usize) -> usize;
    }
    // SAFETY: the handler only sets an atomic flag and performs no allocation,
    // I/O, locking, or other non-signal-safe work.
    let previous = unsafe { signal(15, note_sigterm as *const () as usize) };
    if previous == usize::MAX {
        eprintln!("unpack: could not install SIGTERM cleanup handler");
    }
}

#[cfg(not(unix))]
fn install_sigterm_handler() {}

const USAGE: &str = "\
unpack: stop thinking about archive formats

USAGE
    unpack ARCHIVE [--into DIR]    extract safely into its own directory
    unpack ARCHIVE --list          show what is inside, extract nothing
                                   options may also come before ARCHIVE
    unpack contract --json         print the versioned capability contract
    --max-size N[G|M]              set the size limit: a whole number above 0
    --json                         machine-readable output (for agents)
    --version                      print the version and exit
    unpack help

Handles .zip .jar .tar .tar.gz .tgz .tar.bz2 .tbz .tar.xz .txz .gz using the tools already
on this machine. Nothing is parsed here.

.dmg is recognised and refused, by design rather than pending. Opening one
means asking the kernel to mount a stranger's filesystem image, which is a
much larger thing than running unzip against a stream, and no size check
covers it. `hdiutil attach` does it if you decide to.

Every archive is listed and judged BEFORE anything is written. Paths that
escape the target, absolute paths and drive paths are refused outright.

Extraction uses a private folder beside the destination. Every extracted path
and member type is checked before the complete folder moves into place with one
rename. Failed runs remove that folder, or say where it is if they cannot.

The size limit defaults to half the free space on the target disk, kept between
1 GB and 64 GB. Bytes on disk are checked every 100 ms and after exit,
independently of archive headers. This is a soft limit, not a hard write cap:
extraction can go over between checks and while stopping. Refusals report the
final measured bytes and how far they went over. --max-size changes this limit;
README.md records how far over it has been measured to go.";

/// A private byte-for-byte copy of the archive we judged.
///
/// The callers below must use this copy rather than the pathname supplied on
/// the command line. Copying the caller's file once binds the listings and
/// extractor to private bytes even if somebody replaces the original pathname
/// or rewrites the original inode after preflight.
///
/// The copy has no name on disk. It is unlinked before its bytes are written,
/// so a SIGKILL at any later point cannot strand it, and the kernel frees it
/// when the last descriptor closes. Children read it through `/dev/fd/N`.
///
/// App Sandbox refuses that open for a file the sandboxed process created,
/// measured on macOS 27.0.1. There the copy keeps its private name, as it did
/// before, and `Drop` removes it; a SIGKILL can strand that one.
struct ArchiveAnchor {
    /// Read-only. Never handed to a child except through `prepare`.
    file: std::fs::File,
    path: PathBuf,
    /// The name kept on disk, only where `/dev/fd/N` cannot be opened.
    kept: Option<PathBuf>,
    /// The original basename, which names a bare .gz's output. `path` must
    /// never supply a name or reach the user.
    name: PathBuf,
    quarantine: Option<Vec<u8>>,
}

/// These errors reach here as text, so the errno is read back from what the
/// standard library printed.
fn permission_advice_for(message: &str) -> Option<&'static str> {
    let code = if message.contains("(os error 1)") {
        1
    } else if message.contains("(os error 13)") {
        13
    } else {
        return None;
    };
    etude_core::scan::permission_advice(&std::io::Error::from_raw_os_error(code))
}

#[derive(Debug)]
enum ArchiveAnchorError {
    Io(String),
    Symlink,
    QuarantineChanged,
}
impl std::fmt::Display for ArchiveAnchorError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(message) => formatter.write_str(message),
            Self::Symlink => formatter.write_str("the archive path is a link"),
            Self::QuarantineChanged => {
                formatter.write_str("archive quarantine changed while copying")
            }
        }
    }
}
impl From<std::io::Error> for ArchiveAnchorError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}
impl From<String> for ArchiveAnchorError {
    fn from(message: String) -> Self {
        Self::Io(message)
    }
}
impl From<&str> for ArchiveAnchorError {
    fn from(message: &str) -> Self {
        Self::Io(message.into())
    }
}

#[cfg(test)]
thread_local! {
    static DESCRIPTOR_PATH_REFUSED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static AFTER_ANCHOR_COPY: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

impl ArchiveAnchor {
    fn create(archive: &Path) -> Result<Self, ArchiveAnchorError> {
        // Copy rather than hard-link: a link pins an inode, but an attacker
        // holding a writable descriptor can still truncate and rewrite that
        // inode after both preflights. The private copy is the one immutable
        // input all later operations consume.
        match etude_core::scan::observe_read("archive_metadata", std::fs::symlink_metadata(archive))
        {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(ArchiveAnchorError::Symlink);
            }
            Ok(_) => {}
            Err(error) => return Err(error.into()),
        }
        let mut source = quarantine::open_archive(archive)?;
        let mark = quarantine::capture(&source)?;
        let (mut output, file, kept) = Self::create_private()?;
        let anchor = Self {
            path: kept
                .clone()
                .unwrap_or_else(|| PathBuf::from(format!("/dev/fd/{}", file.as_raw_fd()))),
            file,
            kept,
            name: PathBuf::from(
                archive
                    .file_name()
                    .unwrap_or(std::ffi::OsStr::new("archive")),
            ),
            quarantine: mark.clone(),
        };
        etude_core::scan::observe_read("archive_bytes", std::io::copy(&mut source, &mut output))?;
        #[cfg(test)]
        AFTER_ANCHOR_COPY.with(|hook| {
            if let Some(action) = hook.borrow_mut().take() {
                action();
            }
        });
        if quarantine::capture(&source)? != mark {
            return Err(ArchiveAnchorError::QuarantineChanged);
        }
        Ok(anchor)
    }

    /// Return a write handle and a read-only handle to a new file, and the
    /// file's name if it had to keep one.
    fn create_private()
    -> Result<(std::fs::File, std::fs::File, Option<PathBuf>), ArchiveAnchorError> {
        for _ in 0..128 {
            // A counter plus PID is visible to any process and makes this
            // supposedly private filename predictable.  Read enough bytes
            // from the kernel CSPRNG that another process cannot derive the
            // pathname and replace the file before it is opened again.
            let mut nonce = [0_u8; 16];
            etude_core::scan::observe_read(
                "os_random_bytes",
                std::fs::File::open("/dev/urandom")
                    .and_then(|mut random| random.read_exact(&mut nonce)),
            )
            .map_err(|error| format!("could not generate private anchor name: {error}"))?;
            let path = std::env::temp_dir().join(format!(
                "unpack-{}",
                nonce
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
            ));
            let output = match std::fs::File::options()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
            {
                Ok(output) => output,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            };
            // The name exists only across these calls, and the file is still
            // empty. Nothing else may run here: a kill in this window is the
            // one way to leave it behind.
            let reader = std::fs::File::open(&path);
            let reachable = reader.as_ref().is_ok_and(descriptor_path_opens);
            let unlinked = if reachable {
                std::fs::remove_file(&path)
            } else {
                Ok(())
            };
            let checked = (|| -> Result<std::fs::File, ArchiveAnchorError> {
                let reader = reader?;
                unlinked?;
                let (written, read) = (output.metadata()?, reader.metadata()?);
                if (written.dev(), written.ino()) != (read.dev(), read.ino()) {
                    return Err("the private archive copy was replaced".into());
                }
                Ok(reader)
            })();
            return match checked {
                Ok(reader) => Ok((
                    output,
                    own_descriptor_number(reader),
                    (!reachable).then_some(path),
                )),
                Err(error) => {
                    let _ = std::fs::remove_file(&path);
                    Err(error)
                }
            };
        }
        Err("could not create a private archive anchor".into())
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn name(&self) -> &Path {
        &self.name
    }

    fn len(&self) -> std::io::Result<u64> {
        Ok(self.file.metadata()?.len())
    }

    /// Up to `limit` bytes from the start of the copy. Reading at an offset
    /// leaves the descriptor where a child expects to find it.
    fn head(&self, limit: usize) -> std::io::Result<Vec<u8>> {
        let mut bytes = vec![0; limit];
        let mut filled = 0;
        while filled < limit {
            let got = self.file.read_at(&mut bytes[filled..], filled as u64)?;
            if got == 0 {
                break;
            }
            filled += got;
        }
        bytes.truncate(filled);
        Ok(bytes)
    }

    /// Give one child the copy, at its start.
    ///
    /// `/dev/fd/N` shares the descriptor's offset on macOS, so each child
    /// needs a rewind. The descriptor stays close-on-exec in this process, so
    /// only a command prepared here inherits it.
    fn prepare(&self, command: &mut Command) -> Result<(), String> {
        if self.kept.is_some() {
            return Ok(());
        }
        (&self.file)
            .seek(std::io::SeekFrom::Start(0))
            .map_err(|error| error.to_string())?;
        let fd = self.file.as_raw_fd();
        // SAFETY: `fcntl` is async-signal-safe, and the closure touches only
        // the copied integer and the error number between fork and exec.
        unsafe {
            command.pre_exec(move || {
                if fcntl(fd, F_SETFD, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        Ok(())
    }
}

impl Drop for ArchiveAnchor {
    fn drop(&mut self) {
        if let Some(kept) = &self.kept {
            let _ = std::fs::remove_file(kept);
        }
    }
}

/// Move the descriptor to a number that other runs are unlikely to hold.
///
/// Observed on macOS 27.0.1: `stat` on `/dev/fd/N` can fail with EBADF while
/// another process uses the same N, and `unzip` calls `stat` first. A run that
/// cannot move keeps the number it has, because that number still works
/// almost every time. Every number here is below the default limit of 256.
fn own_descriptor_number(file: std::fs::File) -> std::fs::File {
    let wanted = 64 + (std::process::id() % 192) as i32;
    // SAFETY: `fcntl` reads only its integer arguments, and a returned
    // descriptor is new, so this function alone owns it.
    unsafe {
        let moved = fcntl(file.as_raw_fd(), F_DUPFD_CLOEXEC, wanted);
        if moved == -1 {
            return file;
        }
        std::os::fd::FromRawFd::from_raw_fd(moved)
    }
}

fn descriptor_path_opens(file: &std::fs::File) -> bool {
    #[cfg(test)]
    if DESCRIPTOR_PATH_REFUSED.with(|refused| refused.get()) {
        return false;
    }
    std::fs::File::open(format!("/dev/fd/{}", file.as_raw_fd())).is_ok()
}

// Source: fcntl(2), and F_SETFD in the SDK's sys/fcntl.h.
const F_SETFD: i32 = 2;
// Source: F_DUPFD_CLOEXEC in the SDK's sys/fcntl.h.
const F_DUPFD_CLOEXEC: i32 = 67;
unsafe extern "C" {
    fn fcntl(fd: i32, cmd: i32, ...) -> i32;
}

// This seam exists only to make the pathname-replacement regression
// deterministic.  It is compiled out of the shipped binary, so callers
// cannot influence either the extractor path or the preflight/extract gap.
#[cfg(test)]
thread_local! {
    static AFTER_PREFLIGHT: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn after_preflight_for_test() {
    let action = AFTER_PREFLIGHT.with(|hook| hook.borrow_mut().take());
    if let Some(action) = action {
        action();
    }
}

#[cfg(not(test))]
fn after_preflight_for_test() {}

fn detect(path: &Path) -> Option<Format> {
    let n = path.file_name()?.to_string_lossy().to_ascii_lowercase();
    // Longest suffix first: .tar.gz must win over .gz.
    for &(suffix, f) in ARCHIVE_SUFFIXES {
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

macro_rules! println {
    () => { etude_cli_support::envelope::print(String::new()) };
    ($($arg:tt)*) => { etude_cli_support::envelope::print(format!($($arg)*)) };
}

fn refusal(code: ReasonCode, subject: Option<Subject>) -> ExitCode {
    etude_cli_support::envelope::reason(Reason::refusal(code, subject));
    ExitCode::from(2)
}

fn failure(code: ReasonCode, subject: Option<Subject>) -> ExitCode {
    etude_cli_support::envelope::reason(Reason::refusal(code, subject));
    ExitCode::from(3)
}

fn input_subject(path: &Path) -> Option<Subject> {
    Some(Subject::path("input", &etude_core::redact::path(path)))
}

fn destination_subject(path: &Path) -> Option<Subject> {
    Some(Subject::path(
        "destination",
        &etude_core::redact::path(path),
    ))
}

/// An argument mistake: what to say, and whether it is an unknown option.
#[derive(Debug, PartialEq, Eq)]
enum BadArgument {
    UnknownOption(String),
    Invalid(String),
}

impl BadArgument {
    fn refuse(&self) -> ExitCode {
        match self {
            Self::UnknownOption(option) => {
                eprintln!(
                    "unpack: unknown option {}. It handles --list, --json, --into DIR and \
                     --max-size SIZE. Run `unpack help`.",
                    shown_text(option)
                );
                refusal(ReasonCode::UnknownOption, None)
            }
            Self::Invalid(message) => {
                eprintln!("unpack: {message}");
                refusal(ReasonCode::InvalidArguments, None)
            }
        }
    }
}

/// What this format is called in a sentence, with its article.
fn format_name(fmt: Format) -> &'static str {
    match fmt {
        Format::Zip => "a ZIP",
        Format::Tar => "a tar archive",
        Format::TarGz => "a .tar.gz archive",
        Format::TarBz2 => "a .tar.bz2 archive",
        Format::TarXz => "a .tar.xz archive",
        Format::Gz => "a gzip file",
        Format::Dmg => "a disk image",
    }
}

/// The command that extracts this archive without unpack, for an archive the
/// person trusts and unpack will not take.
fn by_hand(fmt: Format, archive: &Path, dest: &Path) -> String {
    let (archive, dest) = (quoted_path(archive), quoted_path(dest));
    match fmt {
        Format::Zip => format!("unzip {archive} -d {dest}"),
        Format::Gz => format!("gunzip -k {archive}"),
        _ => format!("mkdir {dest} && tar -xf {archive} -C {dest}"),
    }
}

/// The command that shows what is inside without unpack.
fn look_by_hand(fmt: Format, archive: &Path) -> String {
    let archive = quoted_path(archive);
    match fmt {
        Format::Zip => format!("unzip -l {archive}"),
        _ => format!("tar -tf {archive}"),
    }
}

/// A file that cannot be read as what its name says.
fn unreadable_message(archive: &Path, fmt: Format) -> String {
    let kind = format_name(fmt);
    format!(
        "unpack: {} could not be read as {kind}. It may be damaged, incomplete, or not {kind}. \
         Nothing was extracted. If you downloaded it, try downloading it again.",
        shown(archive)
    )
}

fn unreadable(archive: &Path, fmt: Format) -> ExitCode {
    eprintln!("{}", unreadable_message(archive, fmt));
    failure(ReasonCode::ArchiveUnreadable, input_subject(archive))
}

/// The whole file is one end-of-central-directory record that counts no
/// entries, on one disk, with a central directory of no bytes at offset 0. Any
/// other bytes around it mean a damaged file rather than an empty archive.
fn is_empty_zip(bytes: &[u8]) -> bool {
    const RECORD: usize = 22;
    bytes.len() >= RECORD
        && bytes.starts_with(b"PK\x05\x06")
        && bytes[4..20].iter().all(|byte| *byte == 0)
        && usize::from(u16::from_le_bytes([bytes[20], bytes[21]])) == bytes.len() - RECORD
}

/// The largest empty ZIP: its record plus the longest comment one can hold.
const EMPTY_ZIP_MAX_BYTES: usize = 22 + 65_535;

/// Does this start like something `gunzip` can read? Besides gzip, the macOS
/// `gunzip` also reads compress, bzip2 and xz data under a .gz name, measured on
/// macOS 27.0. Calling those unreadable would be false, since extraction works.
fn gunzip_reads(head: &[u8]) -> bool {
    head.starts_with(&[0x1f, 0x8b, 0x08])
        || head.starts_with(&[0x1f, 0x9d])
        || head.starts_with(b"BZh")
        || head.starts_with(&[0xfd, b'7', b'z', b'X', b'Z', 0x00])
}

fn type_unknown_message(archive: &Path, fmt: Format, dest: &Path) -> String {
    format!(
        "unpack: REFUSED. {} lists its names but not what kind of item each one is, so unpack \
         cannot tell whether it holds links or device files. Nothing was extracted.\n\
         To look inside: {}\n\
         If you trust this archive, extract it by hand: {}",
        shown(archive),
        look_by_hand(fmt, archive),
        by_hand(fmt, archive, dest)
    )
}

fn untyped_member_message(archive: &Path, fmt: Format, dest: &Path, member: &str) -> String {
    format!(
        "unpack: REFUSED. {} lists {} without saying what kind of item it is, so unpack will \
         not extract it. Nothing was extracted.\n\
         If you trust this archive, extract it by hand: {}",
        shown(archive),
        shown_text(&format!("{member:?}")),
        by_hand(fmt, archive, dest)
    )
}

fn too_many_entries_message(archive: &Path, fmt: Format, dest: &Path, count: usize) -> String {
    format!(
        "unpack: {} holds {count} entries, and unpack stops at {}. Nothing was extracted.\n\
         If you trust this archive, extract it by hand: {}",
        shown(archive),
        safety::MAX_ENTRIES,
        by_hand(fmt, archive, dest)
    )
}

fn quarantine_changed_message(archive: &Path) -> String {
    format!(
        "unpack: the download warning mark on {} changed while unpack was copying it. \
         Nothing was extracted. Run unpack again.",
        shown(archive)
    )
}

fn quarantine_failed_message(
    error: &std::io::Error,
    dest: &Path,
    cleanup: &str,
    by_hand: &str,
) -> String {
    let next = if permission_denied(error) {
        format!(
            "A file in the archive may be locked. If you trust this archive, extract it by \
             hand: {by_hand}"
        )
    } else {
        "Try another place with --into DIR.".into()
    };
    format!(
        "unpack: could not carry the download warning mark onto the extracted files: {}. \
         Nothing was placed at {}. {cleanup}\n{next}",
        plain_io_error(error),
        shown(dest)
    )
}

fn permission_denied(error: &std::io::Error) -> bool {
    matches!(error.raw_os_error(), Some(1 | 13))
}

fn limit_message(
    written: u64,
    limit: u64,
    chosen: bool,
    cleanup: &str,
    archive: &Path,
    into: Option<&Path>,
) -> String {
    let opening = if chosen {
        "stopped at your --max-size limit"
    } else {
        "stopped at the default size limit"
    };
    let why = if chosen {
        String::new()
    } else {
        "\nThe default is half the free space on this disk, kept between 1 GB and 64 GB. A big \
         project and a decompression bomb look the same from here, so this says nothing about \
         the archive being hostile."
            .into()
    };
    let into = into
        .map(|folder| format!(" --into {}", quoted_path(folder)))
        .unwrap_or_default();
    format!(
        "unpack: {opening}. It wrote {} before stopping; the limit is {}, so it went {} over. \
         {cleanup}{why}\n\
         To allow more, if you trust this archive: unpack {}{into} --max-size SIZE\n\
         SIZE is a whole number, optionally with M or G.",
        exact_bytes(written),
        exact_bytes(limit),
        exact_bytes(written.saturating_sub(limit)),
        quoted_path(archive)
    )
}

/// An operating system error that reached here as text, without its number.
fn plain_text(message: &str) -> String {
    let mut plain = String::with_capacity(message.len());
    let mut rest = message;
    while let Some(start) = rest.find(" (os error ") {
        plain.push_str(&rest[..start]);
        rest = match rest[start..].find(')') {
            Some(end) => &rest[start + end + 1..],
            None => "",
        };
    }
    plain.push_str(rest);
    plain
}

fn main() -> ExitCode {
    install_sigterm_handler();
    let args: Vec<String> = std::env::args().skip(1).collect();
    etude_cli_support::envelope::begin("unpack", env!("CARGO_PKG_VERSION"), &args);
    let code = std::panic::catch_unwind(run_main)
        .unwrap_or_else(|_| failure(ReasonCode::InternalError, None));
    etude_cli_support::envelope::finish(code);
    code
}

fn run_main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "contract") {
        let code = etude_cli_support::contract::command(
            etude_cli_support::contract::Tool::Unpack,
            env!("CARGO_PKG_VERSION"),
            &args,
            &[],
            None,
        );
        if code != ExitCode::SUCCESS {
            etude_cli_support::envelope::reason(Reason::refusal(
                ReasonCode::InvalidArguments,
                None,
            ));
        }
        return code;
    }
    if etude_cli_support::wants_help(&args) {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let args = archive_first(args);
    if let Err(problem) = validate_args(&args) {
        return problem.refuse();
    }
    match args.first().map(String::as_str) {
        None | Some("help" | "--help" | "-h") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some("version" | "--version" | "-V") => {
            println!("unpack {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        // Every option was recognized above and no archive was found among the
        // words, so the archive is the one thing missing.
        Some(p) if p.starts_with('-') => {
            eprintln!(
                "unpack: {p} describes what to do with an archive, so it needs one.\n     \
                 unpack ARCHIVE {p}"
            );
            refusal(ReasonCode::InvalidArguments, None)
        }
        Some(p) => run(&PathBuf::from(expand_tilde(p)), &args),
    }
}

/// Move the archive to the front when options came first, so the checks below
/// see one order. A value belonging to --into or --max-size is never taken for
/// the archive, and a word this stops at stays where it is, to be refused.
fn archive_first(mut args: Vec<String>) -> Vec<String> {
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--list" | "--json" => i += 1,
            "--into" | "--max-size" => i += 2,
            "help" | "version" | "contract" => break,
            word if word.starts_with('-') => break,
            _ => {
                let archive = args.remove(i);
                args.insert(0, archive);
                break;
            }
        }
    }
    args
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

/// Reject typos and incomplete options before reading the archive or creating
/// output. Every argument after the archive must be a recognized option or the
/// value consumed by one. Without an archive, every argument must be one too.
fn validate_args(args: &[String]) -> Result<(), BadArgument> {
    let _ = max_size_flag(args)?;
    for option in ["--into", "--max-size"] {
        if args.iter().filter(|arg| *arg == option).count() > 1 {
            return Err(BadArgument::Invalid(format!(
                "{option} was given more than once, and only the first would have been used. \
                 Remove the ones you do not mean."
            )));
        }
    }
    let no_archive = args.first().is_some_and(|first| first.starts_with('-'))
        && !matches!(args[0].as_str(), "--version" | "-V");
    let mut i = usize::from(!no_archive);
    while i < args.len() {
        match args[i].as_str() {
            "--list" | "--json" => i += 1,
            "--into" => {
                let Some(directory) = args.get(i + 1) else {
                    return Err(BadArgument::Invalid(
                        "--into needs a folder: --into DIR.".into(),
                    ));
                };
                // With no archive at all, that is the mistake to report first.
                if no_archive && directory.starts_with('-') {
                    i += 1;
                    continue;
                }
                if directory.starts_with('-') {
                    return Err(BadArgument::Invalid(format!(
                        "--into needs a folder, got \"{}\".",
                        shown_text(directory)
                    )));
                }
                i += 2;
            }
            // max_size_flag has already judged the value.
            "--max-size" => i += 2,
            option if option.starts_with('-') && !option.contains(char::is_whitespace) => {
                return Err(BadArgument::UnknownOption(option.into()));
            }
            word => {
                return Err(BadArgument::Invalid(format!(
                    "unexpected word \"{}\". Give one archive and any options. \
                     Run `unpack help`.",
                    shown_text(word)
                )));
            }
        }
    }
    Ok(())
}

fn run(archive: &Path, args: &[String]) -> ExitCode {
    match etude_core::scan::observe_read("archive_metadata", std::fs::metadata(archive)) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => {
            eprintln!("unpack: not a file: {}", shown(archive));
            return failure(ReasonCode::MissingInput, input_subject(archive));
        }
        // "Not a directory" is a path that runs through a regular file: it
        // names nothing, exactly as a missing archive does.
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            eprintln!("unpack: no such file: {}", shown(archive));
            return failure(ReasonCode::MissingInput, input_subject(archive));
        }
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            eprintln!("unpack: cannot read {}: permission denied.", shown(archive));
            if let Some(advice) = etude_core::scan::permission_advice(&error) {
                eprintln!("{advice}");
            }
            return failure(ReasonCode::PermissionDenied, input_subject(archive));
        }
        Err(error) => {
            eprintln!(
                "unpack: cannot read {}: {}.",
                shown(archive),
                plain_io_error(&error)
            );
            return failure(ReasonCode::InternalError, None);
        }
    }
    let lowercase = archive.to_string_lossy().to_ascii_lowercase();
    if etude_core::scan::IN_FLIGHT_SUFFIXES
        .iter()
        .any(|suffix| lowercase.ends_with(suffix))
    {
        eprintln!(
            "unpack: {} looks like a download still in progress. Try again when it finishes.",
            shown(archive)
        );
        return refusal(
            ReasonCode::DownloadInFlight,
            Some(Subject::path(
                "download_in_flight",
                &etude_core::redact::path(archive),
            )),
        );
    }
    let Some(fmt) = detect(archive) else {
        let handled = ARCHIVE_SUFFIXES
            .iter()
            .filter(|(_, format)| *format != Format::Dmg)
            .map(|(suffix, _)| *suffix)
            .collect::<Vec<_>>();
        let not_handled = archive
            .extension()
            .map(|ext| format!(".{} is not supported", shown_text(&ext.to_string_lossy())))
            .unwrap_or_else(|| format!("{} has no archive suffix", shown(archive)));
        eprintln!("unpack: {not_handled}. It handles {}.", handled.join(" "));
        return refusal(ReasonCode::UnsupportedFormat, None);
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
        return refusal(ReasonCode::UnsupportedFormat, None);
    }

    // Copy the input once, before either listing. From this point onward no
    // process is handed the caller-controlled pathname: both preflights and
    // extraction consume the same private bytes, even if that pathname is
    // replaced or rewritten in the meantime.
    let pinned = match ArchiveAnchor::create(archive) {
        Ok(pinned) => pinned,
        Err(ArchiveAnchorError::QuarantineChanged) => {
            eprintln!("{}", quarantine_changed_message(archive));
            return refusal(ReasonCode::UnsafeInput, input_subject(archive));
        }
        Err(ArchiveAnchorError::Symlink) => {
            let real = archive
                .canonicalize()
                .map(|path| format!(": unpack {}", quoted_path(&path)))
                .unwrap_or_default();
            eprintln!(
                "unpack: {} is a link, and unpack works only on the real file. \
                 Name the file it points to{real}",
                shown(archive)
            );
            return failure(ReasonCode::UnsafeInput, input_subject(archive));
        }
        Err(e) => {
            let text = e.to_string();
            eprintln!(
                "unpack: cannot read {}: {}. Nothing was extracted.",
                shown(archive),
                plain_text(&text)
            );
            if let Some(advice) = permission_advice_for(&text) {
                eprintln!("{advice}");
                return failure(ReasonCode::PermissionDenied, input_subject(archive));
            }
            return failure(ReasonCode::InternalError, None);
        }
    };

    // Decided here so a refusal below can print a command that works. Nothing
    // is created at this path until step 4.
    let dest = match value(args, "--into") {
        Some(d) => PathBuf::from(expand_tilde(&d)),
        None => archive
            .parent()
            .unwrap_or(Path::new("."))
            .join(stem(archive)),
    };

    // A file of no bytes reads as an empty tar and as an empty stream, and says
    // nothing true about what it was meant to hold.
    if pinned.len().is_ok_and(|len| len == 0) {
        eprintln!(
            "unpack: {} is 0 bytes, so it is not {}. Nothing was extracted. \
             If you downloaded it, try downloading it again.",
            shown(archive),
            format_name(fmt)
        );
        return failure(ReasonCode::ArchiveUnreadable, input_subject(archive));
    }
    let listing_only = flag(args, "--list");
    if fmt == Format::Gz && listing_only && !pinned.head(6).is_ok_and(|head| gunzip_reads(&head)) {
        return unreadable(archive, fmt);
    }

    // --- 1. list -----------------------------------------------------------
    let entries = match list(&pinned, fmt) {
        Ok(e) => e,
        Err(ListError::Unreadable) => {
            // unzip answers an empty ZIP as a failure. The record decides it,
            // so a damaged file never gets this answer.
            let empty = fmt == Format::Zip
                && pinned
                    .head(EMPTY_ZIP_MAX_BYTES + 1)
                    .is_ok_and(|bytes| is_empty_zip(&bytes));
            if !empty {
                return unreadable(archive, fmt);
            }
            Vec::new()
        }
        Err(ListError::Tool(e)) => {
            eprintln!(
                "unpack: could not check {}: {}. Nothing was extracted.",
                shown(archive),
                plain_text(&e)
            );
            return failure(ReasonCode::InternalError, None);
        }
    };
    if entries.is_empty() {
        eprintln!("unpack: the archive is empty. Nothing was extracted.");
        return ExitCode::from(1);
    }
    if entries.len() > safety::MAX_ENTRIES {
        eprintln!(
            "{}",
            too_many_entries_message(archive, fmt, &dest, entries.len())
        );
        return refusal(ReasonCode::UnsafeInput, input_subject(archive));
    }

    // --- 2. judge, before anything is written ------------------------------
    // Two judgements, and the second is the one a name-only listing could not
    // make. Lexical first (traversal, absolute, depth) on every member; then
    // TYPE, from the verbose listing's mode string -- symlink, hard link,
    // device node, setuid. A member is refused if either says so.
    let mut blocked: Vec<Unsafe> = entries.iter().filter_map(|p| safety::judge(p)).collect();
    let TypedListing { members, encrypted } = match list_members(&pinned, fmt) {
        Ok(listing) => {
            if let Err(missing) = typed_rows_cover_entries(&entries, &listing.members) {
                eprintln!("{}", untyped_member_message(archive, fmt, &dest, &missing));
                return refusal(ReasonCode::UnsafeInput, input_subject(archive));
            }
            listing
        }
        Err(_) => {
            // The verbose listing failed where the plain one worked. That is
            // a parse disagreement between two reads of the same archive, and
            // The rule is: never treat agreement as proof, and never
            // treat a failed check as a pass. Refuse rather than extract with
            // one of the two judgements missing.
            eprintln!("{}", type_unknown_message(archive, fmt, &dest));
            return refusal(ReasonCode::UnsafeInput, input_subject(archive));
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
    let password_protected = encrypted > 0;

    if listing_only && flag(args, "--json") {
        // Lets an agent inspect an archive and decide, without extracting.
        use etude_core::json as j;
        println!(
            "{}",
            j::obj(&[
                ("archive", j::path(archive)),
                ("entries", j::num(entries.len())),
                ("junk", j::num(junk)),
                ("safe", j::bool(blocked.is_empty() && !password_protected)),
                ("password_protected", j::bool(password_protected)),
                ("contents_verified", j::bool(fmt != Format::Gz)),
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

    if listing_only {
        if fmt == Format::Gz {
            println!(
                "\n{} would extract to one file named {}.",
                shown(pinned.name()),
                shown_text(&entries[0])
            );
            println!("Its contents are not verified: gzip has no member list.");
            return ExitCode::SUCCESS;
        }
        println!(
            "\n{} in {}",
            counted(entries.len(), "entry", "entries"),
            stem(archive)
        );
        for p in entries.iter().take(40) {
            println!("  {p}");
        }
        if entries.len() > 40 {
            println!("  … {} more", entries.len() - 40);
        }
        if !blocked.is_empty() {
            println!(
                "\n  {} would be refused:",
                counted(blocked.len(), "unsafe path", "unsafe paths")
            );
            for b in &blocked {
                println!("    {}", shown_text(&b.to_string()));
            }
        }
        if password_protected {
            println!(
                "\n  This archive is password-protected, so unpack would not extract it.\n  \
                 Open it with unzip, which asks for the password: {}",
                by_hand(fmt, archive, &dest)
            );
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
        return refusal(ReasonCode::UnsafeInput, input_subject(archive));
    }

    if !blocked.is_empty() {
        // Refuse the whole archive. Extracting the safe subset of a hostile
        // archive gives the attacker a partial success and the user a mess.
        eprintln!("\nunpack: REFUSED. This archive holds items unpack will not create.\n");
        for b in blocked.iter().take(10) {
            eprintln!("  {}", shown_text(&b.to_string()));
        }
        if blocked.len() > 10 {
            eprintln!("  … and {} more", blocked.len() - 10);
        }
        eprintln!(
            "\nNothing was extracted. See everything inside it with: unpack {} --list",
            quoted_path(archive)
        );
        return refusal(ReasonCode::UnsafeInput, input_subject(archive));
    }

    // Asked only after the safety judgement, so an archive that is both is
    // refused for the graver reason.
    if password_protected {
        eprintln!(
            "unpack: {} is password-protected, which unpack does not handle. Nothing was \
             extracted.\nOpen it with unzip, which asks for the password: {}",
            shown(archive),
            by_hand(fmt, archive, &dest)
        );
        return refusal(ReasonCode::UnsupportedFormat, None);
    }

    after_preflight_for_test();

    // --- 3. choose a destination that cannot explode into the cwd ----------
    // Not `exists()`: that follows a link, so a broken one would look vacant.
    if dest.symlink_metadata().is_ok() {
        eprintln!(
            "unpack: {} already exists. Pass --into DIR to choose another.",
            shown(&dest)
        );
        return refusal(ReasonCode::DestinationExists, destination_subject(&dest));
    }

    // --- 4. extract --------------------------------------------------------
    // How much room is there, and therefore how much may this write? Asked of
    // the parent, since dest does not exist yet. This is the question no
    // header can answer, and the reason the bound is not a constant: a fixed
    // 4 GB refuses an ordinary project on a roomy disk and permits filling a
    // full one.
    let free = dest.parent().and_then(free_bytes);
    let chosen_limit = match max_size_flag(args) {
        Ok(limit) => limit,
        Err(problem) => return problem.refuse(),
    };
    let budget = chosen_limit.unwrap_or_else(|| safety::budget(free));
    match free {
        Some(f) if f < MIN_FREE_BYTES => {
            eprintln!(
                "unpack: only {} free on this disk, and unpack needs {} free to start. \
                 Nothing was written. Free some space, or choose another disk with --into DIR.",
                human(f),
                human(MIN_FREE_BYTES)
            );
            return refusal(
                ReasonCode::InsufficientSpace,
                Some(Subject::wait("destination", None)),
            );
        }
        _ => {}
    }

    // Extract into a private sibling, then publish it with one rename.  A
    // SIGKILL cannot run cleanup and may leave the system extractor alive;
    // staging keeps that orphan's partial tree out of the requested path so a
    // retry is never refused because a half-extraction already exists.
    let staging = match staging_destination(&dest) {
        Ok(path) => path,
        Err(e) => {
            let folder = dest
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            if let Some(advice) = etude_core::scan::permission_advice(&e) {
                eprintln!(
                    "unpack: cannot write to {}: permission denied. Nothing was extracted.\n\
                     {advice}\n\
                     Or choose another place with --into DIR.",
                    shown(folder)
                );
                return failure(ReasonCode::PermissionDenied, input_subject(&dest));
            }
            let missing = match e.kind() {
                std::io::ErrorKind::NotFound => format!(
                    "the folder {} does not exist, so {} cannot be made there. \
                     Nothing was extracted. Make the folder first: mkdir -p {}",
                    shown(folder),
                    shown(&dest),
                    quoted_path(folder)
                ),
                std::io::ErrorKind::NotADirectory => format!(
                    "part of {} is a file, not a folder, so {} cannot be made there. \
                     Nothing was extracted. Choose another place with --into DIR.",
                    shown(folder),
                    shown(&dest)
                ),
                _ => {
                    eprintln!(
                        "unpack: cannot write in {}: {}. Nothing was extracted. \
                         Choose another place with --into DIR.",
                        shown(folder),
                        plain_io_error(&e)
                    );
                    return failure(ReasonCode::InternalError, None);
                }
            };
            eprintln!("unpack: {missing}");
            return failure(ReasonCode::MissingInput, input_subject(&dest));
        }
    };
    match extract(&pinned, fmt, &staging, budget) {
        Err(ExtractError::Stopped) => {
            let cleanup = cleanup_destination(&staging);
            etude_cli_support::envelope::status("refused");
            eprintln!("unpack: stopped after SIGTERM. {}", cleanup.message());
            return refusal(ReasonCode::ExtractionStopped, input_subject(archive));
        }
        Err(ExtractError::ToolFailed(output)) => {
            let cleanup = cleanup_destination(&staging);
            if output.disk_full {
                eprintln!(
                    "unpack: the disk ran out of room while extracting {}. {} \
                     Free some space, or choose another disk with --into DIR.",
                    shown(archive),
                    cleanup.message()
                );
                output.print("Detail from the system tool");
                return failure(
                    ReasonCode::InsufficientSpace,
                    Some(Subject::wait("destination", None)),
                );
            }
            eprintln!(
                "unpack: {} could not be fully extracted. It may be damaged, incomplete, or not \
                 {}. {} If you downloaded it, try downloading it again.",
                shown(archive),
                format_name(fmt),
                cleanup.message()
            );
            output.print("Detail from the system tool");
            return failure(ReasonCode::ArchiveUnreadable, input_subject(archive));
        }
        Err(ExtractError::Other(e)) => {
            let cleanup = cleanup_destination(&staging);
            eprintln!(
                "unpack: could not finish extracting {}: {}. {}",
                shown(archive),
                plain_text(&e),
                cleanup.message()
            );
            return failure(ReasonCode::InternalError, None);
        }
        Ok(Err(breach)) => {
            // Remove what landed before the soft limit was observed. A partial extraction
            // left behind is the mess this tool exists to avoid, and it is
            // worse here than usual: the user did not choose to start it.
            let cleanup = cleanup_destination(&staging);
            let Breach::Total(n) = breach;
            // The limit was not the problem if the disk could not have held
            // that much anyway: a larger limit would not help.
            if free.is_some_and(|f| n >= f) {
                eprintln!(
                    "unpack: the disk ran out of room while extracting {}. {} \
                     Free some space, or choose another disk with --into DIR.",
                    shown(archive),
                    cleanup.message()
                );
                return failure(
                    ReasonCode::InsufficientSpace,
                    Some(Subject::wait("destination", None)),
                );
            }
            let into = value(args, "--into").map(|d| PathBuf::from(expand_tilde(&d)));
            eprintln!(
                "{}",
                limit_message(
                    n,
                    budget,
                    chosen_limit.is_some(),
                    &cleanup.message(),
                    archive,
                    into.as_deref(),
                )
            );
            return refusal(ReasonCode::SizeLimitExceeded, input_subject(archive));
        }
        Ok(Ok(())) => {}
    }

    // Audit before tidying, so discarded metadata cannot hide unsafe members.
    if let Err(error) = audit_staging(&staging) {
        return audit_refusal(&staging, &dest, archive, &error);
    }

    // --- 5. tidy -----------------------------------------------------------
    let removed = remove_junk(&staging);
    let flattened = match safety::wrapper_dir(&entries) {
        Some(w) => flatten(&staging, &w),
        None => false,
    };
    // Preserve the captured source state after flattening and junk removal, before publication.
    let quarantined = match quarantine::preserve(&staging, pinned.quarantine.as_deref()) {
        Ok(count) => count,
        Err(error) => {
            etude_cli_support::envelope::status("refused");
            eprintln!(
                "{}",
                quarantine_failed_message(
                    &error,
                    &dest,
                    &cleanup_destination(&staging).message(),
                    &by_hand(fmt, archive, &dest),
                )
            );
            return if permission_denied(&error) {
                refusal(ReasonCode::PermissionDenied, input_subject(archive))
            } else {
                refusal(ReasonCode::InternalError, None)
            };
        }
    };
    etude_cli_support::envelope::effect("quarantine_paths_verified", quarantined);
    // Audit the final tree as well: this is the tree the rename publishes.
    let audited = match audit_staging(&staging) {
        Ok(count) => count,
        Err(error) => return audit_refusal(&staging, &dest, archive, &error),
    };
    if let Err(error) = publish_staging(&staging, &dest) {
        let cleanup = cleanup_destination(&staging);
        etude_cli_support::envelope::status("refused");
        if dest.symlink_metadata().is_ok() {
            eprintln!(
                "unpack: {} appeared while unpack was working, so the extracted files were not \
                 put there. {} Pass --into DIR to choose another.",
                shown(&dest),
                cleanup.message()
            );
            return refusal(ReasonCode::DestinationExists, destination_subject(&dest));
        }
        eprintln!(
            "unpack: could not put the extracted files at {}: {error}. {} \
             Choose another place with --into DIR.",
            shown(&dest),
            cleanup.message()
        );
        return refusal(ReasonCode::InternalError, None);
    }

    etude_cli_support::envelope::effect("entries_published", entries.len() - junk);
    etude_cli_support::envelope::effect("paths_audited", audited);
    etude_cli_support::envelope::effect("metadata_files_removed", removed);
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
                ("paths_audited", j::num(audited)),
                (
                    "quarantine",
                    j::str(if pinned.quarantine.is_some() {
                        "preserved_and_readback_verified"
                    } else {
                        "absent_on_captured_archive"
                    })
                ),
            ])
        );
        return ExitCode::SUCCESS;
    }

    let kept = entries.len() - junk;
    println!("\n  Extracted to {}/", dest.display());
    println!("  {}", counted(kept, "entry", "entries"));
    if flattened {
        println!("  Removed a duplicate wrapper folder");
    }
    if removed > 0 {
        println!(
            "  Dropped {}",
            counted(removed, "metadata file", "metadata files")
        );
    }
    if kept == 0 {
        println!("  The archive held only system metadata, so the folder is empty.");
    }
    println!(
        "  Checked {} before writing anything",
        counted(members.len(), "path", "paths")
    );
    println!(
        "  Audited {} before moving anything into place",
        counted(audited, "extracted path", "extracted paths")
    );
    ExitCode::SUCCESS
}

/// Enumerate member paths without extracting.
/// One member as the listing describes it: its mode string, if the format
/// reports one, and its path.
pub struct Member {
    pub mode: Option<String>,
    pub path: String,
}

/// What the typed listing found.
struct TypedListing {
    members: Vec<Member>,
    /// How many ZIP members say they are encrypted.
    encrypted: usize,
}

/// Why a plain listing gave no names.
enum ListError {
    /// The tool ran and could not read this file.
    Unreadable,
    /// The tool could not be run at all, which says nothing about the file.
    Tool(String),
}

/// zipinfo's fifth column starts with `t` or `b`, in upper case when the member
/// is encrypted (zipinfo(1)). Columns come before the name, so a name cannot
/// imitate this. It only decides which refusal is printed: extraction of such a
/// member fails without its password anyway.
fn zip_row_is_encrypted(line: &str) -> bool {
    line.split_whitespace()
        .nth(4)
        .is_some_and(|flags| flags.starts_with(['T', 'B']))
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
fn list_members(anchor: &ArchiveAnchor, fmt: Format) -> Result<TypedListing, String> {
    if fmt == Format::Gz {
        return Ok(TypedListing {
            members: vec![Member {
                mode: None,
                path: stem(anchor.name()),
            }],
            encrypted: 0,
        });
    }
    let mut command = listing_command(fmt, anchor.path(), true)?;
    anchor.prepare(&mut command)?;
    let out = command.output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err("the typed listing failed".into());
    }
    let mut members = Vec::new();
    let mut encrypted = 0;
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Some(member) = member_from_typed_line(line) {
            if fmt == Format::Zip && zip_row_is_encrypted(line) {
                encrypted += 1;
            }
            members.push(member);
        }
    }
    Ok(TypedListing { members, encrypted })
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
    // The tools append a link target after the name. A regular filename may
    // itself contain ` -> ` or ` link to `, so only a link row gets that suffix
    // cut. A target that holds the words leaves a name that fails to match the
    // plain listing, which is refused too.
    let target = match first.as_bytes()[0] {
        b'l' => p.rfind(" -> "),
        b'h' => p.rfind(" link to "),
        _ => None,
    };
    if let Some(cut) = target {
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

fn list(anchor: &ArchiveAnchor, fmt: Format) -> Result<Vec<String>, ListError> {
    if fmt == Format::Gz {
        return Ok(vec![stem(anchor.name())]);
    }
    let mut command = listing_command(fmt, anchor.path(), false).map_err(ListError::Tool)?;
    anchor.prepare(&mut command).map_err(ListError::Tool)?;
    let out = command
        .output()
        .map_err(|e| ListError::Tool(e.to_string()))?;
    if !out.status.success() {
        return Err(ListError::Unreadable);
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim_end_matches('/').to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

/// `--max-size N[G|M]`: the caller's monitored soft limit, replacing the default.
///
/// Exists because the default WILL refuse legitimate work -- a 6 GB project is
/// indistinguishable from a 6 GB bomb from here. A refusal with no way past it
/// is a wall rather than a safety feature, so the way past it is named in the
/// refusal itself.
fn max_size_flag(args: &[String]) -> Result<Option<u64>, BadArgument> {
    let Some(i) = args.iter().position(|a| a == "--max-size") else {
        return Ok(None);
    };
    let Some(raw) = args.get(i + 1) else {
        return Err(BadArgument::Invalid(
            "--max-size needs a size, such as 500M or 2G.".into(),
        ));
    };
    let bad = || {
        BadArgument::Invalid(format!(
            "--max-size needs a whole number above 0, optionally with M or G, got \"{}\".",
            shown_text(raw)
        ))
    };
    let (digits, mult) = match raw.chars().last() {
        Some('G' | 'g') => (&raw[..raw.len() - 1], 1024 * 1024 * 1024),
        Some('M' | 'm') => (&raw[..raw.len() - 1], 1024 * 1024),
        _ => (raw.as_str(), 1),
    };
    let amount = digits.parse::<u64>().map_err(|_| bad())?;
    let bytes = amount.checked_mul(mult).ok_or_else(|| {
        BadArgument::Invalid(format!(
            "--max-size {} is too large to count.",
            shown_text(raw)
        ))
    })?;
    if bytes == 0 {
        return Err(bad());
    }
    Ok(Some(bytes))
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

/// An exact byte count, with the readable size beside any that needs one.
fn exact_bytes(n: u64) -> String {
    let exact = counted(n as usize, "byte", "bytes");
    if n >= 1024 {
        format!("{exact} ({})", human(n))
    } else {
        exact
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

enum Cleanup {
    Removed,
    Remains {
        path: PathBuf,
        error: std::io::Error,
    },
    Unverified {
        path: PathBuf,
        error: std::io::Error,
    },
}

impl Cleanup {
    fn message(&self) -> String {
        match self {
            Self::Removed => "Nothing was left behind.".into(),
            Self::Remains { path, error } => format!(
                "The partial extraction remains at {}: could not remove it ({}). \
                 To delete it: chmod -R u+w {quoted} && rm -rf {quoted}",
                shown(path),
                plain_io_error(error),
                quoted = quoted_path(path)
            ),
            Self::Unverified { path, error } => format!(
                "Could not remove the partial extraction at {} ({}), and could not check \
                 whether it is still there. Look for it in that folder.",
                shown(path),
                plain_io_error(error)
            ),
        }
    }
}

fn cleanup_destination(dest: &Path) -> Cleanup {
    cleanup_destination_with(dest, |path| std::fs::remove_dir_all(path))
}

fn cleanup_destination_with(
    dest: &Path,
    remove: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Cleanup {
    match remove(dest) {
        Ok(()) => Cleanup::Removed,
        // `remove_dir_all` may report a destination that has disappeared
        // between extraction and cleanup. That is still a checked absence,
        // not an unverified cleanup failure.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Cleanup::Removed,
        Err(error) => match dest.symlink_metadata() {
            Ok(_) => Cleanup::Remains {
                path: dest.to_path_buf(),
                error,
            },
            // Metadata also observed the path absent, so there is no partial
            // extraction left to describe even though removal reported an
            // unrelated error.
            Err(metadata_error) if metadata_error.kind() == std::io::ErrorKind::NotFound => {
                Cleanup::Removed
            }
            Err(_) => Cleanup::Unverified {
                path: dest.to_path_buf(),
                error,
            },
        },
    }
}

/// Total logical bytes observed under `dir`, without following links.
/// An unreadable entry makes the measurement fail, never silently smaller.
fn written(dir: &Path) -> Result<u64, String> {
    measure(dir, dir, false)
}

/// One walk of staging. While an extractor is running, a name can vanish
/// between being listed and being examined: macOS tar writes each file under a
/// temporary name and renames it into place. That is a member moving, seen again
/// on the next poll, and not an unreadable entry. With no writer running, every
/// failure stays a failure, so the final measurement is never silently smaller.
fn measure(dir: &Path, root: &Path, writer_running: bool) -> Result<u64, String> {
    let rd =
        match etude_core::scan::observe_read("staging_tree_enumeration", std::fs::read_dir(dir)) {
            Ok(rd) => rd,
            Err(error)
                if writer_running
                    && dir != root
                    && error.kind() == std::io::ErrorKind::NotFound =>
            {
                return Ok(0);
            }
            Err(error) => return Err(tree_error(dir, error)),
        };
    let mut total = 0_u64;
    for entry in rd {
        let path = entry.map_err(|e| tree_error(dir, e))?.path();
        let md = match path.symlink_metadata() {
            Ok(md) => md,
            Err(error) if writer_running && error.kind() == std::io::ErrorKind::NotFound => {
                continue;
            }
            Err(error) => return Err(tree_error(&path, error)),
        };
        let bytes = if md.is_dir() {
            measure(&path, root, writer_running)?
        } else {
            md.len()
        };
        total = total
            .checked_add(bytes)
            .ok_or("measured byte total overflowed")?;
    }
    Ok(total)
}

fn tree_error(path: &Path, error: std::io::Error) -> String {
    format!(
        "cannot inspect {}: {}",
        etude_core::redact::path(path),
        plain_io_error(&error)
    )
}

/// Inspect the actual tree, including implicit directories and junk, without
/// following links. Only directories and ordinary, singly-linked files may
/// be published. A failed read or containment check refuses the transaction.
fn audit_staging(staging: &Path) -> Result<usize, String> {
    let md = staging
        .symlink_metadata()
        .map_err(|e| tree_error(staging, e))?;
    if !md.is_dir() || md.file_type().is_symlink() {
        return Err("the working folder is not an ordinary folder".into());
    }
    let root = staging.canonicalize().map_err(|e| tree_error(staging, e))?;
    fn walk(dir: &Path, root: &Path) -> Result<usize, String> {
        let mut count = 0;
        for entry in
            etude_core::scan::observe_read("staging_tree_enumeration", std::fs::read_dir(dir))
                .map_err(|e| tree_error(dir, e))?
        {
            let path = entry.map_err(|e| tree_error(dir, e))?.path();
            let md = path.symlink_metadata().map_err(|e| tree_error(&path, e))?;
            let kind = md.file_type();
            let reason = if kind.is_symlink() {
                Some("symlink")
            } else if !kind.is_file() && !kind.is_dir() {
                Some("special member")
            } else {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    if md.mode() & 0o6000 != 0 {
                        Some("setuid/setgid member")
                    } else if kind.is_file() && md.nlink() != 1 {
                        Some("hard-linked file")
                    } else {
                        None
                    }
                }
                #[cfg(not(unix))]
                {
                    None
                }
            };
            if let Some(reason) = reason {
                return Err(format!("{reason} at {}", etude_core::redact::path(&path)));
            }
            let resolved = path.canonicalize().map_err(|e| tree_error(&path, e))?;
            if !resolved.starts_with(root) {
                return Err(format!(
                    "a path left the working folder: {}",
                    etude_core::redact::path(&path)
                ));
            }
            count += 1;
            if kind.is_dir() {
                count += walk(&path, root)?;
            }
        }
        Ok(count)
    }
    // Keep the caller's spelling in diagnostics (e.g. /var vs /private/var),
    // while comparing resolved paths against the canonical containment root.
    walk(staging, &root)
}

fn audit_refusal(staging: &Path, dest: &Path, archive: &Path, error: &str) -> ExitCode {
    eprintln!(
        "unpack: the extracted files failed a safety check ({error}). Nothing was placed at {}. {}",
        shown(dest),
        cleanup_destination(staging).message()
    );
    refusal(ReasonCode::UnsafeInput, input_subject(archive))
}

/// Final measured total on a monitored soft-limit refusal.
pub enum Breach {
    Total(u64),
}

/// System extractors write directly. Poll every 100 ms and after exit; this
/// monitors a soft limit, with no fixed maximum overshoot. After killing and
/// reaping the extractor, recount to include writes made while stopping.
/// How many of the system tool's lines a person is shown. Member names reach
/// these lines, and a hostile archive can make any number of them.
const TOOL_LINES_SHOWN: usize = 5;
/// The longest such line shown, in characters.
const TOOL_LINE_CHARS: usize = 300;

/// What the system tool said, held back so it is printed under a label after
/// unpack's own verdict, with private paths and control characters taken out.
#[derive(Debug, Default)]
struct ToolOutput {
    lines: Vec<String>,
    hidden: usize,
    disk_full: bool,
}

impl ToolOutput {
    fn add(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        // tar and gunzip print strerror; unzip prints its own words.
        if line.contains("No space left") || line.contains("disk full") {
            self.disk_full = true;
        }
        if self.lines.len() < TOOL_LINES_SHOWN {
            self.lines
                .push(shown_text(line).chars().take(TOOL_LINE_CHARS).collect());
        } else {
            self.hidden += 1;
        }
    }

    fn print(&self, label: &str) {
        for line in &self.lines {
            eprintln!("{label}: {line}");
        }
        if self.hidden > 0 {
            eprintln!(
                "{label}: and {} more.",
                counted(self.hidden, "line", "lines")
            );
        }
    }
}

/// Why extraction did not finish.
#[derive(Debug)]
enum ExtractError {
    /// SIGTERM arrived.
    Stopped,
    /// The system tool ran and reported failure, usually over this archive.
    ToolFailed(ToolOutput),
    /// Something else stopped the run, which says nothing about the archive.
    Other(String),
}

impl From<String> for ExtractError {
    fn from(message: String) -> Self {
        Self::Other(message)
    }
}

/// Read one extractor stream, naming the archive as the user knows it and the
/// private folder not at all. The extractor only knows `/dev/fd/N` and that
/// folder, and neither must reach the user.
fn collect_output(
    mut reader: impl BufRead + Send + 'static,
    output: Arc<Mutex<ToolOutput>>,
    anchor: &ArchiveAnchor,
    private_folder: &Path,
) -> std::thread::JoinHandle<()> {
    let path = anchor.path().as_os_str().as_encoded_bytes().to_vec();
    let name = anchor.name().as_os_str().as_encoded_bytes().to_vec();
    // unzip prints the resolved path of a folder it was given, so remove both
    // spellings, the longer one first.
    let resolved = private_folder.canonicalize().ok();
    let mut folders = vec![private_folder.as_os_str().as_encoded_bytes().to_vec()];
    if let Some(resolved) = resolved {
        folders.insert(0, resolved.as_os_str().as_encoded_bytes().to_vec());
    }
    std::thread::spawn(move || {
        let mut line = Vec::new();
        loop {
            line.clear();
            // Member names reach these lines, so bound what one line can hold.
            match reader.by_ref().take(64 * 1024).read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {
                    // Keep draining: the extractor must not block on a full pipe.
                    let named = replace_bytes(&line, &path, &name);
                    let clean = folders.iter().fold(named, |text, folder| {
                        let mut inside = folder.clone();
                        inside.push(b'/');
                        let text = replace_bytes(&text, &inside, b"");
                        replace_bytes(&text, folder, b"the new folder")
                    });
                    if let Ok(mut output) = output.lock() {
                        output.add(&String::from_utf8_lossy(&clean));
                    }
                }
            }
        }
    })
}

fn replace_bytes(line: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(line.len());
    let mut rest = line;
    while !rest.is_empty() {
        if rest.starts_with(from) {
            out.extend_from_slice(to);
            rest = &rest[from.len()..];
        } else {
            out.push(rest[0]);
            rest = &rest[1..];
        }
    }
    out
}

fn run_bounded(
    mut cmd: Command,
    anchor: &ArchiveAnchor,
    dest: &Path,
    budget: u64,
) -> Result<Result<(), Breach>, ExtractError> {
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let output = Arc::new(Mutex::new(ToolOutput::default()));
    let mut readers = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        readers.push(collect_output(
            std::io::BufReader::new(stdout),
            output.clone(),
            anchor,
            dest,
        ));
    }
    if let Some(stderr) = child.stderr.take() {
        readers.push(collect_output(
            std::io::BufReader::new(stderr),
            output.clone(),
            anchor,
            dest,
        ));
    }
    let result = (|| -> Result<Result<(), Breach>, ExtractError> {
        loop {
            #[cfg(unix)]
            if SIGTERM_REQUESTED.swap(false, AtomicOrdering::Relaxed) {
                if child.try_wait().map_err(|e| e.to_string())?.is_none() {
                    child
                        .kill()
                        .map_err(|e| format!("could not stop extractor after SIGTERM: {e}"))?;
                }
                child
                    .wait()
                    .map_err(|e| format!("could not reap extractor after SIGTERM: {e}"))?;
                return Err(ExtractError::Stopped);
            }
            let status = child.try_wait().map_err(|e| e.to_string())?;
            let total = measure(dest, dest, status.is_none())?;
            if total > budget {
                if status.is_none() {
                    child
                        .kill()
                        .map_err(|e| format!("could not stop extractor: {e}"))?;
                    child
                        .wait()
                        .map_err(|e| format!("could not reap extractor: {e}"))?;
                }
                return Ok(Err(Breach::Total(written(dest)?)));
            }
            if let Some(status) = status {
                return if status.success() {
                    Ok(Ok(()))
                } else {
                    Err(ExtractError::ToolFailed(ToolOutput::default()))
                };
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    })();
    if result.is_err() {
        // A failed measurement must not orphan a writer while cleanup runs.
        let _ = child.kill();
        let _ = child.wait();
    }
    // A grandchild can hold a pipe open after the extractor exits, so this wait
    // has a limit.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while readers.iter().any(|reader| !reader.is_finished()) && std::time::Instant::now() < deadline
    {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let said = output
        .lock()
        .map(|mut output| std::mem::take(&mut *output))
        .unwrap_or_default();
    match result {
        Err(ExtractError::ToolFailed(_)) => Err(ExtractError::ToolFailed(said)),
        Ok(Ok(())) => {
            // A warning on a run that worked is still worth seeing.
            said.print("Note from the system tool");
            Ok(Ok(()))
        }
        other => other,
    }
}

fn extract(
    anchor: &ArchiveAnchor,
    fmt: Format,
    dest: &Path,
    budget: u64,
) -> Result<Result<(), Breach>, ExtractError> {
    let mut cmd = extractor_command(fmt, anchor.path(), dest);
    anchor.prepare(&mut cmd)?;
    cmd.stderr(Stdio::piped());
    if fmt == Format::Gz {
        let out_path = dest.join(stem(anchor.name()));
        let f = std::fs::File::create(&out_path).map_err(|e| e.to_string())?;
        cmd.stdout(f);
    } else {
        cmd.stdout(Stdio::piped());
    }
    run_bounded(cmd, anchor, dest, budget)
}

fn publish_staging(staging: &Path, dest: &Path) -> Result<(), String> {
    publish_staging_with(staging, dest, rename_no_replace, || {})
}

fn publish_staging_with(
    staging: &Path,
    dest: &Path,
    exclusive: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
    after_claim: impl FnOnce(),
) -> Result<(), String> {
    match exclusive(staging, dest) {
        Ok(()) => return Ok(()),
        Err(error)
            if error.kind() == std::io::ErrorKind::Unsupported
                || (cfg!(target_os = "macos") && error.raw_os_error() == Some(45)) => {}
        Err(error) => {
            return Err(format!(
                "{}; {} was not replaced",
                plain_io_error(&error),
                shown(dest)
            ));
        }
    }
    // exFAT cannot perform an exclusive rename. mkdir is its atomic claim:
    // an existing folder, even an empty one, must never be replaced.
    std::fs::create_dir(dest).map_err(|error| {
        format!(
            "could not make the folder ({}); {} was not replaced",
            plain_io_error(&error),
            shown(dest)
        )
    })?;
    after_claim();
    // If someone added a file after the claim, rename refuses the nonempty
    // folder. Never clean this folder up: its contents belong to that writer.
    std::fs::rename(staging, dest).map_err(|error| {
        format!(
            "{}; the claimed destination remains at {} and any files added there were left intact",
            plain_io_error(&error),
            shown(dest)
        )
    })
}

#[cfg(target_os = "macos")]
fn rename_no_replace(staging: &Path, dest: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let from = std::ffi::CString::new(staging.as_os_str().as_bytes())?;
    let to = std::ffi::CString::new(dest.as_os_str().as_bytes())?;
    unsafe extern "C" {
        fn renamex_np(
            from: *const std::ffi::c_char,
            to: *const std::ffi::c_char,
            flags: u32,
        ) -> i32;
    }
    // SAFETY: both pointers are live NUL-terminated paths. RENAME_EXCL (4)
    // tells macOS to refuse every existing destination atomically.
    if unsafe { renamex_np(from.as_ptr(), to.as_ptr(), 4) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "macos"))]
fn rename_no_replace(_staging: &Path, _dest: &Path) -> std::io::Result<()> {
    Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
}

/// Reserve a private sibling of `dest` for an in-progress extraction.
///
/// The directory is created, rather than merely named, before the extractor
/// starts. That makes uniqueness an OS-enforced property and gives a crash a
/// harmless place to strand an orphaned system extractor.
fn staging_destination(dest: &Path) -> std::io::Result<PathBuf> {
    let parent = dest.parent().unwrap_or(Path::new("."));
    let name = dest.file_name().unwrap_or_default().to_string_lossy();
    for _ in 0..128 {
        let mut nonce = [0_u8; 16];
        etude_core::scan::observe_read(
            "os_random_bytes",
            std::fs::File::open("/dev/urandom")
                .and_then(|mut random| random.read_exact(&mut nonce)),
        )?;
        let candidate = parent.join(format!(
            ".{name}.unpack-{}.partial",
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
        match builder.create(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::other(
        "could not make a unique private folder",
    ))
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
        let Ok(rd) =
            etude_core::scan::observe_read("staging_tree_enumeration", std::fs::read_dir(dir))
        else {
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
    let Ok(rd) =
        etude_core::scan::observe_read("staging_tree_enumeration", std::fs::read_dir(&inner))
    else {
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
    fn a_missing_root_fails_whether_or_not_a_writer_is_running() {
        let missing = std::env::temp_dir().join(format!("unpack-absent-{}", std::process::id()));
        assert!(measure(&missing, &missing, true).is_err());
        assert!(measure(&missing, &missing, false).is_err());
    }

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
    fn archive_anchor_has_no_name_on_disk_and_stays_read_only() {
        let root = std::env::temp_dir().join(format!("unpack-anchor-name-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).expect("create test directory");
        let archive = root.join("input.tar.gz");
        std::fs::write(&archive, b"checked bytes").expect("write archive");

        let anchor = ArchiveAnchor::create(&archive).expect("copy archive");
        assert_eq!(anchor.file.metadata().unwrap().nlink(), 0);
        assert_eq!(anchor.file.metadata().unwrap().mode() & 0o777, 0o600);
        assert_eq!(anchor.name(), Path::new("input.tar.gz"));
        assert!(std::io::Write::write_all(&mut &anchor.file, b"x").is_err());
        drop(anchor);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn only_a_prepared_command_inherits_the_copy_and_each_starts_at_its_beginning() {
        let root = std::env::temp_dir().join(format!("unpack-anchor-fd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).expect("create test directory");
        let archive = root.join("input.zip");
        std::fs::write(&archive, b"checked bytes").expect("write archive");
        let anchor = ArchiveAnchor::create(&archive).expect("copy archive");

        let cat = || {
            let mut command = Command::new("/bin/cat");
            command.arg(anchor.path()).stderr(Stdio::null());
            command
        };
        assert!(!cat().output().unwrap().status.success());
        for _ in 0..2 {
            let mut command = cat();
            anchor.prepare(&mut command).unwrap();
            assert_eq!(command.output().unwrap().stdout, b"checked bytes");
        }
        assert!(!cat().output().unwrap().status.success());
        drop(anchor);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_refused_descriptor_path_keeps_a_private_name_until_drop() {
        let root = std::env::temp_dir().join(format!("unpack-anchor-kept-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).expect("create test directory");
        let archive = root.join("input.zip");
        std::fs::write(&archive, b"checked bytes").expect("write archive");

        DESCRIPTOR_PATH_REFUSED.with(|refused| refused.set(true));
        let anchor = ArchiveAnchor::create(&archive);
        DESCRIPTOR_PATH_REFUSED.with(|refused| refused.set(false));
        let anchor = anchor.expect("copy archive");

        let kept = anchor.path().to_path_buf();
        assert_eq!(anchor.kept.as_ref(), Some(&kept));
        assert_eq!(std::fs::read(&kept).unwrap(), b"checked bytes");
        assert_eq!(std::fs::metadata(&kept).unwrap().mode() & 0o777, 0o600);
        drop(anchor);
        assert!(!kept.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn replace_bytes_replaces_every_copy_of_the_descriptor_path() {
        assert_eq!(
            replace_bytes(b"gzip: /dev/fd/5: bad /dev/fd/5\n", b"/dev/fd/5", b"a.gz"),
            b"gzip: a.gz: bad a.gz\n"
        );
        assert_eq!(
            replace_bytes(b"/dev/fd/\n", b"/dev/fd/5", b"a"),
            b"/dev/fd/\n"
        );
    }

    #[test]
    fn preflight_fault_is_confined_to_its_installing_thread() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        AFTER_PREFLIGHT.with(|hook| {
            *hook.borrow_mut() = Some(Box::new({
                let calls = calls.clone();
                move || {
                    calls.fetch_add(1, Ordering::SeqCst);
                }
            }));
        });
        std::thread::spawn(after_preflight_for_test)
            .join()
            .expect("parallel preflight completes");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "another thread stole the fault"
        );
        after_preflight_for_test();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "installing thread did not inject its fault"
        );
        after_preflight_for_test();
        assert_eq!(calls.load(Ordering::SeqCst), 1, "fault ran more than once");
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

        #[cfg(target_os = "macos")]
        {
            quarantine::platform_set_for_test(&archive, b"0083;00000000;etudes-original").unwrap();
            quarantine::platform_set_for_test(&replacement, b"0083;00000000;etudes-replacement")
                .unwrap();
        }
        AFTER_PREFLIGHT.with(|hook| {
            *hook.borrow_mut() = Some(Box::new({
                let archive = archive.clone();
                let replacement = replacement.clone();
                move || {
                    std::fs::rename(replacement, archive).expect("replace archive after listings")
                }
            }));
        });
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
        #[cfg(target_os = "macos")]
        {
            let output = quarantine::open_archive(&dest.join("checked.txt")).unwrap();
            assert_eq!(
                quarantine::capture(&output).unwrap().as_deref(),
                Some(b"0083;00000000;etudes-original".as_slice())
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn changing_quarantine_during_anchor_copy_is_refused() {
        let root =
            std::env::temp_dir().join(format!("unpack-quarantine-change-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let archive = root.join("source.gz");
        std::fs::write(&archive, b"synthetic opaque bytes").unwrap();
        quarantine::platform_set_for_test(&archive, b"0083;00000000;etudes-before").unwrap();
        AFTER_ANCHOR_COPY.with(|hook| {
            *hook.borrow_mut() = Some(Box::new({
                let archive = archive.clone();
                move || {
                    quarantine::platform_set_for_test(&archive, b"0083;00000000;etudes-after")
                        .unwrap()
                }
            }))
        });
        assert!(matches!(
            ArchiveAnchor::create(&archive),
            Err(ArchiveAnchorError::QuarantineChanged)
        ));
        assert!(archive.is_file());
        std::fs::remove_dir_all(root).unwrap();
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

    struct AuditFixture(PathBuf);

    impl AuditFixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "unpack-audit-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&root).expect("create audit fixture");
            Self(root)
        }
    }

    impl Drop for AuditFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn publication_never_replaces_a_late_empty_folder() {
        assert_late_folder_preserved(false);
    }

    #[test]
    fn publication_never_replaces_a_late_nonempty_folder() {
        assert_late_folder_preserved(true);
    }

    fn assert_late_folder_preserved(nonempty: bool) {
        use std::os::unix::fs::MetadataExt;
        let fixture = AuditFixture::new();
        let dest = fixture.0.join("out");
        let staging = staging_destination(&dest).unwrap();
        std::fs::write(staging.join("archive-file"), b"archive").unwrap();
        std::fs::create_dir(&dest).unwrap();
        if nonempty {
            std::fs::write(dest.join("their-file"), b"untouched").unwrap();
        }
        let inode = dest.metadata().unwrap().ino();
        assert!(publish_staging(&staging, &dest).is_err());
        assert_eq!(dest.metadata().unwrap().ino(), inode);
        assert!(!dest.join("archive-file").exists());
        assert!(staging.join("archive-file").exists());
        if nonempty {
            assert_eq!(
                std::fs::read(dest.join("their-file")).unwrap(),
                b"untouched"
            );
        }
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn enotsup_exclusive_rename_claims_and_publishes() {
        let fixture = AuditFixture::new();
        let dest = fixture.0.join("out");
        let staging = staging_destination(&dest).unwrap();
        std::fs::write(staging.join("archive-file"), b"archive").unwrap();
        publish_staging_with(
            &staging,
            &dest,
            |_, _| Err(std::io::Error::from_raw_os_error(45)),
            || assert!(dest.is_dir()),
        )
        .unwrap();
        assert_eq!(
            std::fs::read(dest.join("archive-file")).unwrap(),
            b"archive"
        );
        assert!(!staging.exists());
    }

    #[test]
    fn unsupported_exclusive_rename_refuses_existing_folders() {
        for nonempty in [false, true] {
            let fixture = AuditFixture::new();
            let dest = fixture.0.join("out");
            let staging = staging_destination(&dest).unwrap();
            std::fs::create_dir(&dest).unwrap();
            if nonempty {
                std::fs::write(dest.join("their-file"), b"untouched").unwrap();
            }
            let error = publish_staging_with(
                &staging,
                &dest,
                |_, _| Err(std::io::Error::from(std::io::ErrorKind::Unsupported)),
                || panic!("existing destination must not be claimed"),
            )
            .unwrap_err();
            assert!(error.contains("was not replaced"));
            assert!(dest.is_dir());
            assert!(staging.is_dir());
            if nonempty {
                assert_eq!(
                    std::fs::read(dest.join("their-file")).unwrap(),
                    b"untouched"
                );
            }
        }
    }

    #[test]
    fn unsupported_exclusive_rename_preserves_files_added_after_claim() {
        let fixture = AuditFixture::new();
        let dest = fixture.0.join("out");
        let staging = staging_destination(&dest).unwrap();
        std::fs::write(staging.join("archive-file"), b"archive").unwrap();
        let error = publish_staging_with(
            &staging,
            &dest,
            |_, _| Err(std::io::Error::from(std::io::ErrorKind::Unsupported)),
            || std::fs::write(dest.join("their-file"), b"untouched").unwrap(),
        )
        .unwrap_err();
        assert!(error.contains("claimed destination remains"));
        assert!(error.contains(&shown(&dest)));
        assert_eq!(
            std::fs::read(dest.join("their-file")).unwrap(),
            b"untouched"
        );
        assert!(!dest.join("archive-file").exists());
        assert!(staging.join("archive-file").exists());
    }

    #[test]
    fn audit_counts_implicit_directories_and_files_before_and_after_tidying() {
        let fixture = AuditFixture::new();
        let staging = staging_destination(&fixture.0.join("out")).unwrap();
        std::fs::create_dir_all(staging.join("wrapper/nested")).unwrap();
        std::fs::write(staging.join("wrapper/nested/file"), b"data").unwrap();
        std::fs::write(staging.join("wrapper/.DS_Store"), b"junk").unwrap();
        assert_eq!(audit_staging(&staging).unwrap(), 4);
        assert_eq!(remove_junk(&staging), 1);
        assert!(flatten(&staging, "wrapper"));
        assert_eq!(audit_staging(&staging).unwrap(), 2);
        assert_eq!(written(&staging).unwrap(), 4);
    }

    #[cfg(unix)]
    #[test]
    fn audit_refuses_external_dangling_and_internal_symlinks_without_following_them() {
        use std::os::unix::fs::symlink;
        let fixture = AuditFixture::new();
        let staging = staging_destination(&fixture.0.join("out")).unwrap();
        let outside = fixture.0.join("outside");
        std::fs::write(&outside, b"untouched").unwrap();
        std::fs::write(staging.join("file"), b"data").unwrap();
        for target in [outside, fixture.0.join("missing"), staging.join("file")] {
            let link = staging.join(".DS_Store");
            symlink(target, &link).unwrap();
            assert!(audit_staging(&staging).unwrap_err().contains("symlink"));
            std::fs::remove_file(link).unwrap();
        }
        assert_eq!(
            std::fs::read(fixture.0.join("outside")).unwrap(),
            b"untouched"
        );
        let alias = fixture.0.join("alias");
        symlink(&staging, &alias).unwrap();
        assert!(
            audit_staging(&alias)
                .unwrap_err()
                .contains("working folder")
        );
    }

    #[cfg(unix)]
    #[test]
    fn audit_refuses_hard_links_privileged_modes_and_fifos() {
        use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
        let fixture = AuditFixture::new();
        let staging = staging_destination(&fixture.0.join("out")).unwrap();
        let file = staging.join("file");
        std::fs::write(&file, b"data").unwrap();
        let link = staging.join("link");
        std::fs::hard_link(&file, &link).unwrap();
        assert_eq!(file.symlink_metadata().unwrap().nlink(), 2);
        assert!(audit_staging(&staging).unwrap_err().contains("hard-linked"));
        std::fs::remove_file(link).unwrap();
        assert_eq!(file.symlink_metadata().unwrap().nlink(), 1);

        // chmod can succeed while the filesystem clears the requested bit
        // (e.g. setgid for a group the caller does not belong to). Verify the
        // fixture before asking the audit to refuse it; try setuid as well.
        let mut privileged_fixture_created = false;
        for requested in [0o2644, 0o4644] {
            if let Err(error) =
                std::fs::set_permissions(&file, std::fs::Permissions::from_mode(requested))
            {
                eprintln!("privileged-mode fixture: chmod {requested:#06o} failed: {error}");
            }
            let observed = file.symlink_metadata().unwrap().mode() & 0o7777;
            if observed & 0o6000 != 0 {
                assert!(
                    audit_staging(&staging)
                        .expect_err("audit must refuse the privileged bits verified by readback")
                        .contains("setuid/setgid")
                );
                privileged_fixture_created = true;
            } else {
                eprintln!(
                    "privileged-mode fixture unavailable: requested {requested:#06o}, \
                     read back {observed:#06o}; platform did not retain privileged bits"
                );
                assert_eq!(
                    audit_staging(&staging).unwrap(),
                    1,
                    "without privileged bits, this singly-linked file is ordinary"
                );
            }
        }
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(file.symlink_metadata().unwrap().mode() & 0o6000, 0);
        if !privileged_fixture_created {
            eprintln!(
                "privileged-mode audit case was not exercised on this platform; \
                 requiring the verified FIFO audit refusal below instead"
            );
        }
        let fifo = staging.join("fifo");
        assert!(
            Command::new("/usr/bin/mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        assert!(fifo.symlink_metadata().unwrap().file_type().is_fifo());
        assert!(
            audit_staging(&staging)
                .unwrap_err()
                .contains("special member")
        );
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_subtrees_fail_both_audit_and_byte_measurement() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = AuditFixture::new();
        let nested = fixture.0.join("nested");
        std::fs::create_dir(&nested).unwrap();
        std::fs::write(nested.join("file"), b"data").unwrap();
        std::fs::set_permissions(&nested, std::fs::Permissions::from_mode(0o000)).unwrap();
        let audit = audit_staging(&fixture.0);
        let measurement = written(&fixture.0);
        std::fs::set_permissions(&nested, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(audit.unwrap_err().contains("cannot inspect"));
        assert!(measurement.unwrap_err().contains("cannot inspect"));
        assert!(written(&fixture.0.join("missing")).is_err());
    }

    #[test]
    fn a_cleanup_failure_names_the_partial_extraction_and_its_location() {
        let dest =
            std::env::temp_dir().join(format!("unpack-cleanup-failure-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dest);
        // A file is deliberately not a valid extraction directory, so the
        // production `remove_dir_all` call genuinely fails and the following
        // metadata check can establish that something remains at this path.
        std::fs::write(&dest, "partial extraction").expect("create partial extraction");

        let cleanup = cleanup_destination(&dest);
        let message = cleanup.message();

        assert!(
            message.contains("partial extraction remains"),
            "cleanup failure claimed more than it knew: {message}"
        );
        assert!(
            !message.contains("Nothing was left behind"),
            "cleanup failure still claimed the partial extraction was gone: {message}"
        );
        assert!(
            message.contains(&shown(&dest)),
            "cleanup failure did not name where the partial extraction remains: {message}"
        );
        assert!(
            message.contains("could not remove it (not a directory)"),
            "cleanup failure discarded the operating-system reason: {message}"
        );

        std::fs::remove_file(&dest).expect("remove test file");
    }

    #[test]
    fn an_empty_zip_is_only_its_end_record() {
        let mut empty = b"PK\x05\x06".to_vec();
        empty.extend([0; 18]);
        assert!(is_empty_zip(&empty));
        // A comment the record declares is still an empty archive.
        let mut commented = empty.clone();
        commented[20] = 2;
        commented.extend(b"hi");
        assert!(is_empty_zip(&commented));
        // Damage is not emptiness: wrong length, entries counted, junk around.
        let mut counted = empty.clone();
        counted[8] = 1;
        assert!(!is_empty_zip(&counted));
        let mut cut = empty.clone();
        cut[20] = 5;
        assert!(!is_empty_zip(&cut));
        let mut trailing = empty.clone();
        trailing.push(0);
        assert!(!is_empty_zip(&trailing));
        let mut leading = vec![0];
        leading.extend(&empty);
        assert!(!is_empty_zip(&leading));
        assert!(!is_empty_zip(&empty[..21]));
        assert!(!is_empty_zip(b""));
    }

    #[test]
    fn only_a_zip_row_with_an_upper_case_flag_is_encrypted() {
        assert!(zip_row_is_encrypted(
            "-rw-r--r--  3.0 unx        6 TX stor 26-Nov-04 20:12 p.txt"
        ));
        assert!(zip_row_is_encrypted(
            "-rw-r--r--  3.0 unx        6 Bl defN 26-Nov-04 20:12 p.txt"
        ));
        assert!(!zip_row_is_encrypted(
            "-rw-r--r--  3.0 unx        6 tx stor 26-Nov-04 20:12 Tx Tx Tx"
        ));
    }

    #[test]
    fn a_hard_link_row_names_the_link_and_is_judged_by_kind() {
        let member =
            member_from_typed_line("hrw-r--r--  0 0      0           0 Dec 31  1969 hl link to f")
                .unwrap();
        assert_eq!(member.path, "hl");
        assert!(matches!(
            safety::judge_mode(member.mode.as_deref().unwrap(), &member.path),
            Some(Unsafe::Hardlink(_))
        ));
        // A plain file may hold the same words in its name.
        let plain =
            member_from_typed_line("-rw-r--r--  0 0      0           0 Dec 31  1969 a link to b")
                .unwrap();
        assert_eq!(plain.path, "a link to b");
    }

    #[test]
    fn options_may_come_before_the_archive_and_no_value_is_taken_for_it() {
        let args = |words: &[&str]| words.iter().map(|w| w.to_string()).collect::<Vec<_>>();
        for (given, expected) in [
            (vec!["--list", "a.zip"], vec!["a.zip", "--list"]),
            (
                vec!["--into", "o", "--json", "a.zip"],
                vec!["a.zip", "--into", "o", "--json"],
            ),
            (vec!["--into", "a.zip"], vec!["--into", "a.zip"]),
            (vec!["--frob", "a.zip"], vec!["--frob", "a.zip"]),
            (vec!["--json", "contract"], vec!["--json", "contract"]),
            (vec!["a.zip", "--list"], vec!["a.zip", "--list"]),
        ] {
            assert_eq!(archive_first(args(&given)), args(&expected));
        }
        // Every refusal survives the move.
        assert!(validate_args(&archive_first(args(&["--list", "a.zip", "b.zip"]))).is_err());
        assert!(validate_args(&archive_first(args(&["--into", "--json", "a.zip"]))).is_err());
        assert!(matches!(
            validate_args(&archive_first(args(&["--frob", "a.zip"]))),
            Err(BadArgument::UnknownOption(_))
        ));
        assert!(validate_args(&archive_first(args(&["--max-size", "0", "a.zip"]))).is_err());
        assert!(validate_args(&archive_first(args(&["a.zip", "--max-size", "0G"]))).is_err());
    }

    /// The default limit cannot be reached in a test without writing over a GB.
    #[test]
    fn each_size_limit_explains_only_itself() {
        let archive = Path::new("a b.zip");
        let chosen = limit_message(200_000, 10, true, "Nothing was left behind.", archive, None);
        assert!(chosen.contains("your --max-size limit"), "{chosen}");
        assert!(chosen.contains("200000 bytes (195.3 KB)"), "{chosen}");
        assert!(chosen.contains("the limit is 10 bytes"), "{chosen}");
        assert!(!chosen.contains("half the free space"), "{chosen}");
        assert!(
            chosen.contains("unpack 'a b.zip' --max-size SIZE"),
            "{chosen}"
        );
        let default = limit_message(
            2 << 30,
            1 << 30,
            false,
            "Nothing was left behind.",
            archive,
            Some(Path::new("o")),
        );
        assert!(default.contains("the default size limit"), "{default}");
        assert!(default.contains("half the free space"), "{default}");
        assert!(!default.contains("your --max-size"), "{default}");
        assert!(default.contains("--into 'o' --max-size SIZE"), "{default}");
    }

    #[test]
    fn refusals_read_only_from_code_say_what_to_do() {
        let (archive, dest) = (Path::new("a.zip"), Path::new("a"));
        let typeless = type_unknown_message(archive, Format::Zip, dest);
        assert!(typeless.contains("unzip -l 'a.zip'"), "{typeless}");
        assert!(typeless.contains("unzip 'a.zip' -d 'a'"), "{typeless}");
        assert!(
            !typeless.contains("--list"),
            "--list stops here too: {typeless}"
        );
        let tar = type_unknown_message(Path::new("a.tar"), Format::Tar, dest);
        assert!(tar.contains("mkdir 'a' && tar -xf 'a.tar' -C 'a'"), "{tar}");
        let changed = quarantine_changed_message(archive);
        assert!(changed.contains("Run unpack again."), "{changed}");
        let denied = quarantine_failed_message(
            &std::io::Error::from_raw_os_error(13),
            dest,
            "Nothing was left behind.",
            "unzip 'a.zip' -d 'a'",
        );
        assert!(denied.contains(": permission denied."), "{denied}");
        assert!(denied.contains("unzip 'a.zip' -d 'a'"), "{denied}");
        let other = quarantine_failed_message(
            &std::io::Error::from_raw_os_error(45),
            dest,
            "Nothing was left behind.",
            "unzip 'a.zip' -d 'a'",
        );
        assert!(other.contains("--into DIR"), "{other}");
        assert!(!other.contains("os error"), "{other}");
    }

    #[test]
    fn tool_output_is_bounded_and_notices_a_full_disk() {
        let mut output = ToolOutput::default();
        for number in 0..8 {
            output.add(&format!("line {number}\u{1b}[31m\n"));
        }
        assert_eq!(output.lines.len(), TOOL_LINES_SHOWN);
        assert_eq!(output.hidden, 3);
        assert!(output.lines.iter().all(|line| !line.contains('\u{1b}')));
        assert!(!output.disk_full);
        output.add("unzip: write error (disk full?)");
        assert!(output.disk_full, "counted even when past the lines shown");
        assert_eq!(
            plain_text("cannot inspect x: Denied (os error 13)"),
            "cannot inspect x: Denied"
        );
    }

    #[test]
    fn a_not_found_removal_error_means_nothing_remains() {
        let dest = Path::new("/a/path/that-is-already-gone");
        let cleanup = cleanup_destination_with(dest, |_| {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "already removed",
            ))
        });

        assert!(matches!(cleanup, Cleanup::Removed));
        assert_eq!(cleanup.message(), "Nothing was left behind.");
    }

    #[test]
    fn a_successful_cleanup_checks_that_the_destination_is_removed() {
        let dest =
            std::env::temp_dir().join(format!("unpack-cleanup-success-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dest);
        std::fs::create_dir_all(&dest).expect("create extraction directory");

        let cleanup = cleanup_destination(&dest);

        assert!(matches!(cleanup, Cleanup::Removed));
        assert!(
            !dest.exists(),
            "successful cleanup left its destination behind"
        );
    }
}
