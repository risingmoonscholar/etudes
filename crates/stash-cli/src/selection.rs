//! Exact selections retain absolute original paths in the existing encrypted journal.
use etude_cli_support::problems::{self, Problem};
use etude_cli_support::reason::Code;
use etude_cli_support::{plain_io_error, shown, shown_text};
use etude_core::plan::{Group, Plan, Signal};
use etude_core::scan;
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_ITEMS: usize = 100_000;
const MAX_INPUT_BYTES: u64 = 32 * 1024 * 1024;

pub struct Options {
    pub sources: Vec<PathBuf>,
    pub into: Option<PathBuf>,
    pub duration: Option<String>,
    pub json: bool,
}

pub fn receipt_arguments(args: &[String]) -> Vec<String> {
    let mut receipt = vec!["select".into()];
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--" => break,
            flag @ ("--for" | "--into" | "--from0") => {
                // A missing value must not swallow the option after it, or a
                // --json caller would get no result to read.
                i += if args
                    .get(i + 1)
                    .is_some_and(|next| takes_as_value(flag, next))
                {
                    2
                } else {
                    1
                };
            }
            "--json" => {
                receipt.push("--json".into());
                i += 1;
            }
            _ => i += 1,
        }
    }
    receipt
}

/// `-` alone is the one dash-led value: `--from0 -` reads the list from stdin.
fn takes_as_value(flag: &str, next: &str) -> bool {
    !next.starts_with('-') || (flag == "--from0" && next == "-")
}

fn bad_argument(text: String) -> Problem {
    Problem::refused(format!("stash: {text}"), Code::InvalidArguments)
}

fn unsafe_input(text: String) -> Problem {
    Problem::refused(format!("stash: {text}"), Code::UnsafeInput)
}

pub fn parse(args: &[String]) -> Result<Options, Problem> {
    let mut options = Options {
        sources: vec![],
        into: None,
        duration: None,
        json: false,
    };
    let mut from0 = None;
    let mut positional = false;
    let mut i = 1;
    while i < args.len() {
        let token = &args[i];
        if !positional && token == "--" {
            positional = true;
            i += 1;
            continue;
        }
        if !positional && ["--for", "--into", "--from0"].contains(&token.as_str()) {
            let next = args.get(i + 1);
            let value = next
                .filter(|value| takes_as_value(token, value))
                .ok_or_else(|| {
                    bad_argument(super::missing_value_text(token, next.map(String::as_str)))
                })?;
            match token.as_str() {
                "--for" if options.duration.is_none() => options.duration = Some(value.clone()),
                "--into" if options.into.is_none() => {
                    options.into = Some(PathBuf::from(super::expand_tilde(value)))
                }
                "--from0" if from0.is_none() => from0 = Some(value.clone()),
                _ => {
                    return Err(bad_argument(format!(
                        "{token} was given more than once, and only the first would have been used. \
                         Remove the ones you do not mean."
                    )));
                }
            }
            i += 2;
            continue;
        }
        if !positional && token == "--json" {
            options.json = true;
            i += 1;
            continue;
        }
        if !positional && token == "--no-journal" {
            return Err(unsafe_input(
                "stash select cannot run without undo, so remove --no-journal. Nothing was moved."
                    .to_owned(),
            ));
        }
        if !positional && token.starts_with('-') {
            return Err(Problem::refused(
                format!(
                    "stash: unknown option {}. Run `stash help`.",
                    shown_text(token)
                ),
                Code::UnknownOption,
            ));
        }
        options
            .sources
            .push(PathBuf::from(super::expand_tilde(token)));
        i += 1;
    }
    if let Some(from0) = from0 {
        let list = PathBuf::from(super::expand_tilde(&from0));
        let reader: Box<dyn Read> = if from0 == "-" {
            Box::new(std::io::stdin())
        } else {
            Box::new(
                scan::observe_read("selection_list_open", std::fs::File::open(&list))
                    .map_err(|error| list_problem(&from0, &list, &error))?,
            )
        };
        let mut bytes = Vec::new();
        scan::observe_read(
            "selection_list_bytes",
            reader.take(MAX_INPUT_BYTES + 1).read_to_end(&mut bytes),
        )
        .map_err(|error| list_problem(&from0, &list, &error))?;
        if bytes.len() as u64 > MAX_INPUT_BYTES {
            return Err(unsafe_input(
                "the --from0 list is larger than 32 MiB. Nothing was moved. Use a shorter list."
                    .to_owned(),
            ));
        }
        if !bytes.is_empty() && bytes.last() != Some(&0) {
            return Err(unsafe_input(
                "each path in the --from0 list must end with a NUL byte. Nothing was moved."
                    .to_owned(),
            ));
        }
        let records = if bytes.is_empty() {
            &[][..]
        } else {
            &bytes[..bytes.len() - 1]
        };
        for item in records
            .split(|byte| *byte == 0)
            .filter(|_| !bytes.is_empty())
        {
            if item.is_empty() {
                return Err(unsafe_input(
                    "the --from0 list has an empty path. Nothing was moved. Remove it.".to_owned(),
                ));
            }
            let text = std::str::from_utf8(item).map_err(|_| {
                unsafe_input(
                    "the --from0 list has a path that is not UTF-8, and undo cannot record it. \
                     Nothing was moved. Remove it from the list."
                        .to_owned(),
                )
            })?;
            options.sources.push(PathBuf::from(text));
        }
        scan::verify_read("selection_list_bytes");
    }
    if options.sources.len() > MAX_ITEMS {
        return Err(unsafe_input(format!(
            "stash select takes at most {MAX_ITEMS} items at a time, and this names {}. Nothing was moved.",
            options.sources.len()
        )));
    }
    Ok(options)
}

