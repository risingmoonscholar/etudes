use etude_cli_support::{
    contract, envelope,
    reason::{Code, Reason, Subject},
};
use etude_core::{json as j, scan};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const TAR: &str = "/usr/bin/tar";
const MAX_ENTRIES: usize = 20_000;
const DEFAULT_MAX: u64 = 4 * 1024 * 1024 * 1024;
static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn stop(_: i32) {
    STOP.store(true, Ordering::Relaxed);
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    envelope::begin("pack", VERSION, &args);
    unsafe extern "C" {
        fn signal(number: i32, handler: usize) -> usize;
    }
    // SAFETY: the handler only sets an atomic flag and lives for the process.
    unsafe {
        signal(15, stop as *const () as usize);
    }
    let result = std::panic::catch_unwind(|| run(&args));
    let code = match result {
        Ok(Ok(())) => ExitCode::SUCCESS,
        Ok(Err(error)) => error.report(),
        Err(_) => Problem::new(Code::InternalError, 3, "internal failure", None).report(),
    };
    envelope::finish(code);
    code
}

#[derive(Debug)]
struct Problem {
    code: Code,
    exit: u8,
    message: String,
    path: Option<PathBuf>,
}
type Result<T> = std::result::Result<T, Problem>;
impl Problem {
    fn new(code: Code, exit: u8, message: impl Into<String>, path: Option<&Path>) -> Self {
        Self {
            code,
            exit,
            message: message.into(),
            path: path.map(Path::to_path_buf),
        }
    }
    fn unsafe_input() -> Self {
        Self::new(
            Code::UnsafeInput,
            2,
            "source violates archive safety policy",
            None,
        )
    }
    fn report(self) -> ExitCode {
        if self.code == Code::NothingToDo {
            eprintln!("pack: nothing to include");
            return ExitCode::from(1);
        }
        let kind = match self.code {
            Code::MissingInput | Code::UnsafeInput | Code::PermissionDenied => Some("input"),
            Code::DestinationExists | Code::InsufficientSpace => Some("destination"),
            Code::DownloadInFlight => Some("download_in_flight"),
            Code::OperationIncomplete => Some("journal"),
            _ => None,
        };
        let subject = kind.map(|kind| match &self.path {
            Some(path) if self.code == Code::DownloadInFlight => Subject::path(
                kind,
                &path.file_name().unwrap_or_default().to_string_lossy(),
            ),
            Some(path) => Subject::path(kind, &etude_core::redact::path(path)),
            None => Subject::wait(kind, None),
        });
        envelope::reason(Reason::refusal(self.code, subject));
        eprintln!("pack: {}", self.message);
        if self.code == Code::DownloadInFlight
            && let Some(path) = &self.path
        {
            eprintln!(
                "pack: download in flight: {}",
                path.file_name().unwrap_or_default().to_string_lossy()
            );
        }
        ExitCode::from(self.exit)
    }
}
fn io_problem(error: io::Error) -> Problem {
    if error.kind() == io::ErrorKind::PermissionDenied {
        Problem::new(
            Code::PermissionDenied,
            2,
            "required access is unavailable",
            None,
        )
    } else {
        Problem::new(
            Code::InternalError,
            3,
            "filesystem or child process failed",
            None,
        )
    }
}
fn observed<T>(category: &'static str, value: io::Result<T>) -> Result<T> {
    scan::observe_read(category, value).map_err(io_problem)
}

#[derive(Debug, Clone)]
struct Policy {
    source: PathBuf,
    into: PathBuf,
    recipient: String,
    format: String,
    keep: bool,
    max: u64,
    list: bool,
}
const HELP: &str = "pack — create a new archive with an explicit disclosure policy

Usage: pack SOURCE --for self|mac|other [--into FILE] [--format tar.gz|zip]
            [--metadata keep|drop] [--max-size N[G|M]] [--list] [--json]
       pack contract [--json]
       pack help | --version

