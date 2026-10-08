//! Exact selections retain absolute original paths in the existing encrypted journal.
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
}
#[derive(Debug)]
pub enum Error {
    Refused(&'static str),
    Io(&'static str),
}
impl Error {
    pub fn message(&self) -> &'static str {
        match self {
            Self::Refused(m) | Self::Io(m) => m,
        }
    }
    pub fn code(&self) -> u8 {
        match self {
            Self::Refused(_) => 2,
            Self::Io(_) => 3,
        }
    }
}

pub fn receipt_arguments(args: &[String]) -> Vec<String> {
    let mut receipt = vec!["select".into()];
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--" => break,
            "--for" | "--into" | "--from0" => i += 2,
            "--json" => {
                receipt.push("--json".into());
                i += 1;
            }
            _ => i += 1,
        }
    }
    receipt
}

pub fn parse(args: &[String]) -> Result<Options, Error> {
    let mut options = Options {
        sources: vec![],
        into: None,
        duration: None,
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
            let value = args
                .get(i + 1)
                .ok_or(Error::Refused("selection option needs a value"))?;
            match token.as_str() {
                "--for" if options.duration.is_none() => options.duration = Some(value.clone()),
                "--into" if options.into.is_none() => {
                    options.into = Some(PathBuf::from(super::expand_tilde(value)))
                }
                "--from0" if from0.is_none() => from0 = Some(value.clone()),
                _ => return Err(Error::Refused("selection option repeated")),
            }
            i += 2;
            continue;
        }
        if !positional && token == "--json" {
            i += 1;
            continue;
        }
        if !positional && token == "--no-journal" {
            return Err(Error::Refused(
                "explicit selection requires an encrypted journal to retain original parents",
            ));
        }
        if !positional && token.starts_with('-') {
            return Err(Error::Refused("unknown selection option"));
        }
        options
            .sources
            .push(PathBuf::from(super::expand_tilde(token)));
        i += 1;
    }
    if let Some(from0) = from0 {
        let reader: Box<dyn Read> = if from0 == "-" {
            Box::new(std::io::stdin())
        } else {
            Box::new(
                scan::observe_read(
                    "selection_list_open",
                    std::fs::File::open(super::expand_tilde(&from0)),
                )
                .map_err(|_| Error::Io("could not open selection list"))?,
            )
        };
        let mut bytes = Vec::new();
        scan::observe_read(
            "selection_list_bytes",
            reader.take(MAX_INPUT_BYTES + 1).read_to_end(&mut bytes),
        )
        .map_err(|_| Error::Io("could not read selection list"))?;
        if bytes.len() as u64 > MAX_INPUT_BYTES {
            return Err(Error::Refused("selection list exceeds 32 MiB"));
        }
        if !bytes.is_empty() && bytes.last() != Some(&0) {
            return Err(Error::Refused("selection list must end each path with NUL"));
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
                return Err(Error::Refused("selection list contains an empty path"));
            }
            let text = std::str::from_utf8(item).map_err(|_| {
                Error::Refused("selection paths must be UTF-8 for reversible journals")
            })?;
            options.sources.push(PathBuf::from(text));
        }
        scan::verify_read("selection_list_bytes");
    }
    if options.sources.len() > MAX_ITEMS {
        return Err(Error::Refused("selection exceeds 100000 objects"));
    }
    Ok(options)
}

pub struct Prepared {
    pub sources: Vec<PathBuf>,
    pub parent: PathBuf,
}
pub fn prepare(options: &Options) -> Result<Prepared, Error> {
    let mut entries = Vec::with_capacity(options.sources.len());
    #[cfg(unix)]
    let mut identities = std::collections::HashSet::new();
    for source in &options.sources {
        let (path, metadata) = scan::selected_object(source).map_err(|error| {
            if error.is_refusal() {
                Error::Refused("selected location is refused")
            } else {
                Error::Io("could not inspect a selected object")
            }
        })?;
        if metadata.file_type().is_symlink() {
            return Err(Error::Refused(
                "explicit selection does not support symlinks",
            ));
        }
        if !(metadata.is_file() || metadata.is_dir()) {
            return Err(Error::Refused(
                "selection contains an unsupported object type",
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if !identities.insert((metadata.dev(), metadata.ino())) {
                return Err(Error::Refused("selection repeats an object identity"));
            }
        }
        entries.push((path, metadata));
    }
    entries.sort_by(|a, b| a.0.components().cmp(b.0.components()));
    for adjacent in entries.windows(2) {
        if adjacent[1].0.starts_with(&adjacent[0].0) {
            return Err(Error::Refused(
                "selection contains duplicate or overlapping objects",
            ));
        }
    }
    let Some((first, _)) = entries.first() else {
        return Ok(Prepared {
            sources: vec![],
            parent: PathBuf::new(),
        });
    };
    let parent = options
        .into
        .clone()
        .unwrap_or_else(|| first.parent().unwrap_or(Path::new("/")).to_path_buf());
    let (parent, metadata) = scan::selected_object(&parent)
        .map_err(|_| Error::Refused("holding parent is unavailable or refused"))?;
    if !metadata.is_dir() {
        return Err(Error::Refused("holding parent must be a directory"));
    }
    for (source, source_metadata) in &entries {
        if parent.starts_with(source) {
            return Err(Error::Refused("holding parent is inside a selected object"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if !source_metadata.is_file() && source_metadata.dev() != metadata.dev() {
                return Err(Error::Refused(
                    "selected directories need holding storage on the same volume",
                ));
            }
        }
    }
    Ok(Prepared {
        sources: entries.into_iter().map(|e| e.0).collect(),
        parent,
    })
}

pub fn reserve(parent: &Path) -> Result<PathBuf, Error> {
    for _ in 0..128 {
        let mut random = [0u8; 16];
        scan::observe_read(
            "os_random_bytes",
            std::fs::File::open("/dev/urandom").and_then(|mut file| file.read_exact(&mut random)),
        )
        .map_err(|_| Error::Io("could not name private holding storage"))?;
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
            Err(_) => return Err(Error::Io("could not create private holding storage")),
        }
    }
    Err(Error::Io("could not reserve holding storage"))
}

pub fn plan(prepared: &Prepared, root: PathBuf, holding: &str) -> Plan {
    let count = prepared.sources.len();
    Plan {
        root,
        groups: prepared
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
        untouched: vec![],
        scanned: count,
        skipped_hidden: 0,
        skipped_symlink: 0,
        skipped_system: 0,
        skipped_project: 0,
        skipped_in_flight: 0,
        skipped_package: 0,
        skipped_unreadable: 0,
        root_is_synced: false,
        allow_sync: true,
    }
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