/// The `--from0` list could not be opened or read. Nothing has happened yet.
fn list_problem(typed: &str, list: &Path, error: &std::io::Error) -> Problem {
    if typed == "-" {
        return Problem::failed(
            format!(
                "stash: cannot read the list from standard input: {}. Nothing was moved.",
                plain_io_error(error)
            ),
            io_reason(error),
        );
    }
    let name = shown(list);
    match error.kind() {
        std::io::ErrorKind::IsADirectory => bad_argument(format!(
            "{name} is a folder; --from0 needs a file. Nothing was moved."
        )),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory => Problem::failed(
            format!("stash: no list file at {name}. Nothing was moved."),
            Code::MissingInput,
        ),
        _ => Problem::failed(
            format!(
                "stash: cannot read {name}: {}. Nothing was moved.",
                plain_io_error(error)
            ),
            io_reason(error),
        ),
    }
}

/// A selected path that could not be checked.
fn source_problem(source: &Path, error: &scan::ScanError) -> Problem {
    if error.is_refusal() {
        return Problem::refused(scan_text(source, error), Code::UnsafeInput);
    }
    let nothing_there = || {
        Problem::failed(
            format!("stash: nothing at {}. Nothing was moved.", shown(source)),
            Code::MissingInput,
        )
    };
    match error {
        scan::ScanError::Io(io)
            if matches!(
                io.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            nothing_there()
        }
        scan::ScanError::Io(io) => {
            let mut text = format!(
                "stash: cannot read {}: {}. Nothing was moved.",
                shown(source),
                plain_io_error(io)
            );
            if let Some(advice) = scan::permission_advice(io) {
                text.push('\n');
                text.push_str(advice);
            }
            Problem::failed(text, io_reason(io))
        }
        scan::ScanError::NotADirectory(_) => nothing_there(),
        _ => Problem::failed(scan_text(source, error), Code::InternalError),
    }
}

fn scan_text(path: &Path, error: &scan::ScanError) -> String {
    format!(
        "{} Nothing was moved.",
        problems::scan_problem("stash", path, error)
    )
}

/// The place to keep held items could not be used. It is the folder the person
/// named with `--into`, or the folder beside the first selected item.
fn parent_problem(parent: &Path, named: bool, cause: &std::io::Error) -> Problem {
    let place = if named {
        format!("--into folder {}", shown(parent))
    } else {
        format!("the folder {}", shown(parent))
    };
    if matches!(
        cause.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
    ) {
        return Problem::failed(
            format!("stash: no folder at {}. Nothing was moved.", shown(parent)),
            Code::MissingInput,
        );
    }
    Problem::refused(
        format!(
            "stash: cannot use {place}: {}. Nothing was moved.",
            plain_io_error(cause)
        ),
        io_reason(cause),
    )
}

