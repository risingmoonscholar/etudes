//! pack: make a new archive from a folder under a recipient you name.
//!
//! The order that makes it safe: decide everything from metadata, write only to
//! a private file beside the destination, read that file back, re-check the
//! folder, then give it the requested name with a rename that cannot replace
//! anything. The requested name holds a complete archive or nothing.
//!
//! Every member sits under one top folder named after the source. That keeps a
//! stranger's `unzip` from spilling files, and it means `unpack` puts every path
//! back where it was: unpack strips one wrapper, so an archive whose only top
//! item was a real subfolder would otherwise lose that level.

use etude_cli_support::{
    contract, counted, envelope, plain_io_error, quoted_path,
    reason::{CODES, Code, Reason, Subject},
    shown, shown_text,
};
use etude_core::{json as j, scan};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, ExitCode, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const TAR: &str = "/usr/bin/tar";
const MAX_ENTRIES: usize = 20_000;
/// The same depth `unpack` accepts, counting the top folder. A deeper tree would
/// make an archive that unpack refuses.
const MAX_DEPTH: usize = 32;
const DEFAULT_MAX: u64 = 4 * 1024 * 1024 * 1024;
const STDERR_LIMIT: u64 = 64 * 1024;
static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn stop(_: i32) {
    STOP.store(true, Ordering::Relaxed);
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // A question about any command is answered as help, including `contract`.
    let scope = if etude_cli_support::wants_help(&args) {
        let mut scope = vec!["help".to_string()];
        scope.extend(args.iter().filter(|arg| *arg == "--json").cloned());
        scope
    } else {
        args.clone()
    };
    envelope::begin("pack", VERSION, &scope);
    unsafe extern "C" {
        fn signal(number: i32, handler: usize) -> usize;
    }
    // SAFETY: the handler only sets an atomic flag and lives for the process.
    // Hang up, interrupt and terminate all end a run the same way: the writer is
    // stopped and the unfinished file is removed. A signal that was already
    // being ignored stays ignored, so `nohup pack` keeps running.
    unsafe {
        const SIG_IGN: usize = 1;
        for number in [1, 2, 15] {
            if signal(number, stop as *const () as usize) == SIG_IGN {
                signal(number, SIG_IGN);
            }
        }
    }
    let result = std::panic::catch_unwind(|| run(&args));
    let code = match result {
        Ok(Ok(())) => ExitCode::SUCCESS,
        Ok(Err(error)) => error.report(),
        Err(_) => Problem::new(
            Code::InternalError,
            3,
            "pack stopped on an internal error. The folder was not changed. Run it again; if it \
             stops again, please report it.",
            None,
        )
        .report(),
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
    fn invalid(message: impl Into<String>) -> Self {
        Self::new(Code::InvalidArguments, 2, message, None)
    }
    fn report(self) -> ExitCode {
        if self.code == Code::NothingToDo {
            eprintln!("pack: {}", self.message);
            return ExitCode::from(1);
        }
        let kind = CODES
            .iter()
            .find(|spec| spec.code == self.code)
            .and_then(|spec| spec.subject_kind);
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
        ExitCode::from(self.exit)
    }
}

/// The folder as the person typed it, and the steps that name it.
#[derive(Debug, Clone)]
struct Policy {
    source: PathBuf,
    typed: PathBuf,
    into: Option<PathBuf>,
    recipient: String,
    format: String,
    keep: bool,
    max: u64,
    list: bool,
}
impl Policy {
    fn shown(&self) -> String {
        shown(&self.typed)
    }
    fn quoted(&self) -> String {
        quoted_path(&self.typed)
    }
    fn destination(&self) -> &Path {
        self.into.as_deref().unwrap_or(Path::new(""))
    }
    /// A refusal about something inside the folder. The name stays hidden; the
    /// step finds it, and the person reads the result on their own terminal.
    fn refuse(&self, what: &str, step: String) -> Problem {
        Problem::new(
            Code::UnsafeInput,
            2,
            format!("{what} No archive was made. {step}"),
            Some(&self.source),
        )
    }
    fn changed(&self) -> Problem {
        self.refuse(
            &format!("{} changed while pack was reading it.", self.shown()),
            "Run pack again when nothing is writing to it.".into(),
        )
    }
    fn denied(&self, error: &io::Error) -> Problem {
        let advice = scan::permission_advice(error).unwrap_or("Check the permissions.");
        Problem::new(
            Code::PermissionDenied,
            3,
            format!(
                "cannot read something in {}: {}. No archive was made. {advice} To list what you \
                 cannot read: find {} ! -perm -u+r",
                self.shown(),
                plain_io_error(error),
                self.quoted()
            ),
            Some(&self.source),
        )
    }
    /// A failed read of the folder.
    fn read_failed(&self, error: io::Error) -> Problem {
        match error.kind() {
            io::ErrorKind::PermissionDenied => self.denied(&error),
            io::ErrorKind::NotFound => self.changed(),
            _ => Problem::new(
                Code::InternalError,
                3,
                format!(
                    "cannot read {}: {}. No archive was made. Check the disk, then run pack again.",
                    self.shown(),
                    plain_io_error(&error)
                ),
                None,
            ),
        }
    }
    /// A failed write in the destination folder.
    fn write_failed(&self, error: io::Error) -> Problem {
        let destination = self.destination();
        let folder = destination.parent().unwrap_or(Path::new("."));
        let name = shown(destination);
        match error.kind() {
            io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem => {
                let advice = if error.kind() == io::ErrorKind::ReadOnlyFilesystem {
                    "That disk is read-only."
                } else {
                    scan::permission_advice(&error).unwrap_or("Check the permissions.")
                };
                Problem::new(
                    Code::PermissionDenied,
                    3,
                    format!(
                        "cannot write to {}: {}. No archive was made. {advice} Or choose another \
                         place with --into FILE.",
                        shown(folder),
                        plain_io_error(&error)
                    ),
                    Some(destination),
                )
            }
            io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded => {
                out_of_room(&name, destination)
            }
            _ => Problem::new(
                Code::InternalError,
                3,
                format!(
                    "cannot write {name}: {}. No archive was made. Choose another place with \
                     --into FILE.",
                    plain_io_error(&error)
                ),
                None,
            ),
        }
    }
}

fn out_of_room(name: &str, destination: &Path) -> Problem {
    Problem::new(
        Code::InsufficientSpace,
        3,
        format!(
            "the disk ran out of room while writing {name}. No archive was made. Free some space, \
             or choose another disk with --into FILE."
        ),
        Some(destination),
    )
}

fn stopped(policy: &Policy) -> Problem {
    Problem::new(
        Code::ExtractionStopped,
        2,
        format!(
            "stopped by a signal before the archive was finished. No archive was made, and {} was \
             not changed. Run pack again when ready.",
            policy.shown()
        ),
        Some(&policy.source),
    )
}

fn check_stop(policy: &Policy) -> Result<()> {
    if STOP.load(Ordering::Relaxed) {
        return Err(stopped(policy));
    }
    Ok(())
}

const HELP: &str = "pack: make a new archive from a folder, for a recipient you name

USAGE
    pack FOLDER --for self|mac|other     make FOLDER.tar.gz or FOLDER.zip beside it
    --into FILE                          choose the archive's name and place
    --format tar.gz|zip                  self and mac default to tar.gz, other to zip
    --metadata keep|drop                 self and mac default to keep, other to drop
    --max-size N[K|M|G]                  largest archive allowed; 4G unless you say
    --list                               say what would happen and write nothing
    --json                               machine-readable output (for agents)
    pack contract --json                 print the versioned capability contract
    pack help | --version

WHAT GOES IN
Files, hidden files and empty folders, under one top folder named after FOLDER.
Packages go in whole. Unpack puts every path and byte back. Recent files count.

WHAT IS LEFT OUT, COUNTED AND NEVER NAMED
Names that look personal or like credentials, as sweep judges them, and folders
such as .ssh, .git and node_modules. Stored ._ files always; .DS_Store, Thumbs.db
and __MACOSX with drop. A package that would lose a private-looking part is
refused rather than split.

MAC METADATA
keep carries Finder tags, the download warning and other attributes, and needs
tar.gz. drop removes them all. Pack counts the items with tags, the download
warning, and forks or Finder info, and says what it did. A file that loses the
download warning will not warn after it is extracted. Owners are not kept. A file
with another hard-link name outside the archive goes in as its own copy.

WHAT IS REFUSED
Links, hard links between two names in FOLDER, setuid and setgid items, files
still downloading or stored only in the cloud, unreadable items, names that are
unsafe on another system, names that differ only by case or accent, more than
20000 items, items more than 31 levels down. System and credential folders, your
home folder and any folder holding it. Running as root.

WHAT IT PROMISES
It never replaces a file. An archive appears at FILE only when complete and read
back. FOLDER is not changed. If FOLDER changes during the run, no archive is made.
The size limit is checked while writing and can be passed briefly between checks.
A later run for the same FILE removes the unfinished file a killed run left, after
proving that it is pack's own and that nothing is still writing it.

Exit: 0 done, 1 nothing to include, 2 refused, 3 error or incomplete.";

fn run(args: &[String]) -> Result<()> {
    if etude_cli_support::wants_help(args) {
        envelope::print(HELP.into());
        return Ok(());
    }
    match args.first().map(String::as_str) {
        Some("contract") => {
            if args.len() > 2 || args.get(1).is_some_and(|arg| arg != "--json") {
                return Err(Problem::invalid(
                    "contract takes only --json. Nothing was done.",
                ));
            }
            contract::command(contract::Tool::Pack, VERSION, args, &[], None);
            return Ok(());
        }
        Some("help")
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
    refuse_root_user()?;
    resolve_source(&mut policy)?;
    resolve_destination(&mut policy)?;
    // Before anything can refuse, so a leftover is found whatever happens next.
    let leftovers = sweep_leftovers(policy.destination(), !policy.list);
    leftovers.report(policy.destination());
    if fs::symlink_metadata(policy.destination()).is_ok() {
        return Err(occupied(policy.destination()));
    }
    let plan = preflight(&policy)?;
    details(&policy, &plan, &leftovers);
    if plan.entries.is_empty() {
        envelope::status("nothing_to_do");
        envelope::reason(Reason::nothing_to_do());
        let why = if plan.left_out() == 0 {
            format!("{} is empty.", policy.shown())
        } else {
            format!(
                "everything in {} was left out: {}.",
                policy.shown(),
                plan.left_out_text()
            )
        };
        return Err(Problem::new(
            Code::NothingToDo,
            1,
            format!("{why} No archive was made."),
            None,
        ));
    }
    let json = args.iter().any(|arg| arg == "--json");
    if policy.list {
        if !json {
            envelope::print(summary(
                &policy,
                &plan,
                &format!(
                    "Would make {}. Nothing was written.",
                    shown(policy.destination())
                ),
            ));
        }
        return Ok(());
    }
    let mut staging = Staging::create(policy.destination()).map_err(|e| policy.write_failed(e))?;
    let written = write_archive(&policy, &plan, &staging);
    let written = match written {
        Ok(()) => publish(&policy, &plan, &mut staging),
        Err(error) => Err(error),
    };
    let written = match (written, staging.remove()) {
        (Err(mut error), Cleanup::Remains(step)) => {
            error.message.push(' ');
            error.message.push_str(&step);
            Err(error)
        }
        (result, _) => result,
    };
    written?;
    if !json {
        envelope::print(summary(
            &policy,
            &plan,
            &format!("Made {}.", shown(policy.destination())),
        ));
    }
    Ok(())
}

fn write_archive(policy: &Policy, plan: &Plan, staging: &Staging) -> Result<()> {
    // After the file exists: a read-only disk is not a matter of free space.
    let parent = policy.destination().parent().unwrap_or(Path::new("."));
    if let Some(available) = free_bytes(parent)
        && available < plan.bound.saturating_add(16 * 1024 * 1024)
    {
        return Err(Problem::new(
            Code::InsufficientSpace,
            2,
            format!(
                "{} could need up to {} on its disk, and {} is free. No archive was made. Free \
                 some space, or choose another disk with --into FILE.",
                shown(policy.destination()),
                human(plan.bound.saturating_add(16 * 1024 * 1024)),
                human(available)
            ),
            Some(policy.destination()),
        ));
    }
    let mut cmd = writer(policy, staging).map_err(|e| policy.write_failed(e))?;
    let manifest = plan
        .entries
        .iter()
        .flat_map(|e| e.name.bytes().chain(std::iter::once(0)))
        .collect::<Vec<_>>();
    let mut child = cmd.spawn().map_err(|e| system_tool_failed(policy, &e))?;
    let stdin = child.stdin.take();
    let stderr = child.stderr.take();
    // On threads, so a long manifest or a noisy writer cannot delay a stop.
    let feeder = std::thread::spawn(move || {
        if let Some(mut stdin) = stdin {
            let _ = stdin.write_all(&manifest);
        }
    });
    let complaints = std::thread::spawn(move || {
        let mut text = Vec::new();
        if let Some(stderr) = stderr {
            let _ = stderr.take(STDERR_LIMIT).read_to_end(&mut text);
        }
        String::from_utf8_lossy(&text).to_ascii_lowercase()
    });
    let finished = (|| {
        loop {
            let status = child
                .try_wait()
                .map_err(|e| system_tool_failed(policy, &e))?;
            let size = staging
                .file
                .metadata()
                .map_err(|e| policy.write_failed(e))?
                .len();
            // Only the person's limit. The bound is an estimate for the free-space
            // check: it cannot count an attribute pack does not name, and tar
            // stores those too, so passing it is no failure.
            if size > policy.max {
                return Err(passed_limit(policy, size));
            }
            check_stop(policy)?;
            if let Some(status) = status {
                return Ok(status);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    })();
    if finished.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    let _ = feeder.join();
    let complaints = complaints.join().unwrap_or_default();
    if !finished?.success() {
        return Err(writer_failed(policy, &complaints));
    }
    staging.file.sync_all().map_err(|e| policy.write_failed(e))
}

/// What the system tool said, turned into a cause. Its words can hold file
/// names, so they decide the message and are never printed.
fn writer_failed(policy: &Policy, complaints: &str) -> Problem {
    let destination = policy.destination();
    if complaints.contains("no space left") || complaints.contains("write error") {
        return out_of_room(&shown(destination), destination);
    }
    if complaints.contains("permission denied") || complaints.contains("not permitted") {
        return policy.denied(&io::Error::from_raw_os_error(13));
    }
    if complaints.contains("no such file") || complaints.contains("changed") {
        return policy.changed();
    }
    Problem::new(
        Code::InternalError,
        3,
        format!(
            "the system tar could not finish {}. No archive was made. Check the folder and the \
             disk, then run pack again.",
            shown(destination)
        ),
        None,
    )
}

fn system_tool_failed(policy: &Policy, error: &io::Error) -> Problem {
    Problem::new(
        Code::InternalError,
        3,
        format!(
            "could not run the system tar for {}: {}. No archive was made.",
            shown(policy.destination()),
            plain_io_error(error)
        ),
        None,
    )
}

fn over_limit(policy: &Policy, needed: u64) -> Problem {
    Problem::new(
        Code::SizeLimitExceeded,
        2,
        format!(
            "the archive of {} could reach {}, over the limit of {}. No archive was made. To \
             allow it: --max-size {}",
            policy.shown(),
            human(needed),
            human(policy.max),
            size_flag(needed)
        ),
        Some(&policy.source),
    )
}

fn passed_limit(policy: &Policy, size: u64) -> Problem {
    Problem::new(
        Code::SizeLimitExceeded,
        2,
        format!(
            "the archive of {} passed the limit of {} while it was being written, at {}. No \
             archive was made. To allow more: --max-size {}",
            policy.shown(),
            human(policy.max),
            human(size),
            size_flag(size.saturating_mul(2))
        ),
        Some(&policy.source),
    )
}

/// Read the file back, re-check the folder, and give the file its name.
fn publish(policy: &Policy, plan: &Plan, staging: &mut Staging) -> Result<()> {
    let checked = readback(policy, &staging.path, plan);
    // A folder that changed is the cause of a mismatch, so it is asked first.
    let current = preflight(policy)?;
    if current != *plan {
        return Err(policy.changed());
    }
    checked?;
    check_stop(policy)?;
    if !staging.still_ours() {
        return Err(Problem::new(
            Code::InternalError,
            3,
            format!(
                "something else replaced the unfinished file for {}. No archive was made. Run \
                 pack again.",
                shown(policy.destination())
            ),
            None,
        ));
    }
    let destination = policy.destination();
    sys::clear_mark(&staging.file);
    if let Err(error) = rename_no_replace(&staging.path, destination) {
        return Err(match error.raw_os_error() {
            Some(17) => occupied(destination),
            // ENOTSUP, EINVAL: this volume cannot promise not to replace a file.
            Some(45 | 102 | 22) => Problem::new(
                Code::InternalError,
                3,
                format!(
                    "the disk holding {} cannot add a file without risking replacing one, so pack \
                     will not use it. No archive was made. Choose another disk with --into FILE.",
                    shown(destination)
                ),
                None,
            ),
            _ => policy.write_failed(error),
        });
    }
    staging.published = true;
    envelope::effect("archives_published", 1);
    verify_published(destination, || {
        readback(policy, destination, plan)?;
        let same = fs::symlink_metadata(destination)
            .and_then(|m| {
                staging
                    .file
                    .metadata()
                    .map(|held| Identity::of(&m) == Identity::of(&held))
            })
            .map_err(|e| policy.read_failed(e))?;
        if !same {
            return Err(policy.changed());
        }
        Ok(())
    })
}

fn occupied(destination: &Path) -> Problem {
    Problem::new(
        Code::DestinationExists,
        2,
        format!(
            "{} already exists, and pack never replaces anything. No archive was made. Choose \
             another name with --into FILE.",
            shown(destination)
        ),
        Some(destination),
    )
}

fn verify_published(path: &Path, verify: impl FnOnce() -> Result<()>) -> Result<()> {
    if verify().is_err() {
        envelope::status("incomplete");
        return Err(Problem::new(
            Code::OperationIncomplete,
            3,
            format!(
                "{0} was made and passed its first check, but its second check did not finish. \
                 It was kept. Look inside before you rely on it: unpack {1} --list",
                shown(path),
                quoted_path(path)
            ),
            Some(path),
        ));
    }
    Ok(())
}

fn human(bytes: u64) -> String {
    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = 1024 * MIB;
    if bytes >= GIB {
        format!("{:.1} GB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.1} MB", bytes as f64 / MIB as f64)
    } else {
        counted(bytes as usize, "byte", "bytes")
    }
}

/// A `--max-size` value at least this large, rounded up to a whole unit.
fn size_flag(bytes: u64) -> String {
    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = 1024 * MIB;
    if bytes >= GIB {
        format!("{}G", bytes.div_ceil(GIB))
    } else {
        format!("{}M", bytes.div_ceil(MIB).max(1))
    }
}

/// The lines a person reads after a run or a `--list`.
fn summary(policy: &Policy, plan: &Plan, first: &str) -> String {
    let mut lines = vec![
        first.to_string(),
        counted(plan.entries.len(), "item", "items"),
    ];
    if plan.left_out() > 0 {
        lines.push(format!("Left out {}.", plan.left_out_text()));
    }
    if plan.other_names > 0 {
        lines.push(if plan.other_names == 1 {
            "1 item has another name that stays out of the archive, and goes in as its own copy."
                .to_string()
        } else {
            format!(
                "{} items have another name that stays out of the archive, and go in as their \
                 own copies.",
                plan.other_names
            )
        });
    }
    let attributes = plan.attributes_text();
    if policy.keep {
        lines.push(format!("Mac metadata carried: {attributes}."));
    } else {
        lines.push(format!("Mac metadata removed: {attributes}."));
        if plan.quarantined > 0 {
            lines.push(
                "A file that lost the download warning will not warn when it is extracted.".into(),
            );
        }
    }
    lines.join("\n")
}

fn details(policy: &Policy, plan: &Plan, leftovers: &Leftovers) {
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
            j::str(&etude_core::redact::path(policy.destination())),
        ),
        ("excluded", j::num(plan.left_out())),
        ("excluded_personal", j::num(plan.personal)),
        ("excluded_folders", j::num(plan.folders)),
        ("excluded_litter", j::num(plan.litter)),
        ("items_with_finder_tags", j::num(plan.tagged)),
        ("items_with_download_warning", j::num(plan.quarantined)),
        ("items_with_fork_or_finder_info", j::num(plan.attributed)),
        ("items_with_another_name", j::num(plan.other_names)),
        ("leftovers_removed", j::num(leftovers.removed)),
        ("leftovers_kept", j::num(leftovers.kept.len())),
        ("leftovers_still_open", j::num(leftovers.busy.len())),
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
    const VALUED: [&str; 5] = ["--for", "--into", "--format", "--metadata", "--max-size"];
    const OTHER_FLAGS: [&str; 3] = ["--json", "--list", "--"];
    let mut values: BTreeMap<&str, &str> = BTreeMap::new();
    let mut source: Option<&str> = None;
    let mut list = false;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "--json" => {}
            "--list" if !list => list = true,
            "--list" => {
                return Err(Problem::invalid(
                    "--list was given more than once. Remove the extra one.",
                ));
            }
            _ if VALUED.contains(&arg) => {
                i += 1;
                let hint = match arg {
                    "--for" => "--for self, --for mac or --for other",
                    "--into" => "--into FILE",
                    "--format" => "--format tar.gz or --format zip",
                    "--metadata" => "--metadata keep or --metadata drop",
                    _ => "--max-size 500M or --max-size 2G",
                };
                let given = args.get(i).map(String::as_str).unwrap_or("");
                if given.is_empty() || VALUED.contains(&given) || OTHER_FLAGS.contains(&given) {
                    return Err(Problem::invalid(format!(
                        "{arg} needs a value: {hint}. Nothing was written."
                    )));
                }
                if given.starts_with('-') {
                    return Err(Problem::invalid(format!(
                        "{arg} got \"{0}\", which reads as an option. Nothing was written. \
                         A name that starts with - needs a path in front, such as ./{0}.",
                        shown_text(given)
                    )));
                }
                let value = &args[i];
                if values.insert(arg, value.as_str()).is_some() {
                    return Err(Problem::invalid(format!(
                        "{arg} was given more than once, and only one can be used. Remove the \
                         ones you do not mean."
                    )));
                }
            }
            s if s.starts_with('-') => {
                return Err(Problem::new(
                    Code::UnknownOption,
                    2,
                    format!(
                        "unknown option {}. It handles --for, --into, --format, --metadata, \
                         --max-size, --list and --json. Run `pack help`.",
                        shown_text(s)
                    ),
                    None,
                ));
            }
            _ if source.is_none() => source = Some(arg),
            word => {
                return Err(Problem::invalid(format!(
                    "unexpected word \"{}\". Give one folder and any options. Run `pack help`.",
                    shown_text(word)
                )));
            }
        }
        i += 1;
    }
    let source = match source {
        None => {
            return Err(Problem::invalid(
                "give a folder to pack: pack FOLDER --for self|mac|other. Run `pack help`.",
            ));
        }
        Some("") => {
            return Err(Problem::invalid(
                "the folder name is empty. Give the folder to pack. Nothing was written.",
            ));
        }
        Some(source) => source,
    };
    let Some(recipient) = values.get("--for").copied() else {
        return Err(Problem::invalid(
            "say who the archive is for: --for self, --for mac or --for other. Pack does not \
             guess. Nothing was written.",
        ));
    };
    if !["self", "mac", "other"].contains(&recipient) {
        return Err(Problem::invalid(format!(
            "--for takes self, mac or other, got \"{}\". Nothing was written.",
            shown_text(recipient)
        )));
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
            format!(
                "pack makes tar.gz and zip, not \"{}\". Nothing was written. Use --format tar.gz \
                 or --format zip.",
                shown_text(format)
            ),
            None,
        ));
    }
    let metadata = *values
        .get("--metadata")
        .unwrap_or(&if recipient == "other" { "drop" } else { "keep" });
    if !["keep", "drop"].contains(&metadata) {
        return Err(Problem::invalid(format!(
            "--metadata takes keep or drop, got \"{}\". Nothing was written.",
            shown_text(metadata)
        )));
    }
    if format == "zip" && metadata == "keep" {
        return Err(Problem::invalid(
            "a zip from this writer cannot carry Mac metadata, so zip with keep cannot be done. \
             Nothing was written. Use --format tar.gz, or --metadata drop.",
        ));
    }
    let max = match values.get("--max-size") {
        None => DEFAULT_MAX,
        Some(value) => size(value).ok_or_else(|| {
            Problem::invalid(format!(
                "--max-size needs a whole number above 0, optionally with K, M or G, got \"{}\". \
                 Nothing was written.",
                shown_text(value)
            ))
        })?,
    };
    let into = values.get("--into").map(PathBuf::from);
    if let Some(into) = &into {
        let text = into.to_string_lossy();
        if text.ends_with('/') || into.file_name().is_none() {
            return Err(Problem::invalid(format!(
                "--into needs a file name, and {} names a folder. Nothing was written. Use \
                 --into FOLDER/NAME.{format}.",
                shown(into)
            )));
        }
        // unpack goes by the name, so a name that lies makes an archive it refuses.
        let lower = text.to_ascii_lowercase();
        let fits = match format {
            "zip" => lower.ends_with(".zip"),
            _ => lower.ends_with(".tar.gz") || lower.ends_with(".tgz"),
        };
        if !fits {
            return Err(Problem::invalid(format!(
                "{} does not end in .{format}, and this archive will be {format}. Nothing was \
                 written. Change the name, or choose the format with --format.",
                shown(into)
            )));
        }
    }
    Ok(Policy {
        source: PathBuf::from(source),
        typed: PathBuf::from(source),
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

fn refuse_root_user() -> Result<()> {
    if sys::running_as_root() {
        return Err(Problem::new(
            Code::UnsafeInput,
            2,
            "will not run as root. No archive was made. Run it as yourself.",
            None,
        ));
    }
    Ok(())
}

/// Turn what was typed into a real folder pack is willing to read.
fn resolve_source(policy: &mut Policy) -> Result<()> {
    let typed = policy.typed.clone();
    let text = typed.to_string_lossy();
    // A trailing slash would look through a link, and a link is refused.
    let trimmed = text.trim_end_matches('/');
    let bare = if trimmed.is_empty() {
        Path::new("/")
    } else {
        Path::new(trimmed)
    };
    let meta = match scan::observe_read("ancestor_metadata", fs::symlink_metadata(bare)) {
        Ok(meta) => meta,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return Err(Problem::new(
                Code::MissingInput,
                3,
                format!(
                    "no folder at {}. No archive was made. Check the name.",
                    policy.shown()
                ),
                Some(&typed),
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            return Err(policy.denied(&error));
        }
        Err(error) => return Err(policy.read_failed(error)),
    };
    if meta.file_type().is_symlink() {
        return Err(policy.refuse(
            &format!(
                "{} is a link, and pack does not follow links.",
                policy.shown()
            ),
            format!(
                "Give the folder it points to: readlink {}",
                quoted_path(bare)
            ),
        ));
    }
    if !meta.is_dir() {
        return Err(Problem::new(
            Code::MissingInput,
            3,
            format!(
                "{} is a file, and pack takes a folder. No archive was made. Put it in a folder \
                 and pack that.",
                policy.shown()
            ),
            Some(&typed),
        ));
    }
    policy.source = scan::observe_read("ancestor_metadata", fs::canonicalize(bare))
        .map_err(|e| policy.read_failed(e))?;
    let source = policy.source.clone();
    let protected = |what: &str| {
        policy.refuse(
            &format!(
                "{} is {what}, and pack will not put it in an archive.",
                policy.shown()
            ),
            "Choose a folder of your own.".into(),
        )
    };
    if source.parent().is_none() {
        return Err(protected("the whole disk"));
    }
    if scan::refused_location(&source).is_some() {
        return Err(protected("a system or credential folder"));
    }
    match home_relation(&source, &home_folders()) {
        Some(HomeRelation::Is) => return Err(protected("your home folder")),
        Some(HomeRelation::Holds) => return Err(protected("a folder holding your home folder")),
        None => {}
    }
    let name = root_name(policy)?;
    if hazard(name) {
        return Err(policy.refuse(
            &format!(
                "the name of {} is not safe to store in an archive for every system.",
                policy.shown()
            ),
            "Copy the folder to a plainer name and pack the copy.".into(),
        ));
    }
    if package(&source) {
        return Err(policy.refuse(
            &format!("{} is a package.", policy.shown()),
            "Pack the folder that holds it, so it goes in whole.".into(),
        ));
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
enum HomeRelation {
    Is,
    Holds,
}

/// The account's home from the system, and `HOME` too. `HOME` alone is not
/// enough, because a safety rule must not fail open when a variable is unset or
/// wrong. Sweep avoids it for the same reason; see `scan::refused_location`.
fn home_folders() -> Vec<PathBuf> {
    let variable = std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from);
    [sys::account_home(), variable]
        .into_iter()
        .flatten()
        .filter_map(|home| fs::canonicalize(home).ok())
        .collect()
}

fn home_relation(source: &Path, homes: &[PathBuf]) -> Option<HomeRelation> {
    if homes.iter().any(|home| home == source) {
        return Some(HomeRelation::Is);
    }
    homes
        .iter()
        .any(|home| home.starts_with(source))
        .then_some(HomeRelation::Holds)
}

fn root_name(policy: &Policy) -> Result<&str> {
    policy
        .source
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            policy.refuse(
                &format!("the name of {} cannot be read as text.", policy.shown()),
                "Copy the folder to a plainer name and pack the copy.".into(),
            )
        })
}

/// Fill in the destination's real folder and full name, and judge them.
fn resolve_destination(policy: &mut Policy) -> Result<()> {
    let into = match policy.into.take() {
        Some(into) => {
            let parent = into
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            let name = into.file_name().map(PathBuf::from).unwrap_or_default();
            let parent = match scan::observe_read(
                "destination_parent_metadata",
                fs::canonicalize(parent),
            ) {
                Ok(parent) if parent.is_dir() => parent,
                Ok(_) => return Err(not_a_folder(parent, &into)),
                Err(error) if error.kind() == io::ErrorKind::NotADirectory => {
                    return Err(not_a_folder(parent, &into));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    return Err(Problem::new(
                        Code::MissingInput,
                        3,
                        format!(
                            "the folder {} does not exist, so {} cannot be made there. No archive \
                             was made. Make the folder first: mkdir -p {}",
                            shown(parent),
                            shown(&into),
                            quoted_path(parent)
                        ),
                        Some(&into),
                    ));
                }
                Err(error) => {
                    policy.into = Some(into);
                    return Err(policy.write_failed(error));
                }
            };
            parent.join(name)
        }
        None => {
            // Beside the folder, however it was typed: `pack .` and `pack a/`
            // would otherwise aim inside the folder being packed.
            let name = policy.source.file_name().unwrap_or_default();
            let mut file = name.to_os_string();
            file.push(format!(".{}", policy.format));
            policy.source.with_file_name(file)
        }
    };
    if into.starts_with(&policy.source) {
        return Err(Problem::invalid(format!(
            "{} is inside the folder being packed, so the archive would contain itself. No \
             archive was made. Choose a place outside {} with --into FILE.",
            shown(&into),
            policy.shown()
        )));
    }
    policy.into = Some(into);
    Ok(())
}

fn not_a_folder(parent: &Path, into: &Path) -> Problem {
    Problem::new(
        Code::MissingInput,
        3,
        format!(
            "part of {} is a file, not a folder, so {} cannot be made there. No archive was made. \
             Choose another place with --into FILE.",
            shown(parent),
            shown(into)
        ),
        Some(into),
    )
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
    /// Names that look personal or like credentials.
    personal: usize,
    /// Folders such as .ssh, .git and node_modules.
    folders: usize,
    /// .DS_Store, stored ._ files and the like.
    litter: usize,
    tagged: usize,
    quarantined: usize,
    attributed: usize,
    /// Files with a hard-link name that stays out of the archive.
    other_names: usize,
    payload: u64,
    bound: u64,
}
impl Plan {
    fn left_out(&self) -> usize {
        self.personal + self.folders + self.litter
    }
    fn left_out_text(&self) -> String {
        let mut parts = Vec::new();
        if self.personal > 0 {
            parts.push(format!(
                "{} that {} personal",
                counted(self.personal, "item", "items"),
                if self.personal == 1 { "looks" } else { "look" }
            ));
        }
        if self.folders > 0 {
            parts.push(format!(
                "{} of credentials or tool data",
                counted(self.folders, "folder", "folders")
            ));
        }
        if self.litter > 0 {
            parts.push(counted(self.litter, "Finder leftover", "Finder leftovers"));
        }
        parts.join(", ")
    }
    fn attributes_text(&self) -> String {
        let mut parts = Vec::new();
        if self.tagged > 0 {
            parts.push(format!(
                "Finder tags on {}",
                counted(self.tagged, "item", "items")
            ));
        }
        if self.quarantined > 0 {
            parts.push(format!(
                "the download warning on {}",
                counted(self.quarantined, "item", "items")
            ));
        }
        if self.attributed > 0 {
            parts.push(format!(
                "resource forks or Finder info on {}",
                counted(self.attributed, "item", "items")
            ));
        }
        parts.push("any other attributes".to_string());
        parts.join(", ")
    }
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
/// Only call this for a folder. The last test runs a system tool.
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
/// What `unpack` throws away as junk, plus the stored `._` shadow files.
fn litter(name: &str, directory: bool, keep: bool) -> bool {
    if name.starts_with("._") {
        return true;
    }
    !keep && (name == ".DS_Store" || name == "Thumbs.db" || (directory && name == "__MACOSX"))
}
fn preflight(policy: &Policy) -> Result<Plan> {
    let mut walker = Walker {
        policy,
        root: root_name(policy)?.to_owned(),
        plan: Plan {
            entries: vec![],
            identities: vec![],
            personal: 0,
            folders: 0,
            litter: 0,
            tagged: 0,
            quarantined: 0,
            attributed: 0,
            other_names: 0,
            payload: 0,
            bound: 65536,
        },
        keys: BTreeSet::new(),
        links: BTreeSet::new(),
    };
    walker.walk(&policy.source, false, 0)?;
    let plan = walker.plan;
    if plan.bound > policy.max {
        return Err(over_limit(policy, plan.bound));
    }
    Ok(plan)
}
struct Walker<'a> {
    policy: &'a Policy,
    root: String,
    plan: Plan,
    keys: BTreeSet<String>,
    links: BTreeSet<(u64, u64)>,
}
impl Walker<'_> {
    fn walk(&mut self, path: &Path, in_package: bool, depth: usize) -> Result<()> {
        let policy = self.policy;
        check_stop(policy)?;
        if depth >= MAX_DEPTH {
            return Err(policy.refuse(
                &format!("{} holds an item more than {} levels down, and unpack refuses anything deeper.",
                    policy.shown(),
                    MAX_DEPTH - 1),
                "Move the deepest folders up, or pack a folder further down.".into(),
            ));
        }
        if self.plan.identities.len() >= MAX_ENTRIES {
            return Err(policy.refuse(
                &format!(
                    "{} holds more than {MAX_ENTRIES} items, and pack stops there.",
                    policy.shown()
                ),
                "Pack its subfolders one at a time.".into(),
            ));
        }
        let m = scan::observe_read("selected_tree_metadata", fs::symlink_metadata(path))
            .map_err(|e| policy.read_failed(e))?;
        let top = path == policy.source;
        let Some(name) = path.file_name().unwrap_or_default().to_str() else {
            return Err(policy.refuse(
                &format!("a name in {} cannot be read as text.", policy.shown()),
                "Rename it, then run pack again.".into(),
            ));
        };
        if scan::IN_FLIGHT_SUFFIXES
            .iter()
            .any(|suffix| name.to_ascii_lowercase().ends_with(suffix))
        {
            // A download with a private-looking name is refused without its name.
            let private = sensitive(path, &m);
            let named = if private {
                "A download with a private-looking name".to_string()
            } else {
                format!("\"{}\"", shown_text(name))
            };
            return Err(Problem::new(
                Code::DownloadInFlight,
                2,
                format!(
                    "{named} in {} is still downloading. No archive was made. Run pack again when \
                     it has finished, or move it out.",
                    policy.shown()
                ),
                (!private).then_some(path),
            ));
        }
        if m.file_type().is_symlink() {
            return Err(policy.refuse(
                &format!(
                    "{} holds a link, and pack does not follow links or store them.",
                    policy.shown()
                ),
                format!("To find it: find {} -type l", policy.quoted()),
            ));
        }
        if !(m.is_dir() || m.is_file()) {
            return Err(policy.refuse(
                &format!(
                    "{} holds a pipe, socket or device, and an archive cannot safely hold one.",
                    policy.shown()
                ),
                format!("To find it: find {} ! -type f ! -type d", policy.quoted()),
            ));
        }
        self.plan.identities.push((path.into(), Identity::of(&m)));
        let in_package = in_package || (!top && m.is_dir() && package(path));
        if !top {
            if sensitive(path, &m) || (m.is_dir() && scan::never_enter(name)) {
                if in_package {
                    return Err(policy.refuse(
                        &format!(
                            "a package in {} holds a part pack leaves out, and a package goes in \
                             whole or not at all.",
                            policy.shown()
                        ),
                        "Move the private part out of the package, or move the package out of the folder."
                            .into(),
                    ));
                }
                if m.is_dir() && scan::never_enter(name) {
                    self.plan.folders += 1;
                } else {
                    self.plan.personal += 1;
                }
                return Ok(());
            }
            if litter(name, m.is_dir(), policy.keep) {
                self.plan.litter += 1;
                return Ok(());
            }
            self.include(path, &m)?;
        }
        if m.is_dir() {
            let listed = scan::observe_read("selected_tree_metadata", fs::read_dir(path))
                .map_err(|e| policy.read_failed(e))?;
            let mut children = listed
                .map(|e| e.map(|e| e.path()))
                .collect::<io::Result<Vec<_>>>()
                .map_err(|e| policy.read_failed(e))?;
            children.sort();
            for child in children {
                self.walk(&child, in_package, depth + 1)?;
            }
        }
        Ok(())
    }

    /// One item that goes in the archive: judge it the way unpack will.
    fn include(&mut self, path: &Path, m: &Metadata) -> Result<()> {
        let policy = self.policy;
        let inside = |what: &str, step: String| {
            policy.refuse(&format!("{} holds {what}", policy.shown()), step)
        };
        let relative = path
            .strip_prefix(&policy.source)
            .map_err(|_| policy.changed())?;
        if relative
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(policy.changed());
        }
        let Some(relative_text) = relative.to_str() else {
            return Err(inside(
                "a name that cannot be read as text.",
                "Rename it, then run pack again.".into(),
            ));
        };
        let name = format!("{}/{relative_text}", self.root);
        if hazard(&name) {
            return Err(inside(
                "a name that is not safe to store for every system: it has one of \\ : < > \" | ? * or a control character, ends in a dot or a space, starts with -, or is a reserved name such as CON or NUL.",
                "Rename it, then run pack again.".into(),
            ));
        }
        let key = etude_core::apply::dedupe_key(relative, true).map_err(|_| {
            inside(
                "a name that cannot be read as text.",
                "Rename it, then run pack again.".into(),
            )
        })?;
        if !self.keys.insert(key) {
            return Err(inside(
                "two names that differ only by case or accent, and extracting one would overwrite the other.",
                "Rename one of them, then run pack again.".into(),
            ));
        }
        if m.mode() & 0o6000 != 0 {
            return Err(inside(
                "a setuid or setgid file or folder, and unpack refuses to extract one.",
                format!(
                    "To find it: find {} -mindepth 1 \\( -type f -o -type d \\) \\( -perm -4000 -o -perm -2000 \\)",
                    policy.quoted()
                ),
            ));
        }
        if m.is_file() {
            if m.nlink() > 1 && !self.links.insert((m.dev(), m.ino())) {
                return Err(inside(
                    "two names for one file, known as hard links, and unpack refuses to extract them.",
                    format!(
                        "To see files with more than one name: find {} -type f -links +1. Make one a separate copy, then run pack again.",
                        policy.quoted()
                    ),
                ));
            }
            if m.nlink() > 1 {
                self.plan.other_names += 1;
            }
            if sys::dataless(m) {
                return Err(inside(
                    "a file stored only in the cloud, and reading it would download it. Pack uses no network.",
                    "Download the folder in Finder, then run pack again.".into(),
                ));
            }
            // Asked now, so a file that cannot be read is named as the cause.
            scan::observe_read(
                "selected_tree_metadata",
                OpenOptions::new()
                    .read(true)
                    .custom_flags(sys::OPEN_FLAGS)
                    .open(path),
            )
            .map_err(|e| policy.read_failed(e))?;
        }
        let attributes = sys::attributes(path).map_err(|e| policy.read_failed(e))?;
        self.plan.tagged += usize::from(attributes.tagged);
        self.plan.quarantined += usize::from(attributes.quarantined);
        self.plan.attributed += usize::from(attributes.other);
        // Deflate grows data it cannot shrink by well under 1 in 256, and the
        // per-member allowance covers headers and metadata encoding. A bound of
        // twice the payload refused folders whose archive could never reach the
        // limit; the check while writing still stops one that does.
        let metadata = if policy.keep { attributes.bytes } else { 0 };
        let overflow = || over_limit(policy, u64::MAX);
        self.plan.bound = self
            .plan
            .bound
            .checked_add(m.len())
            .and_then(|n| n.checked_add(m.len() / 256))
            .and_then(|n| n.checked_add(metadata.saturating_mul(4)))
            .and_then(|n| n.checked_add(65536))
            .ok_or_else(overflow)?;
        if m.is_file() {
            self.plan.payload = self.plan.payload.saturating_add(m.len());
        }
        self.plan.entries.push(Member {
            name,
            identity: Identity::of(m),
            directory: m.is_dir(),
        });
        Ok(())
    }
}