self/mac default to tar.gz and keep; other defaults to zip and drop.
zip requires drop. Sensitive names are omitted and counted, never printed.
Hidden files are included. drop omits .DS_Store and AppleDouble files and
discards xattrs, resource forks, ACLs and file flags. keep requires tar.gz.
Packages are included complete or refused. All symlinks, in-flight downloads,
unsafe member paths and extraction-name collisions are refused.
--list performs metadata preflight only and writes nothing.
Output uses private staging beside FILE and exclusive publication on macOS.
Existing destinations are never replaced. Unsupported volumes are refused.
The size limit is monitored, with possible overshoot confined to staging.
Source identity is checked before publication; this is not a filesystem lock.
No ownership, hard-link topology or application-level consistency guarantee.
Exit: 0 done, 1 nothing to include, 2 refused, 3 error or incomplete.";
fn run(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("contract") => {
            let code = contract::command(contract::Tool::Pack, VERSION, args, &[], None);
            if code != ExitCode::SUCCESS {
                return Err(Problem::new(
                    Code::InvalidArguments,
                    2,
                    "contract accepts only --json",
                    None,
                ));
            }
            return Ok(());
        }
        Some("help" | "--help" | "-h")
            if args.len() == 1 || args.get(1).is_some_and(|s| s == "--json") && args.len() == 2 =>
        {
            envelope::print(HELP.into());
            return Ok(());
        }
        Some("--version" | "version" | "-V")
            if args.len() == 1 || args.get(1).is_some_and(|s| s == "--json") && args.len() == 2 =>
        {
            envelope::print(format!("pack {VERSION}"));
            return Ok(());
        }
        _ => {}
    }
    let mut policy = parse(args)?;
    let root_meta = match fs::symlink_metadata(&policy.source) {
        Ok(value) => value,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(Problem::new(
                Code::MissingInput,
                3,
                "source is absent",
                Some(&policy.source),
            ));
        }
        Err(error) => return Err(io_problem(error)),
    };
    if !root_meta.is_dir() || root_meta.file_type().is_symlink() {
        return Err(Problem::unsafe_input());
    }
    policy.source = observed("ancestor_metadata", fs::canonicalize(&policy.source))?;
    let parent = policy
        .into
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = observed("destination_parent_metadata", fs::canonicalize(parent))?;
    policy.into = parent.join(policy.into.file_name().ok_or_else(Problem::unsafe_input)?);
    if policy.into.starts_with(&policy.source) {
        return Err(Problem::unsafe_input());
    }
    if fs::symlink_metadata(&policy.into).is_ok() {
        return Err(Problem::new(
            Code::DestinationExists,
            2,
            "destination is occupied",
            Some(&policy.into),
        ));
    }
    let plan = preflight(&policy)?;
    details(&policy, &plan);
    if plan.entries.is_empty() {
        envelope::status("nothing_to_do");
        envelope::reason(Reason::nothing_to_do());
        return Err(Problem::new(
            Code::NothingToDo,
            1,
            "nothing to include",
            None,
        ));
    }
    if policy.list {
        if !args.iter().any(|arg| arg == "--json") {
            envelope::print(format!(
                "Preflight: {} entries; {} exclusions; upper bound {} bytes.",
                plan.entries.len(),
                plan.excluded,
                plan.bound
            ));
        }
        return Ok(());
    }
    let available = free_bytes(&parent)?;
    if available < plan.bound.saturating_add(16 * 1024 * 1024) {
        return Err(Problem::new(
            Code::InsufficientSpace,
            2,
            "insufficient destination working space",
            Some(&policy.into),
        ));
    }
    let staging = Staging::create(&policy.into)?;
    let mut cmd = writer(&policy, &staging);
    let manifest = plan
        .entries
        .iter()
        .flat_map(|e| e.name.bytes().chain(std::iter::once(0)))
        .collect::<Vec<_>>();
    let mut child = cmd.spawn().map_err(io_problem)?;
    let write = child
        .stdin
        .take()
        .ok_or_else(|| io_problem(io::Error::other("missing pipe")))?
        .write_all(&manifest);
    if let Err(error) = write {
        let _ = child.kill();
        let _ = child.wait();
        return Err(io_problem(error));
    }
    let started = Instant::now();
    let written = (|| {
        loop {
            let status = child.try_wait().map_err(io_problem)?;
            let size = observed("staging_archive_metadata", fs::metadata(&staging.path))?.len();
            if size > policy.max || size > plan.bound {
                return Err(Problem::new(
                    Code::InsufficientSpace,
                    2,
                    "private archive exceeded output budget",
                    Some(&policy.into),
                ));
            }
            if STOP.load(Ordering::Relaxed) || started.elapsed() > Duration::from_secs(300) {
                return Err(Problem::new(
                    Code::InternalError,
                    3,
                    "archive writer stopped",
                    None,
                ));
            }
            if let Some(status) = status {
                return if status.success() {
                    Ok(())
                } else {
                    Err(Problem::new(
                        Code::UnsafeInput,
                        2,
                        "archive writer refused source",
                        None,
                    ))
                };
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    })();
    if written.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    written?;
    staging.file.sync_all().map_err(io_problem)?;
    readback(&staging.path, &plan, policy.keep)?;
    let current = preflight(&policy)?;
    if current != plan {
        return Err(Problem::unsafe_input());
    }
    staging.verify()?;
    rename_no_replace(&staging.path, &policy.into).map_err(|_| {
        Problem::new(
            Code::DestinationExists,
            2,
            "exclusive publication unavailable or destination occupied",
            Some(&policy.into),
        )
    })?;
    envelope::effect("archives_published", 1);
    verify_published(&policy.into, || {
        readback(&policy.into, &plan, policy.keep)?;
        let same = fs::symlink_metadata(&policy.into)
            .and_then(|m| {
                staging
                    .file
                    .metadata()
                    .map(|held| Identity::of(&m) == Identity::of(&held))
            })
            .map_err(io_problem)?;
        if !same {
            return Err(Problem::unsafe_input());
        }
        Ok(())
    })?;
    if !args.iter().any(|arg| arg == "--json") {
        envelope::print(format!(
            "Created archive: {} entries; {} exclusions.",
            plan.entries.len(),
            plan.excluded
        ));
    }
    Ok(())
}
fn verify_published(path: &Path, verify: impl FnOnce() -> Result<()>) -> Result<()> {
    if verify().is_err() {
        envelope::status("incomplete");
        return Err(Problem::new(
            Code::OperationIncomplete,
            3,
            "published output retained; verification failed",
            Some(path),
        ));
    }
    Ok(())
}

fn details(policy: &Policy, plan: &Plan) {
    envelope::detail(j::obj(&[
        ("recipient", j::str(&policy.recipient)),
        ("format", j::str(&policy.format)),
        (
            "metadata",
            j::str(if policy.keep { "keep" } else { "drop" }),
        ),
        ("source", j::str(&etude_core::redact::path(&policy.source))),
        (
            "destination",
            j::str(&etude_core::redact::path(&policy.into)),
        ),
        ("excluded", j::num(plan.excluded)),
        ("upper_bound_bytes", j::num(plan.bound)),
        (
            "items",
            j::arr(
                (1..=plan.entries.len()).map(|n| j::obj(&[("id", j::str(&format!("i-{n:06}")))])),
            ),
        ),
    ]));
}
fn parse(args: &[String]) -> Result<Policy> {
    let known = [
        "--json",
        "--list",
        "--for",
        "--into",
        "--format",
        "--metadata",
        "--max-size",
    ];
    if args
        .iter()
        .any(|arg| arg.starts_with('-') && !known.contains(&arg.as_str()))
    {
        return Err(Problem::new(Code::UnknownOption, 2, "unknown option", None));
    }
    let invalid = || {
        Problem::new(
            Code::InvalidArguments,
            2,
            "explicit valid recipient and policy required",
            None,
        )
    };
    let mut values = BTreeMap::new();
    let mut source = None;
    let mut list = false;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--json" => {}
            "--list" if !list => list = true,
            "--list" => return Err(invalid()),
            "--for" | "--into" | "--format" | "--metadata" | "--max-size" => {
                i += 1;
                let value = args
                    .get(i)
                    .filter(|s| !s.starts_with('-'))
                    .ok_or_else(invalid)?;
                if values.insert(arg.as_str(), value.as_str()).is_some() {
                    return Err(invalid());
                }
            }
            s if s.starts_with('-') => {
                return Err(Problem::new(Code::UnknownOption, 2, "unknown option", None));
            }
            _ if source.is_none() => source = Some(PathBuf::from(arg)),
            _ => return Err(invalid()),
        }
        i += 1;
    }
    let recipient = *values.get("--for").ok_or_else(invalid)?;
    if !["self", "mac", "other"].contains(&recipient) {
        return Err(invalid());
    }
    let format = *values.get("--format").unwrap_or(&if recipient == "other" {
        "zip"
    } else {
        "tar.gz"
    });
    if !["zip", "tar.gz"].contains(&format) {
        return Err(Problem::new(
            Code::UnsupportedFormat,
            2,
            "supported formats: tar.gz, zip",
            None,
        ));
    }
    let metadata = *values
        .get("--metadata")
        .unwrap_or(&if recipient == "other" { "drop" } else { "keep" });
    if !["keep", "drop"].contains(&metadata) || format == "zip" && metadata == "keep" {
        return Err(invalid());
    }
    let max = match values.get("--max-size") {
        None => DEFAULT_MAX,
        Some(value) => size(value).ok_or_else(invalid)?,
    };
    let source = source.ok_or_else(invalid)?;
    let into = values.get("--into").map(PathBuf::from).unwrap_or_else(|| {
        let mut value = source.as_os_str().to_os_string();
        value.push(format!(".{format}"));
        PathBuf::from(value)
    });
    Ok(Policy {
        source,
        into,
        recipient: recipient.into(),
        format: format.into(),
        keep: metadata == "keep",
        max,
        list,
    })
}
fn size(raw: &str) -> Option<u64> {
    let (number, factor) = match raw.as_bytes().last()? {
        b'G' | b'g' => (&raw[..raw.len() - 1], 1024_u64.pow(3)),
        b'M' | b'm' => (&raw[..raw.len() - 1], 1024_u64.pow(2)),
        b'K' | b'k' => (&raw[..raw.len() - 1], 1024),
        _ => (raw, 1),
    };
    number
        .parse::<u64>()
        .ok()?
        .checked_mul(factor)
        .filter(|n| *n > 0)
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct Identity {
    dev: u64,
    ino: u64,
    len: u64,
    mode: u32,
    mt: (i64, i64),
    ct: (i64, i64),
}
impl Identity {
    fn of(m: &Metadata) -> Self {
        Self {
            dev: m.dev(),
            ino: m.ino(),
            len: m.len(),
            mode: m.mode(),
            mt: (m.mtime(), m.mtime_nsec()),
            ct: (m.ctime(), m.ctime_nsec()),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct Member {
    name: String,
    identity: Identity,
    directory: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct Plan {
    entries: Vec<Member>,
    identities: Vec<(PathBuf, Identity)>,
    excluded: usize,
    bound: u64,
}
fn sensitive(path: &Path, m: &Metadata) -> bool {
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    etude_core::classify::sensitive(&scan::Entry {
        path: path.into(),
        name,
        ext: path
            .extension()
            .unwrap_or_default()
            .to_string_lossy()
            .to_ascii_lowercase(),
        size: m.len(),
        modified: m.modified().ok(),
        is_dir: m.is_dir(),
        is_package: false,
    })
    .is_some()
}
fn package(path: &Path) -> bool {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    scan::is_package(&name)
        || scan::name_matches(&name, scan::BUNDLE_MARKERS)
        || scan::os_says_package(path)
}
fn hazard(name: &str) -> bool {
    if name.is_empty()
        || name.len() > 1024
        || name.starts_with('-')
        || name.contains('\\')
        || name.chars().any(|c| c.is_control())
        || name.starts_with('/')
    {
        return true;
    }
    name.split('/').any(|part| {
        let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        part.is_empty()
            || part == "."
            || part == ".."
            || part.ends_with(['.', ' '])
            || part.contains(':')
            || part.contains(['<', '>', '"', '|', '?', '*'])
            || ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && matches!(stem.as_bytes()[3], b'1'..=b'9'))
    })
}
fn preflight(policy: &Policy) -> Result<Plan> {
    // A package selected as the root would lose its wrapper in a relative manifest.
    if package(&policy.source) {
        return Err(Problem::unsafe_input());
    }
    let mut plan = Plan {
        entries: vec![],
        identities: vec![],
        excluded: 0,
        bound: 65536,
    };
    let mut keys = BTreeSet::new();
    walk(&policy.source, policy, false, 0, &mut plan, &mut keys)?;
    if plan.bound > policy.max {
        return Err(Problem::new(
            Code::InsufficientSpace,
            2,
            "conservative archive bound exceeds --max-size",
            Some(&policy.into),
        ));
    }
    Ok(plan)
}
fn walk(
    path: &Path,
    policy: &Policy,
    in_package: bool,
    depth: usize,
    plan: &mut Plan,
    keys: &mut BTreeSet<String>,
) -> Result<()> {
    if depth > 64 || plan.identities.len() >= MAX_ENTRIES {
        return Err(Problem::unsafe_input());
    }
    let m = observed("selected_tree_metadata", fs::symlink_metadata(path))?;
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_str()
        .ok_or_else(Problem::unsafe_input)?;
    if scan::IN_FLIGHT_SUFFIXES
        .iter()
        .any(|suffix| name.to_ascii_lowercase().ends_with(suffix))
    {
        return Err(Problem::new(
            Code::DownloadInFlight,
            2,
            "download is not complete",
            Some(path),
        ));
    }
    if m.file_type().is_symlink() || !(m.is_dir() || m.is_file()) {
        return Err(Problem::unsafe_input());
    }
    plan.identities.push((path.into(), Identity::of(&m)));
    let litter = name == ".DS_Store" || name.starts_with("._");
    let exclude =
        sensitive(path, &m) || m.is_dir() && scan::never_enter(name) || litter && !policy.keep;
    if exclude {
        if in_package {
            return Err(Problem::unsafe_input());
        }
        plan.excluded += 1;
        return Ok(());
    }
    if path != policy.source {
        let relative = path
            .strip_prefix(&policy.source)
            .map_err(|_| Problem::unsafe_input())?;
        if relative
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(Problem::unsafe_input());
        }
        let name = relative
            .to_str()
            .ok_or_else(Problem::unsafe_input)?
            .to_owned();
        if hazard(&name) {
            return Err(Problem::unsafe_input());
        }
        let key =
            etude_core::apply::dedupe_key(relative, true).map_err(|_| Problem::unsafe_input())?;
        if !keys.insert(key) {
            return Err(Problem::unsafe_input());
        }
        // A generous per-member allowance covers headers and metadata encoding.
        let metadata = if policy.keep { xattr_size(path)? } else { 0 };
        plan.bound = plan
            .bound
            .checked_add(m.len().saturating_mul(2))
            .and_then(|n| n.checked_add(metadata.saturating_mul(4)))
            .and_then(|n| n.checked_add(65536))
            .ok_or_else(Problem::unsafe_input)?;
        plan.entries.push(Member {
            name,
            identity: Identity::of(&m),
            directory: m.is_dir(),
        });
    }
    if m.is_dir() {
        let mut children = observed("selected_tree_metadata", fs::read_dir(path))?
            .map(|e| e.map(|e| e.path()))
            .collect::<io::Result<Vec<_>>>()
            .map_err(io_problem)?;
        children.sort();
        for child in children {
            let child_package = in_package || package(&child);
            walk(&child, policy, child_package, depth + 1, plan, keys)?;
        }
    }
    Ok(())
}
#[cfg(target_os = "macos")]
fn xattr_size(path: &Path) -> Result<u64> {
    use std::os::unix::ffi::OsStrExt;
    unsafe extern "C" {
        fn getxattr(
            path: *const i8,
            name: *const i8,
            value: *mut u8,
            size: usize,
            position: u32,
            options: i32,
        ) -> isize;
    }
    // The system utility emits names only. Attribute values stay out of this process.
    let mut child = Command::new("/usr/bin/xattr")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(io_problem)?;
    let mut names = Vec::new();
    let read = child
        .stdout
        .take()
        .unwrap()
        .take(1024 * 1024 + 1)
        .read_to_end(&mut names);
    if read.is_err() || names.len() > 1024 * 1024 {
        let _ = child.kill();
        let _ = child.wait();
        return Err(Problem::unsafe_input());
    }
    if !child.wait().map_err(io_problem)?.success() {
        return Err(Problem::unsafe_input());
    }
    let path =
        std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| Problem::unsafe_input())?;
    let mut total = 0u64;
    for name in names.split(|b| *b == b'\n').filter(|name| !name.is_empty()) {
        let name = std::ffi::CString::new(name).map_err(|_| Problem::unsafe_input())?;
        // SAFETY: both strings are NUL terminated; a null buffer queries length only.
        let len = unsafe { getxattr(path.as_ptr(), name.as_ptr(), std::ptr::null_mut(), 0, 0, 1) };
        if len < 0 {
            return Err(io_problem(io::Error::last_os_error()));
        }
        total = total
            .checked_add(len as u64)
            .ok_or_else(Problem::unsafe_input)?;
    }
    Ok(total)
}
#[cfg(not(target_os = "macos"))]
fn xattr_size(_: &Path) -> Result<u64> {
    Err(Problem::unsafe_input())
}

fn free_bytes(path: &Path) -> Result<u64> {
    let out = Command::new("/bin/df")
        .args(["-Pk"])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .map_err(io_problem)?;
    scan::record_read_outcome("volume_free_space", out.status.success());
    if !out.status.success() {
        return Err(Problem::unsafe_input());
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .nth(1)
        .and_then(|s| s.split_whitespace().nth(3))
        .and_then(|s| s.parse::<u64>().ok())
        .and_then(|n| n.checked_mul(1024))
        .ok_or_else(Problem::unsafe_input)
}
struct Staging {
    path: PathBuf,
    file: File,
    identity: Identity,
}
impl Staging {
    fn create(dest: &Path) -> Result<Self> {
        for _ in 0..128 {
            let mut nonce = [0u8; 16];
            observed(
                "os_random_bytes",
                File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut nonce)),
            )?;
            let nonce = nonce.iter().map(|b| format!("{b:02x}")).collect::<String>();
            let path = dest.with_file_name(format!(
                ".{}.pack-{nonce}.partial",
                dest.file_name().unwrap_or_default().to_string_lossy()
            ));
            match OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
            {
                Ok(file) => {
                    let identity = Identity::of(&file.metadata().map_err(io_problem)?);
                    return Ok(Self {
                        path,
                        file,
                        identity,
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(io_problem(e)),
            }
        }
        Err(Problem::new(
            Code::InternalError,
            3,
            "could not reserve staging",
            None,
        ))
    }
    fn verify(&self) -> Result<()> {
        let m = fs::symlink_metadata(&self.path).map_err(io_problem)?;
        if m.dev() != self.identity.dev || m.ino() != self.identity.ino || !m.is_file() {
            return Err(Problem::unsafe_input());
        }
        Ok(())
    }
}
impl Drop for Staging {
    fn drop(&mut self) {
        if self.verify().is_ok() && fs::remove_file(&self.path).is_err() {
            eprintln!(
                "pack: private staging remains at {}",
                etude_core::redact::path(&self.path)
            );
        }
    }
}
fn writer(policy: &Policy, staging: &Staging) -> Command {
    let mut cmd = Command::new(TAR);
    cmd.arg("-c");
    if policy.format == "zip" {
        cmd.args(["--format", "zip"]);
    } else {
        cmd.args(["-z", "--format", "pax"]);
    }
    if policy.keep {
        cmd.arg("--mac-metadata").env_remove("COPYFILE_DISABLE");
    } else {
        cmd.args([
            "--no-mac-metadata",
            "--no-xattrs",
            "--no-acls",
            "--no-fflags",
        ]);
        cmd.env("COPYFILE_DISABLE", "1");
    }
    cmd.args(["-f", "-"])
        .arg("-C")
        .arg(&policy.source)
        .args(["-n", "--null", "-T", "-"]);
    cmd.stdin(Stdio::piped())
        .stdout(staging.file.try_clone().expect("staging handle"))
        .stderr(Stdio::null());
    cmd
}
fn readback(path: &Path, plan: &Plan, keep: bool) -> Result<()> {
    let mut cmd = Command::new(TAR);
    cmd.args(["-tvf"])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env("LC_ALL", "en_US.UTF-8");
    let mut child = cmd.spawn().map_err(io_problem)?;
    let mut data = Vec::new();
    let limit = (plan.entries.len() as u64 + 1) * 4096 * 2;
    let read = child
        .stdout
        .take()
        .unwrap()
        .take(limit + 1)
        .read_to_end(&mut data);
    if read.is_err() || data.len() as u64 > limit {
        let _ = child.kill();
        let _ = child.wait();
        return Err(Problem::unsafe_input());
    }
    let status = child.wait().map_err(io_problem)?;
    scan::record_read_outcome("archive_readback_bytes", status.success());
    if !status.success() {
        return Err(Problem::unsafe_input());
    }
    let text = std::str::from_utf8(&data).map_err(|_| Problem::unsafe_input())?;
    let mut actual = BTreeMap::new();
    for line in text.lines() {
        let (kind, name) = typed_member(line).ok_or_else(Problem::unsafe_input)?;
        if kind != b'-' && kind != b'd' {
            return Err(Problem::unsafe_input());
        }
        if actual
            .insert(comparison_name(name)?, kind == b'd')
            .is_some()
        {
            return Err(Problem::unsafe_input());
        }
    }
    let expected = plan
        .entries
        .iter()
        .map(|m| comparison_name(&m.name).map(|name| (name, m.directory)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    if keep {
        // AppleDouble companions may be generated by the system writer.
        actual.retain(|name, _| {
            if expected.contains_key(name) {
                return true;
            }
            let p = Path::new(name);
            let Some(base) = p
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_prefix("._"))
            else {
                return true;
            };
            !expected.contains_key(&p.with_file_name(base).to_string_lossy().into_owned())
        });
    }
    if actual != expected {
        return Err(Problem::unsafe_input());
    }
    // Stream every payload through the system reader as well as listing headers.
    let mut child = Command::new(TAR)
        .args(["-xOf"])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(io_problem)?;
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().map_err(io_problem)? {
            if !status.success() {
                return Err(Problem::unsafe_input());
            }
            break;
        }
        if STOP.load(Ordering::Relaxed) || start.elapsed() > Duration::from_secs(300) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Problem::unsafe_input());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    scan::verify_read("archive_readback_bytes");
    Ok(())
}
fn comparison_name(name: &str) -> Result<String> {
    etude_core::apply::dedupe_key(Path::new(name), false).map_err(|_| Problem::unsafe_input())
}
fn typed_member(line: &str) -> Option<(u8, &str)> {
    let t = line.trim_start();
    let mode = t.split_whitespace().next()?;
    if mode.len() != 10 {
        return None;
    }
    let rest = t[mode.len()..].trim_start();
    let mut start = None;
    let mut previous = None;
    let mut before_previous = None;
    for (end, ch) in rest.char_indices() {
        if ch.is_whitespace() {
            let Some(begin) = start.take() else {
                continue;
            };
            let token = &rest[begin..end];
            let time = token.len() == 5
                && token.as_bytes()[2] == b':'
                && token
                    .bytes()
                    .enumerate()
                    .all(|(i, b)| i == 2 || b.is_ascii_digit());
            let year = token.len() == 4
                && token.bytes().all(|b| b.is_ascii_digit())
                && previous.is_some_and(|s: &str| {
                    (1..=2).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit())
                })
                && before_previous.is_some_and(|s: &str| {
                    s.len() == 3 && s.bytes().all(|b| b.is_ascii_alphabetic())
                });
            if time || year {
                return Some((
                    mode.as_bytes()[0],
                    rest[end + ch.len_utf8()..].trim_end_matches('/'),
                ));
            }
            before_previous = previous;
            previous = Some(token);
        } else if start.is_none() {
            start = Some(end);
        }
    }
    None
}
#[cfg(target_os = "macos")]
fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let from = std::ffi::CString::new(from.as_os_str().as_bytes())?;
    let to = std::ffi::CString::new(to.as_os_str().as_bytes())?;
    unsafe extern "C" {
        fn renamex_np(from: *const i8, to: *const i8, flags: u32) -> i32;
    }
    // SAFETY: both paths are live NUL-terminated strings; 4 is RENAME_EXCL.
    if unsafe { renamex_np(from.as_ptr(), to.as_ptr(), 4) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
#[cfg(not(target_os = "macos"))]
fn rename_no_replace(_: &Path, _: &Path) -> io::Result<()> {
    Err(io::ErrorKind::Unsupported.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "etudes-pack-test-{}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir(&root).unwrap();
            fs::create_dir(root.join("source")).unwrap();
            Self(root)
        }
        fn policy(&self) -> Policy {
            parse(&[
                self.0.join("source").to_str().unwrap().into(),
                "--for".into(),
                "other".into(),
                "--into".into(),
                self.0.join("out.zip").to_str().unwrap().into(),
            ])
            .unwrap()
        }
        fn write(&self, name: &str) {
            fs::write(self.0.join("source").join(name), b"synthetic payload").unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn sensitive_parity() {
        let f = Fixture::new();
        // Consume the shared fixture names without adding a runtime dependency.
        let shared = include_str!("../../fixtures/src/lib.rs");
        let names = shared
            .split("pub const SENSITIVE_NAMES: &[&str] = &[")
            .nth(1)
            .unwrap()
            .split("];")
            .next()
            .unwrap();
        let names: Vec<_> = names
            .lines()
            .filter_map(|line| {
                line.trim()
                    .strip_prefix('"')
                    .and_then(|line| line.strip_suffix("\","))
            })
            .collect();
        assert!(!names.is_empty());
        for name in &names {
            f.write(name);
        }
        f.write("ordinary.txt");
        let policy = f.policy();
        let manifest = preflight(&policy).unwrap();
        assert_eq!(manifest.excluded, names.len());
        assert_eq!(manifest.entries.len(), 1);
        let scanned = scan::scan(
            &policy.source,
            &scan::ScanConfig {
                grace: None,
                allow_sync: true,
                ..Default::default()
            },
        )
        .unwrap();
        let swept = etude_core::plan::build(&scanned);
        let held = swept
            .untouched
            .iter()
            .filter(|(_, why)| matches!(why, etude_core::Untouched::LooksPersonal(_)))
            .count();
        assert_eq!(held, names.len());
    }

    #[test]
    fn in_flight_download() {
        let f = Fixture::new();
        for suffix in scan::IN_FLIGHT_SUFFIXES {
            let name = format!("movie{suffix}");
            f.write(&name);
            let error = preflight(&f.policy()).unwrap_err();
            assert_eq!(error.code, Code::DownloadInFlight);
            assert_eq!(
                error.path.as_deref(),
                Some(f.0.join("source").join(&name).as_path())
            );
            fs::remove_file(f.0.join("source").join(&name)).unwrap();
        }
    }
    #[test]
    fn recent_file() {
        let f = Fixture::new();
        f.write("just-created.txt");
        let plan = preflight(&f.policy()).unwrap();
        assert_eq!(plan.entries[0].name, "just-created.txt");
    }
    #[test]
    fn opaque_package() {
        let f = Fixture::new();
        fs::create_dir(f.0.join("source/Example.app")).unwrap();
        fs::write(f.0.join("source/Example.app/one"), b"one").unwrap();
        let policy = f.policy();
        assert_eq!(preflight(&policy).unwrap().entries.len(), 2);
        let mut selected_package = policy.clone();
        selected_package.source = policy.source.join("Example.app");
        assert_eq!(
            preflight(&selected_package).unwrap_err().code,
            Code::UnsafeInput
        );
        fs::write(f.0.join("source/Example.app/passport.txt"), b"private").unwrap();
        assert_eq!(preflight(&policy).unwrap_err().code, Code::UnsafeInput);
    }
    #[test]
    fn archive_path_hazards() {
        for name in [
            "../escape",
            "/absolute",
            "a/../b",
            "C:/drive",
            "NUL",
            "COM1.txt",
            "a\\b",
            "a\nb",
            "-option",
            "trailing.",
            "a//b",
        ] {
            assert!(hazard(name), "accepted {name:?}");
        }
        for name in ["ordinary", ".hidden", "notes 12:30", "café"] {
            if name.contains(':') {
                continue;
            }
            assert!(!hazard(name), "refused {name:?}");
        }
        assert_eq!(
            etude_core::apply::dedupe_key(Path::new("A.txt"), true).unwrap(),
            etude_core::apply::dedupe_key(Path::new("a.txt"), true).unwrap()
        );
        if cfg!(target_os = "macos") {
            assert_eq!(
                etude_core::apply::dedupe_key(Path::new("café"), true).unwrap(),
                etude_core::apply::dedupe_key(Path::new("cafe\u{301}"), true).unwrap()
            );
        }
        let f = Fixture::new();
        std::os::unix::fs::symlink("../outside", f.0.join("source/link")).unwrap();
        assert_eq!(preflight(&f.policy()).unwrap_err().code, Code::UnsafeInput);
        fs::remove_file(f.0.join("source/link")).unwrap();
        f.write("ordinary");
        std::os::unix::fs::symlink("ordinary", f.0.join("source/link")).unwrap();
        assert_eq!(preflight(&f.policy()).unwrap_err().code, Code::UnsafeInput);
        fs::remove_file(f.0.join("source/link")).unwrap();
        f.write("CON.txt");
        assert_eq!(preflight(&f.policy()).unwrap_err().code, Code::UnsafeInput);
    }
    #[test]
    fn metadata_size() {
        let f = Fixture::new();
        f.write("ordinary");
        f.write(".DS_Store");
        f.write("._ordinary");
        f.write(".hidden");
        f.write(".git");
        let mut policy = f.policy();
        let plan = preflight(&policy).unwrap();
        assert_eq!(plan.excluded, 2);
        assert!(plan.entries.iter().any(|entry| entry.name == ".git"));
        policy.max = 1;
        assert_eq!(
            preflight(&policy).unwrap_err().code,
            Code::InsufficientSpace
        );
        assert!(
            parse(&[
                "source".into(),
                "--for".into(),
                "other".into(),
                "--metadata".into(),
                "keep".into()
            ])
            .is_err()
        );
        assert_eq!(size("2G"), Some(2 * 1024_u64.pow(3)));
        assert_eq!(size("18446744073709551615G"), None);
        assert_eq!(size("0"), None);
    }
    #[test]
    fn recipient_source_preservation() {
        let f = Fixture::new();
        f.write("ordinary");
        let policy = f.policy();
        let before = preflight(&policy).unwrap();
        let _ = preflight(&policy).unwrap();
        assert_eq!(preflight(&policy).unwrap(), before);
        fs::write(f.0.join("source/ordinary"), b"changed source").unwrap();
        assert_ne!(preflight(&policy).unwrap(), before);
        for (recipient, format, keep) in [
            ("self", "tar.gz", true),
            ("mac", "tar.gz", true),
            ("other", "zip", false),
        ] {
            let p = parse(&["source".into(), "--for".into(), recipient.into()]).unwrap();
            assert_eq!(p.format, format);
            assert_eq!(p.keep, keep);
        }
        assert_eq!(
            parse(&["source".into()]).unwrap_err().code,
            Code::InvalidArguments
        );
    }

    #[test]
    fn publish_never_replaces() {
        let f = Fixture::new();
        let dest = f.0.join("destination");
        for kind in [
            "file",
            "empty-directory",
            "directory",
            "symlink",
            "dangling-symlink",
        ] {
            let staging = Staging::create(&dest).unwrap();
            match kind {
                "file" => fs::write(&dest, b"existing bytes").unwrap(),
                "empty-directory" => fs::create_dir(&dest).unwrap(),
                "directory" => {
                    fs::create_dir(&dest).unwrap();
                    fs::write(dest.join("sentinel"), b"existing bytes").unwrap();
                }
                "symlink" => std::os::unix::fs::symlink("source", &dest).unwrap(),
                _ => std::os::unix::fs::symlink("absent", &dest).unwrap(),
            }
            let before = Identity::of(&fs::symlink_metadata(&dest).unwrap());
            assert!(
                rename_no_replace(&staging.path, &dest).is_err(),
                "replaced {kind}"
            );
            assert_eq!(Identity::of(&fs::symlink_metadata(&dest).unwrap()), before);
            if kind.ends_with("directory") {
                fs::remove_dir_all(&dest).unwrap();
            } else {
                fs::remove_file(&dest).unwrap();
            }
        }
    }
    #[test]
    fn cleanup_requires_owned_identity() {
        let f = Fixture::new();
        let staging = Staging::create(&f.0.join("out")).unwrap();
        let path = staging.path.clone();
        fs::rename(&path, f.0.join("moved")).unwrap();
        fs::write(&path, b"replacement belongs to another writer").unwrap();
        drop(staging);
        assert_eq!(
            fs::read(path).unwrap(),
            b"replacement belongs to another writer"
        );
    }
    #[test]
    fn typed_manifest_parser_preserves_spaces_and_rejects_links() {
        assert_eq!(
            typed_member("-rw-r--r--  0 user group 4 Oct 29 12:30 notes  12:30"),
            Some((b'-', "notes  12:30"))
        );
        assert_eq!(
            typed_member("drwx------  0 user group 0 Jan  1  2020 directory/"),
            Some((b'd', "directory"))
        );
        assert_eq!(typed_member("invalid"), None);
    }
    #[test]
    fn failed_post_publication_verification_retains_output() {
        let fixture = Fixture::new();
        let output = fixture.0.join("published.zip");
        fs::write(&output, b"published bytes").unwrap();
        let error = verify_published(&output, || Err(Problem::unsafe_input())).unwrap_err();
        assert_eq!(error.code, Code::OperationIncomplete);
        assert_eq!(error.exit, 3);
        assert_eq!(fs::read(&output).unwrap(), b"published bytes");
    }
}