pub struct Prepared {
    pub sources: Vec<PathBuf>,
    pub parent: PathBuf,
}
pub fn prepare(options: &Options) -> Result<Prepared, Problem> {
    let mut entries = Vec::with_capacity(options.sources.len());
    #[cfg(unix)]
    let mut identities = std::collections::HashMap::new();
    for source in &options.sources {
        let (path, metadata) =
            scan::selected_object(source).map_err(|error| source_problem(source, &error))?;
        let name = shown(source);
        if metadata.file_type().is_symlink() {
            return Err(unsafe_input(format!(
                "{name} is a symbolic link, and stash select does not move links. \
                 Nothing was moved. Name the file or folder it points to."
            )));
        }
        if !(metadata.is_file() || metadata.is_dir()) {
            return Err(unsafe_input(format!(
                "{name} is not a file or folder, so it cannot be stashed. Nothing was moved."
            )));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if let Some(first) = identities.insert((metadata.dev(), metadata.ino()), source) {
                return Err(unsafe_input(if first == source {
                    format!("you named {name} twice. Nothing was moved. Name it once.")
                } else {
                    format!(
                        "{} and {name} are the same file. Nothing was moved. Name it once.",
                        shown(first)
                    )
                }));
            }
        }
        if scan::is_synced(&path) {
            return Err(unsafe_input(format!(
                "{name} is in a cloud-synced folder, and stash select cannot use those. \
                 Nothing was moved. Choose items outside it."
            )));
        }
        entries.push((path, metadata));
    }
    entries.sort_by(|a, b| a.0.components().cmp(b.0.components()));
    for adjacent in entries.windows(2) {
        if adjacent[1].0.starts_with(&adjacent[0].0) {
            return Err(unsafe_input(format!(
                "{} is inside {}, which you also selected. Nothing was moved. Name one.",
                shown(&adjacent[1].0),
                shown(&adjacent[0].0)
            )));
        }
    }
    let Some((first, _)) = entries.first() else {
        return Ok(Prepared {
            sources: vec![],
            parent: PathBuf::new(),
        });
    };
    let named = options.into.is_some();
    let typed = options
        .into
        .clone()
        .unwrap_or_else(|| first.parent().unwrap_or(Path::new("/")).to_path_buf());
    let parent = scan::observe_read("selection_parent_metadata", typed.canonicalize())
        .map_err(|error| parent_problem(&typed, named, &error))?;
    let (parent, metadata) = scan::selected_object(&parent).map_err(|error| match &error {
        scan::ScanError::Io(cause) => parent_problem(&typed, named, cause),
        other => Problem::refused(scan_text(&typed, other), Code::UnsafeInput),
    })?;
    if scan::is_synced(&parent) {
        return Err(unsafe_input(format!(
            "--into folder {} is in a cloud-synced folder, and stash select cannot use those. \
             Nothing was moved. Choose a local folder.",
            shown(&typed)
        )));
    }
    if !metadata.is_dir() {
        return Err(unsafe_input(format!(
            "--into needs a folder, and {} is not one. Nothing was moved.",
            shown(&typed)
        )));
    }
    for (source, source_metadata) in &entries {
        if parent.starts_with(source) {
            return Err(unsafe_input(format!(
                "--into folder {} is inside {}, which you selected. Nothing was moved. \
                 Choose a folder outside it.",
                shown(&typed),
                shown(source)
            )));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if !source_metadata.is_file() && source_metadata.dev() != metadata.dev() {
                return Err(unsafe_input(format!(
                    "folder {} is on another disk from {}, and a folder cannot be stashed across disks. \
                     Nothing was moved. Use --into with a folder on the same disk as it.",
                    shown(source),
                    shown(&parent)
                )));
            }
        }
    }
    Ok(Prepared {
        sources: entries.into_iter().map(|e| e.0).collect(),
        parent,
    })
}