/// None when the disk's free space cannot be read. The writer's own failure on
/// a full disk still stops the run, so this is not a safety check by itself.
fn free_bytes(path: &Path) -> Option<u64> {
    let out = Command::new("/bin/df")
        .args(["-Pk"])
        .arg(path)
        .stdin(Stdio::null())
        .output();
    let out = scan::observe_read("volume_free_space", out).ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .nth(1)
        .and_then(|s| s.split_whitespace().nth(3))
        .and_then(|s| s.parse::<u64>().ok())
        .and_then(|n| n.checked_mul(1024))
}

/// The unfinished file: a private sibling of the destination.
///
/// Three things make a later run able to prove a file is its own, because a
/// kill leaves no code running to remove it: an exact name holding a random
/// nonce, an extended attribute holding the same nonce, and an exclusive lock
/// on the open file. The writer's stdout shares that open file, so the lock
/// lasts until the last process that could still write it has gone.
struct Staging {
    path: PathBuf,
    file: File,
    published: bool,
}
const STAGING_SUFFIX: &str = ".partial";
/// Keeps the name within the 255-byte limit whatever the destination is called.
fn staging_prefix(dest: &Path) -> String {
    let name = dest.file_name().unwrap_or_default().to_string_lossy();
    let mut end = name.len().min(160);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    format!(".{}.pack-", &name[..end])
}
enum Cleanup {
    Done,
    Remains(String),
}
impl Staging {
    fn create(dest: &Path) -> io::Result<Self> {
        for _ in 0..128 {
            let mut nonce = [0u8; 16];
            scan::observe_read(
                "os_random_bytes",
                File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut nonce)),
            )?;
            let nonce = nonce.iter().map(|b| format!("{b:02x}")).collect::<String>();
            let path =
                dest.with_file_name(format!("{}{nonce}{STAGING_SUFFIX}", staging_prefix(dest)));
            let open = |flags: i32| {
                OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .custom_flags(flags)
                    .open(&path)
            };
            let opened = match open(sys::LOCKED) {
                // A disk without locks: nothing can prove this file later, so a
                // kill leaves it for the person. The run itself still works.
                Err(e) if sys::no_support(&e) => open(0),
                other => other,
            };
            match opened {
                Ok(file) => {
                    // Not fatal: without the mark a later run leaves the file alone.
                    sys::set_mark(&file, &nonce);
                    return Ok(Self {
                        path,
                        file,
                        published: false,
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::other("could not choose a private file name"))
    }
    fn still_ours(&self) -> bool {
        let (Ok(on_disk), Ok(held)) = (fs::symlink_metadata(&self.path), self.file.metadata())
        else {
            return false;
        };
        on_disk.is_file() && on_disk.dev() == held.dev() && on_disk.ino() == held.ino()
    }
    /// Remove the file only if the name still means this file. Anything else at
    /// that name belongs to someone else.
    fn remove(&mut self) -> Cleanup {
        if self.published {
            return Cleanup::Done;
        }
        self.published = true;
        let shown_path = shown(&self.path);
        match fs::symlink_metadata(&self.path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Cleanup::Done,
            _ => {}
        }
        if !self.still_ours() {
            return Cleanup::Remains(format!(
                "Something else is now at {shown_path}, so pack left it alone."
            ));
        }
        match fs::remove_file(&self.path) {
            Ok(()) => Cleanup::Done,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Cleanup::Done,
            Err(error) => Cleanup::Remains(format!(
                "The unfinished file could not be removed: {}. You can delete it: rm {}",
                plain_io_error(&error),
                quoted_path(&self.path)
            )),
        }
    }
}
impl Drop for Staging {
    fn drop(&mut self) {
        if let Cleanup::Remains(step) = self.remove() {
            eprintln!("pack: {step}");
        }
    }
}

#[derive(Default)]
struct Leftovers {
    removed: usize,
    /// Proven with --list, which removes nothing.
    removable: usize,
    /// Held open by another process. Never removed.
    busy: Vec<PathBuf>,
    /// Files with the right name that could not be proven to be pack's own.
    kept: Vec<PathBuf>,
}
impl Leftovers {
    fn report(&self, destination: &Path) {
        if self.removed > 0 {
            eprintln!(
                "pack: removed {} from an earlier run for {} that was stopped.",
                counted(self.removed, "unfinished file", "unfinished files"),
                shown(destination)
            );
        }
        envelope::effect("leftovers_removed", self.removed);
        envelope::effect("leftovers_still_open", self.busy.len());
        envelope::effect("leftovers_kept", self.kept.len());
        if self.removable > 0 {
            eprintln!(
                "pack: {} from an earlier stopped run for {} can be removed. --list changes \
                 nothing; a run without it removes {}.",
                counted(self.removable, "unfinished file", "unfinished files"),
                shown(destination),
                if self.removable == 1 { "it" } else { "them" },
            );
        }
        if !self.busy.is_empty() {
            let names = self
                .busy
                .iter()
                .map(|path| shown(path))
                .collect::<Vec<_>>()
                .join(", ");
            let it = if self.busy.len() == 1 { "it" } else { "them" };
            eprintln!(
                "pack: left {names} alone, because another process still has {it} open: another \
                 pack, or the tar of a pack that was killed. A later run removes {it} once that \
                 process has ended and nothing else removed {it} first."
            );
        }
        if !self.kept.is_empty() {
            let names = self
                .kept
                .iter()
                .map(|path| shown(path))
                .collect::<Vec<_>>()
                .join(", ");
            eprintln!(
                "pack: left {} alone, because pack cannot prove {} its own: {names}. If you do \
                 not want {}, delete {} yourself.",
                counted(
                    self.kept.len(),
                    "file with an unfinished-archive name",
                    "files with an unfinished-archive name"
                ),
                if self.kept.len() == 1 {
                    "it is"
                } else {
                    "they are"
                },
                if self.kept.len() == 1 { "it" } else { "them" },
                if self.kept.len() == 1 { "it" } else { "them" },
            );
        }
    }
}

/// Remove what a killed run left for this destination, and nothing else.
fn sweep_leftovers(destination: &Path, remove: bool) -> Leftovers {
    let mut found = Leftovers::default();
    let prefix = staging_prefix(destination);
    let Some(parent) = destination.parent() else {
        return found;
    };
    let Ok(listed) = scan::observe_read("destination_parent_metadata", fs::read_dir(parent)) else {
        return found;
    };
    for entry in listed.flatten() {
        let name = entry.file_name();
        let Some(nonce) = name
            .to_str()
            .and_then(|name| name.strip_prefix(prefix.as_str()))
            .and_then(|rest| rest.strip_suffix(STAGING_SUFFIX))
            .filter(|nonce| {
                nonce.len() == 32
                    && nonce
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
        else {
            continue;
        };
        let path = entry.path();
        match remove_if_ours(&path, nonce, remove) {
            Leftover::Removed => found.removed += 1,
            Leftover::Removable => found.removable += 1,
            Leftover::Busy => found.busy.push(path),
            Leftover::NotProven => found.kept.push(path),
        }
    }
    found
}

enum Leftover {
    Removed,
    Removable,
    Busy,
    NotProven,
}

/// Every test must pass before anything is removed, and each one looks at the
/// file itself through a descriptor opened without following links.
fn remove_if_ours(path: &Path, nonce: &str, remove: bool) -> Leftover {
    let Ok(file) = OpenOptions::new()
        .read(true)
        .custom_flags(sys::OPEN_FLAGS)
        .open(path)
    else {
        return Leftover::NotProven;
    };
    let Ok(held) = file.metadata() else {
        return Leftover::NotProven;
    };
    let same_name = || {
        fs::symlink_metadata(path).is_ok_and(|on_disk| {
            on_disk.is_file() && on_disk.dev() == held.dev() && on_disk.ino() == held.ino()
        })
    };
    if !held.is_file()
        || held.nlink() != 1
        || held.mode() & 0o7777 != 0o600
        || held.uid() != sys::user_id()
        || !sys::has_mark(&file, nonce)
        || !same_name()
    {
        return Leftover::NotProven;
    }
    match sys::try_lock(&file) {
        Ok(true) => {}
        Ok(false) => return Leftover::Busy,
        Err(_) => return Leftover::NotProven,
    }
    if !remove {
        return Leftover::Removable;
    }
    if !same_name() || fs::remove_file(path).is_err() {
        return Leftover::NotProven;
    }
    Leftover::Removed
}

fn writer(policy: &Policy, staging: &Staging) -> io::Result<Command> {
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
    let parent = policy.source.parent().unwrap_or(Path::new("/"));
    cmd.args(["-f", "-"])
        .arg("-C")
        .arg(parent)
        .args(["-n", "--null", "-T", "-"])
        .env("LC_ALL", "en_US.UTF-8");
    cmd.stdin(Stdio::piped())
        .stdout(staging.file.try_clone()?)
        .stderr(Stdio::piped());
    Ok(cmd)
}

/// Wait for a system tool, stopping it if a signal arrives. Returns its exit
/// status and what its stdout held: up to `keep` bytes, and the total count.
fn watch(mut child: Child, keep: u64, policy: &Policy) -> Result<(ExitStatus, Vec<u8>, u64)> {
    let stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut kept = Vec::new();
        let mut total = 0u64;
        if let Some(mut stdout) = stdout {
            let mut buffer = [0u8; 65536];
            while let Ok(n) = stdout.read(&mut buffer) {
                if n == 0 {
                    break;
                }
                total += n as u64;
                if (kept.len() as u64) <= keep {
                    kept.extend_from_slice(&buffer[..n]);
                }
            }
        }
        (kept, total)
    });
    let status = loop {
        if STOP.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Err(stopped(policy));
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(system_tool_failed(policy, &error));
            }
        }
    };
    let (kept, total) = reader.join().unwrap_or_default();
    Ok((status, kept, total))
}

fn failed_check(policy: &Policy) -> Problem {
    let destination = policy.destination();
    Problem::new(
        Code::ArchiveUnreadable,
        3,
        format!(
            "the archive for {} did not match the folder when pack read it back. No archive was \
             made. Run pack again; if it fails again, check the disk.",
            shown(destination)
        ),
        Some(destination),
    )
}

fn readback(policy: &Policy, path: &Path, plan: &Plan) -> Result<()> {
    let keep = policy.keep;
    let tool = || {
        let mut cmd = Command::new(TAR);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .env("LC_ALL", "en_US.UTF-8");
        cmd
    };
    let child = tool()
        .args(["-tvf"])
        .arg(path)
        .spawn()
        .map_err(|e| system_tool_failed(policy, &e))?;
    let limit = (plan.entries.len() as u64 + 1) * 4096 * 2;
    let (status, data, total) = watch(child, limit, policy)?;
    scan::record_read_outcome("archive_readback_bytes", status.success());
    if !status.success() || total > limit {
        return Err(failed_check(policy));
    }
    let text = std::str::from_utf8(&data).map_err(|_| failed_check(policy))?;
    let mut actual = BTreeMap::new();
    for line in text.lines() {
        let (kind, name) = typed_member(line).ok_or_else(|| failed_check(policy))?;
        if kind != b'-' && kind != b'd' {
            return Err(failed_check(policy));
        }
        let name = comparison_name(name).ok_or_else(|| failed_check(policy))?;
        if actual.insert(name, kind == b'd').is_some() {
            return Err(failed_check(policy));
        }
    }
    let expected = plan
        .entries
        .iter()
        .map(|m| comparison_name(&m.name).map(|name| (name, m.directory)))
        .collect::<Option<BTreeMap<_, _>>>()
        .ok_or_else(|| failed_check(policy))?;
    if keep {
        // AppleDouble companions are generated by the system writer. The stored
        // kind never left the source, because pack leaves out `._` names.
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
        return Err(failed_check(policy));
    }
    // Stream every payload through the system reader and count the bytes.
    let child = tool()
        .args(["-xOf"])
        .arg(path)
        .spawn()
        .map_err(|e| system_tool_failed(policy, &e))?;
    let (status, _, total) = watch(child, 0, policy)?;
    let enough = if keep {
        total >= plan.payload
    } else {
        total == plan.payload
    };
    if !status.success() || !enough {
        return Err(failed_check(policy));
    }
    scan::verify_read("archive_readback_bytes");
    Ok(())
}
fn comparison_name(name: &str) -> Option<String> {
    etude_core::apply::dedupe_key(Path::new(name), false).ok()
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

/// The operating system calls with no safe wrapper in the standard library.
#[cfg(target_os = "macos")]
mod sys {
    use std::ffi::{CStr, CString};
    use std::fs::{File, Metadata};
    use std::io;
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Path, PathBuf};

    /// sys/fcntl.h: O_NONBLOCK | O_NOFOLLOW, so opening never waits on a pipe
    /// and never looks through a link.
    pub const OPEN_FLAGS: i32 = 0x0004 | 0x0100;
    /// sys/fcntl.h: O_EXLOCK, taken in the same step that creates the file.
    pub const LOCKED: i32 = 0x0020;
    /// sys/xattr.h: XATTR_NOFOLLOW.
    const NOFOLLOW: i32 = 1;
    const MARK: &[u8] = b"org.etudes.pack.staging\0";
    const LOCK_EX: i32 = 2;
    const LOCK_NB: i32 = 4;
    /// sys/stat.h: SF_DATALESS, a file whose bytes are not on this machine.
    const SF_DATALESS: u32 = 0x4000_0000;

    unsafe extern "C" {
        fn geteuid() -> u32;
        fn flock(fd: i32, operation: i32) -> i32;
        fn fsetxattr(
            fd: i32,
            name: *const u8,
            value: *const u8,
            size: usize,
            position: u32,
            options: i32,
        ) -> i32;
        fn fgetxattr(
            fd: i32,
            name: *const u8,
            value: *mut u8,
            size: usize,
            position: u32,
            options: i32,
        ) -> isize;
        fn fremovexattr(fd: i32, name: *const u8, options: i32) -> i32;
        fn getxattr(
            path: *const i8,
            name: *const i8,
            value: *mut u8,
            size: usize,
            position: u32,
            options: i32,
        ) -> isize;
    }

    /// pwd.h, through the one field read here.
    #[repr(C)]
    struct Passwd {
        name: *const i8,
        password: *const i8,
        uid: u32,
        gid: u32,
        change: i64,
        class: *const i8,
        gecos: *const i8,
        dir: *const i8,
    }
    unsafe extern "C" {
        fn getpwuid(uid: u32) -> *const Passwd;
    }

    /// This account's home folder from the system's records, not the environment.
    pub fn account_home() -> Option<PathBuf> {
        // SAFETY: the record is read and copied at once, and this runs before the
        // tool starts any thread, so nothing else can overwrite the shared result.
        unsafe {
            let record = getpwuid(user_id());
            if record.is_null() || (*record).dir.is_null() {
                return None;
            }
            let dir = CStr::from_ptr((*record).dir);
            Some(PathBuf::from(std::ffi::OsStr::from_bytes(dir.to_bytes())))
        }
    }

    pub fn running_as_root() -> bool {
        user_id() == 0
    }
    pub fn user_id() -> u32 {
        // SAFETY: geteuid reads a process property and takes no pointers.
        unsafe { geteuid() }
    }
    pub fn no_support(error: &io::Error) -> bool {
        matches!(error.raw_os_error(), Some(45 | 102))
    }
    pub fn dataless(metadata: &Metadata) -> bool {
        use std::os::macos::fs::MetadataExt;
        metadata.st_flags() & SF_DATALESS != 0
    }
    pub fn set_mark(file: &File, nonce: &str) {
        // SAFETY: both pointers are live for the call, and the sizes match.
        unsafe {
            fsetxattr(
                file.as_raw_fd(),
                MARK.as_ptr(),
                nonce.as_ptr(),
                nonce.len(),
                0,
                0,
            );
        }
    }
    pub fn has_mark(file: &File, nonce: &str) -> bool {
        let mut value = [0u8; 64];
        // SAFETY: the buffer is live for the call and its size is passed.
        let got = unsafe {
            fgetxattr(
                file.as_raw_fd(),
                MARK.as_ptr(),
                value.as_mut_ptr(),
                value.len(),
                0,
                0,
            )
        };
        got == nonce.len() as isize && &value[..nonce.len()] == nonce.as_bytes()
    }
    /// The finished archive does not carry the mark. A failure is harmless,
    /// because the mark only counts together with the unfinished file's name.
    pub fn clear_mark(file: &File) {
        // SAFETY: the name is NUL terminated and lives for the call.
        unsafe {
            fremovexattr(file.as_raw_fd(), MARK.as_ptr(), 0);
        }
    }
    /// True when this process now holds the only lock on the open file.
    pub fn try_lock(file: &File) -> io::Result<bool> {
        // SAFETY: flock takes only integers.
        if unsafe { flock(file.as_raw_fd(), LOCK_EX | LOCK_NB) } == 0 {
            return Ok(true);
        }
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::WouldBlock {
            Ok(false)
        } else {
            Err(error)
        }
    }

    #[derive(Default)]
    pub struct Attributes {
        pub tagged: bool,
        pub quarantined: bool,
        pub other: bool,
        pub bytes: u64,
    }
    /// Lengths of the attributes this tool names, and no others. It never lists
    /// names, because that would look at Finder comments, and values stay out of
    /// this process.
    pub fn attributes(path: &Path) -> io::Result<Attributes> {
        let path = CString::new(path.as_os_str().as_bytes())?;
        let length = |name: &[u8]| -> io::Result<Option<u64>> {
            // SAFETY: both strings are NUL terminated; a null buffer asks for length.
            let len = unsafe {
                getxattr(
                    path.as_ptr(),
                    name.as_ptr().cast(),
                    std::ptr::null_mut(),
                    0,
                    0,
                    NOFOLLOW,
                )
            };
            if len >= 0 {
                return Ok(Some(len as u64));
            }
            let error = io::Error::last_os_error();
            match error.raw_os_error() {
                // ENOATTR, or ENOTSUP on a disk with no attributes to lose.
                Some(93 | 45 | 102) => Ok(None),
                _ => Err(error),
            }
        };
        let tags = length(b"com.apple.metadata:_kMDItemUserTags\0")?;
        let warning = length(b"com.apple.quarantine\0")?;
        let fork = length(b"com.apple.ResourceFork\0")?;
        let info = length(b"com.apple.FinderInfo\0")?;
        let bytes = [tags, warning, fork, info].iter().flatten().sum();
        Ok(Attributes {
            tagged: tags.is_some(),
            quarantined: warning.is_some(),
            other: fork.is_some() || info.is_some(),
            bytes,
        })
    }
}