pub fn reserve(parent: &Path, named: bool) -> Result<PathBuf, Problem> {
    let folder = shown(parent);
    for _ in 0..128 {
        let mut random = [0u8; 16];
        scan::observe_read(
            "os_random_bytes",
            std::fs::File::open("/dev/urandom").and_then(|mut file| file.read_exact(&mut random)),
        )
        .map_err(|error| {
            Problem::failed(
                format!(
                    "stash: could not get random bytes from the system: {}. Nothing was moved.",
                    plain_io_error(&error)
                ),
                io_reason(&error),
            )
        })?;
        let name = random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let root = parent.join(format!(".stash-selection-{name}"));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(&root) {
            Ok(()) => return Ok(root),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                // Without --into this is the selected item's own folder. An item
                // cannot leave a folder that refuses writes, whatever --into says.
                let elsewhere = "Choose another place with --into FOLDER.";
                let (why, next) = if problems::disk_full(&error) {
                    (
                        "the disk is full".to_owned(),
                        if named {
                            elsewhere
                        } else {
                            "Free some space, or choose another disk with --into FOLDER."
                        },
                    )
                } else if problems::disk_read_only(&error) {
                    (
                        "the disk is read-only".to_owned(),
                        if named {
                            elsewhere
                        } else {
                            "Items on a read-only disk cannot be moved."
                        },
                    )
                } else {
                    (
                        plain_io_error(&error),
                        if named {
                            elsewhere
                        } else {
                            "Check it with `ls -ld`, then run this again."
                        },
                    )
                };
                return Err(Problem::failed(
                    format!("stash: cannot write in {folder}: {why}. Nothing was moved. {next}"),
                    io_reason(&error),
                ));
            }
        }
    }
    Err(Problem::failed(
        format!(
            "stash: could not make a new private folder in {folder}. Nothing was moved. \
             Choose another place with --into FOLDER."
        ),
        Code::InternalError,
    ))
}

fn io_reason(error: &std::io::Error) -> Code {
    match error.kind() {
        std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ReadOnlyFilesystem => {
            Code::PermissionDenied
        }
        std::io::ErrorKind::StorageFull => Code::InsufficientSpace,
        // "Not a directory" is a path that runs through a regular file: it
        // names nothing, exactly as a missing object does.
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory => Code::MissingInput,
        _ => Code::InternalError,
    }
}

pub fn plan(prepared: &Prepared, root: PathBuf, holding: &str) -> Plan {
    let mut plan = Plan::display_only(
        root,
        prepared
            .sources
            .iter()
            .enumerate()
            .map(|(index, path)| Group {
                name: format!("{holding}/{index:08}"),
                signal: Signal::Collected { count: 1 },
                members: vec![path.clone()],
                accepted: true,
            })
            .collect(),
    );
    // The caller named these objects, tagged or not.
    plan.include_tagged = true;
    plan
}

pub fn is_selection_root(root: &Path) -> bool {
    root.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.strip_prefix(".stash-selection-")
                .is_some_and(|suffix| {
                    suffix.len() == 32 && suffix.bytes().all(|b| b.is_ascii_hexdigit())
                })
        })
}

pub fn remove_empty_root(journal: &etude_core::Journal) {
    let root = &journal.root;
    if !is_selection_root(root) || journal.entries.is_empty() {
        return;
    }
    let mut directories = std::collections::BTreeSet::new();
    for entry in &journal.entries {
        let Ok(relative) = entry.to.strip_prefix(root) else {
            return;
        };
        let parts: Vec<_> = relative.components().collect();
        if parts.len() != 3 {
            return;
        }
        let (Some(holding), Some(slot)) =
            (parts[0].as_os_str().to_str(), parts[1].as_os_str().to_str())
        else {
            return;
        };
        if !holding.starts_with(".stash-")
            || slot.len() != 8
            || !slot.bytes().all(|byte| byte.is_ascii_digit())
        {
            return;
        }
        directories.insert(entry.to.parent().unwrap().to_path_buf());
        directories.insert(entry.to.parent().unwrap().parent().unwrap().to_path_buf());
    }
    let mut directories: Vec<_> = directories.into_iter().collect();
    directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for directory in directories {
        let _ = std::fs::remove_dir(directory);
    }
    let _ = std::fs::remove_dir(root);
}