/// Pack is macOS only. These keep the crate buildable elsewhere, refusing to act.
#[cfg(not(target_os = "macos"))]
mod sys {
    use std::fs::{File, Metadata};
    use std::io;
    use std::path::{Path, PathBuf};
    pub const OPEN_FLAGS: i32 = 0;
    pub fn account_home() -> Option<PathBuf> {
        None
    }
    pub const LOCKED: i32 = 0;
    pub fn running_as_root() -> bool {
        false
    }
    pub fn user_id() -> u32 {
        u32::MAX
    }
    pub fn no_support(_: &io::Error) -> bool {
        false
    }
    pub fn dataless(_: &Metadata) -> bool {
        false
    }
    pub fn set_mark(_: &File, _: &str) {}
    pub fn has_mark(_: &File, _: &str) -> bool {
        false
    }
    pub fn clear_mark(_: &File) {}
    pub fn try_lock(_: &File) -> io::Result<bool> {
        Ok(false)
    }
    #[derive(Default)]
    pub struct Attributes {
        pub tagged: bool,
        pub quarantined: bool,
        pub other: bool,
        pub bytes: u64,
    }
    pub fn attributes(_: &Path) -> io::Result<Attributes> {
        Err(io::ErrorKind::Unsupported.into())
    }
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
            let mut policy = parse(&[
                self.0.join("source").to_str().unwrap().into(),
                "--for".into(),
                "other".into(),
                "--into".into(),
                self.0.join("out.zip").to_str().unwrap().into(),
            ])
            .unwrap();
            resolve_source(&mut policy).unwrap();
            resolve_destination(&mut policy).unwrap();
            policy
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
        assert_eq!(manifest.personal, names.len());
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
                error.path.as_deref().and_then(Path::file_name),
                Some(std::ffi::OsStr::new(&name))
            );
            fs::remove_file(f.0.join("source").join(&name)).unwrap();
        }
    }
    #[test]
    fn recent_file() {
        let f = Fixture::new();
        f.write("just-created.txt");
        let plan = preflight(&f.policy()).unwrap();
        assert_eq!(plan.entries[0].name, "source/just-created.txt");
    }
    #[test]
    fn opaque_package() {
        let f = Fixture::new();
        fs::create_dir(f.0.join("source/Example.app")).unwrap();
        fs::write(f.0.join("source/Example.app/one"), b"one").unwrap();
        let policy = f.policy();
        assert_eq!(preflight(&policy).unwrap().entries.len(), 2);
        let mut selected_package = policy.clone();
        selected_package.typed = policy.source.join("Example.app");
        assert_eq!(
            resolve_source(&mut selected_package).unwrap_err().code,
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
        assert_eq!(plan.left_out(), 2);
        assert!(plan.entries.iter().any(|entry| entry.name == "source/.git"));
        policy.max = 1;
        assert_eq!(
            preflight(&policy).unwrap_err().code,
            Code::SizeLimitExceeded
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
        let mut staging = Staging::create(&f.0.join("out")).unwrap();
        let path = staging.path.clone();
        fs::rename(&path, f.0.join("moved")).unwrap();
        fs::write(&path, b"replacement belongs to another writer").unwrap();
        let cleanup = staging.remove();
        assert!(matches!(cleanup, Cleanup::Remains(step) if step.contains("left it alone")));
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
        let error = verify_published(&output, || Err(Problem::invalid("fails"))).unwrap_err();
        assert_eq!(error.code, Code::OperationIncomplete);
        assert_eq!(error.exit, 3);
        assert_eq!(fs::read(&output).unwrap(), b"published bytes");
    }

    fn nonce_of(path: &Path) -> String {
        let name = path.file_name().unwrap().to_str().unwrap();
        name.rsplit_once(".pack-")
            .unwrap()
            .1
            .trim_end_matches(STAGING_SUFFIX)
            .to_owned()
    }

    /// A staging file left as a killed run leaves it: closed, marked, unlocked.
    fn abandoned(dest: &Path) -> PathBuf {
        let mut staging = Staging::create(dest).unwrap();
        staging.published = true;
        fs::write(&staging.path, b"partial").unwrap();
        staging.path.clone()
    }

    #[test]
    fn a_later_run_removes_what_a_killed_run_left() {
        let f = Fixture::new();
        let dest = f.0.join("out.zip");
        let left = abandoned(&dest);
        // Another test's child holds every open file between fork and exec, so
        // the lock can look busy for a moment. A busy file is left, as it should be.
        let mut found = sweep_leftovers(&dest, true);
        for _ in 0..200 {
            if found.removed > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
            found = sweep_leftovers(&dest, true);
        }
        assert_eq!((found.removed, found.kept.len()), (1, 0));
        assert!(!left.exists());
    }

    #[test]
    fn a_leftover_being_written_stays() {
        let f = Fixture::new();
        let dest = f.0.join("out.zip");
        let live = Staging::create(&dest).unwrap();
        let found = sweep_leftovers(&dest, true);
        assert_eq!((found.removed, found.kept.len()), (0, 0));
        assert_eq!(found.busy, vec![live.path.clone()]);
        assert!(live.path.exists());
    }

    #[test]
    fn nothing_that_only_looks_like_a_leftover_is_removed() {
        let f = Fixture::new();
        let dest = f.0.join("out.zip");
        let name = |tail: &str| {
            f.0.join(format!("{}{tail}{STAGING_SUFFIX}", staging_prefix(&dest)))
        };
        let hex = "a".repeat(32);
        // The right name, never marked: a person's own file.
        let unmarked = name(&hex);
        fs::write(&unmarked, b"mine").unwrap();
        fs::set_permissions(
            &unmarked,
            std::os::unix::fs::PermissionsExt::from_mode(0o600),
        )
        .unwrap();
        // A link at the right name, to a file that must survive.
        let outside = f.0.join("outside");
        fs::write(&outside, b"outside").unwrap();
        let link = name(&"b".repeat(32));
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        // A folder at the right name.
        let folder = name(&"c".repeat(32));
        fs::create_dir(&folder).unwrap();
        // A real leftover under a name whose nonce does not match its mark.
        let copied = name(&"d".repeat(32));
        fs::rename(abandoned(&dest), &copied).unwrap();
        // A real leftover with a second name, which someone else may rely on.
        let linked = abandoned(&dest);
        fs::hard_link(&linked, f.0.join("second-name")).unwrap();
        // A real leftover made readable by others: no longer private.
        let opened_up = abandoned(&dest);
        fs::set_permissions(
            &opened_up,
            std::os::unix::fs::PermissionsExt::from_mode(0o644),
        )
        .unwrap();
        // The wrong shape of name, and another destination's leftover.
        let short = name("abc");
        fs::write(&short, b"mine").unwrap();
        let other = abandoned(&f.0.join("other.zip"));
        let found = sweep_leftovers(&dest, true);
        assert_eq!(found.removed, 0);
        assert_eq!(found.kept.len(), 6);
        for path in [
            &unmarked, &link, &folder, &copied, &linked, &opened_up, &short, &other, &outside,
        ] {
            assert!(fs::symlink_metadata(path).is_ok(), "removed {path:?}");
        }
        assert_eq!(nonce_of(&unmarked), hex);
    }

    #[test]
    fn a_stopped_run_is_a_refusal_that_says_so() {
        let f = Fixture::new();
        let error = stopped(&f.policy());
        assert_eq!(error.code, Code::ExtractionStopped);
        assert_eq!(error.exit, 2);
        assert!(error.message.contains("No archive was made"));
    }

    #[test]
    fn an_archive_that_does_not_match_fails_its_check() {
        let f = Fixture::new();
        f.write("ordinary");
        let policy = f.policy();
        let plan = preflight(&policy).unwrap();
        let bad = f.0.join("bad.zip");
        fs::write(&bad, b"not an archive").unwrap();
        assert_eq!(
            readback(&policy, &bad, &plan).unwrap_err().code,
            Code::ArchiveUnreadable
        );
    }

    #[test]
    fn what_unpack_would_refuse_is_refused_first() {
        let f = Fixture::new();
        f.write("ordinary");
        fs::hard_link(f.0.join("source/ordinary"), f.0.join("source/second")).unwrap();
        let error = preflight(&f.policy()).unwrap_err();
        assert_eq!(error.code, Code::UnsafeInput);
        assert!(error.message.contains("hard links"));
        fs::remove_file(f.0.join("source/second")).unwrap();
        // One name inside the folder is fine, even with another name elsewhere.
        fs::hard_link(f.0.join("source/ordinary"), f.0.join("outside")).unwrap();
        assert!(preflight(&f.policy()).is_ok());
        fs::set_permissions(
            f.0.join("source/ordinary"),
            std::os::unix::fs::PermissionsExt::from_mode(0o4755),
        )
        .unwrap();
        // The no-network sandbox denies setting the bit, so there is nothing to refuse.
        let mode = fs::metadata(f.0.join("source/ordinary")).unwrap().mode();
        if mode & 0o4000 == 0 {
            return;
        }
        assert!(
            preflight(&f.policy())
                .unwrap_err()
                .message
                .contains("setuid")
        );
    }

    #[test]
    fn the_size_step_covers_what_it_is_for() {
        for bytes in [1, 1024 * 1024, 1024 * 1024 + 1, 3 * 1024 * 1024 * 1024 + 7] {
            assert!(size(&size_flag(bytes)).unwrap() >= bytes);
        }
    }

    #[test]
    fn names_that_cannot_round_trip_are_refused_up_front() {
        // unpack goes by the suffix, so a mismatched name is an archive it refuses.
        let args = |into: &str, format: &str| {
            vec![
                "source".to_string(),
                "--for".into(),
                "other".into(),
                "--into".into(),
                into.into(),
                "--format".into(),
                format.into(),
            ]
        };
        assert!(parse(&args("a.tar.gz", "zip")).is_err());
        assert!(parse(&args("a.zip", "tar.gz")).is_err());
        assert!(parse(&args("a.zip", "zip")).is_ok());
        assert!(parse(&args("a.TGZ", "tar.gz")).is_ok());
        assert!(parse(&args("folder/", "zip")).is_err());
        assert!(parse(&args("", "zip")).is_err());
    }

    #[test]
    fn list_mode_proves_a_leftover_and_removes_nothing() {
        let f = Fixture::new();
        let dest = f.0.join("out.zip");
        let left = abandoned(&dest);
        let mut found = sweep_leftovers(&dest, false);
        for _ in 0..200 {
            if found.removable > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
            found = sweep_leftovers(&dest, false);
        }
        assert_eq!((found.removed, found.removable), (0, 1));
        assert!(left.exists());
    }

    #[test]
    fn the_home_refusal_does_not_depend_on_the_variable() {
        // Compared, never read: this touches nothing in the real home folder.
        let account = fs::canonicalize(sys::account_home().unwrap()).unwrap();
        let homes = vec![account.clone()];
        assert_eq!(home_relation(&account, &homes), Some(HomeRelation::Is));
        let parent = account.parent().unwrap();
        assert_eq!(home_relation(parent, &homes), Some(HomeRelation::Holds));
        assert_eq!(home_relation(&account.join("Documents"), &homes), None);
        assert_eq!(home_relation(Path::new("/nowhere"), &homes), None);
        // Whatever HOME says in this test run, the account's home is still listed.
        assert!(home_folders().contains(&account));
    }

    #[test]
    fn a_value_that_starts_with_a_dash_is_named_as_given() {
        let run = |into: &str| {
            parse(&[
                "source".to_string(),
                "--for".into(),
                "other".into(),
                "--into".into(),
                into.into(),
            ])
            .unwrap_err()
            .message
        };
        assert!(run("-x.zip").contains("got \"-x.zip\""));
        assert!(run("--list").contains("needs a value"));
    }

    #[test]
    fn a_setgid_folder_is_refused_as_unpack_would() {
        let f = Fixture::new();
        fs::create_dir(f.0.join("source/shared")).unwrap();
        fs::set_permissions(
            f.0.join("source/shared"),
            std::os::unix::fs::PermissionsExt::from_mode(0o2755),
        )
        .unwrap();
        // The no-network sandbox denies setting the bit, so there is nothing to refuse.
        let mode = fs::metadata(f.0.join("source/shared")).unwrap().mode();
        if mode & 0o2000 == 0 {
            return;
        }
        assert!(
            preflight(&f.policy())
                .unwrap_err()
                .message
                .contains("setuid or setgid")
        );
    }
}
