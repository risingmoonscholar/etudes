//! Journal-backed batch lookup never chooses a different operation for an explicit id.
use etude_core::journal::{self, Journal, JournalError, Sealer};
use std::path::{Path, PathBuf};

pub enum Selector {
    Id(String),
    Root(PathBuf),
    Latest(Option<PathBuf>),
}
pub fn selector(args: &[String]) -> Result<Selector, &'static str> {
    let mut id = None;
    let mut named = None;
    let mut latest = false;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--id" => {
                index += 1;
                let value = args.get(index).ok_or("--id requires an operation id")?;
                if id.replace(value.clone()).is_some() {
                    return Err("--id may be supplied once");
                }
            }
            "--latest" => {
                if latest {
                    return Err("--latest may be supplied once");
                }
                latest = true;
            }
            "--json" | "--if-due" => {}
            value if value.starts_with('-') => return Err("unknown pop option"),
            value => {
                if named.replace(value.to_owned()).is_some() {
                    return Err("pop accepts one id or path");
                }
            }
        }
        index += 1;
    }
    if id.is_some() && (latest || named.is_some()) {
        return Err("--id cannot be combined with a path or --latest");
    }
    if let Some(id) = id {
        journal::validate_id(&id).map_err(|_| "invalid operation id")?;
        return Ok(Selector::Id(id));
    }
    if !latest
        && let Some(value) = &named
        && creation(value).is_some()
    {
        return Ok(Selector::Id(value.clone()));
    }
    let root = named.map(|path| PathBuf::from(super::expand_tilde(&path)));
    if latest {
        return Ok(Selector::Latest(root));
    }
    Ok(Selector::Root(root.unwrap_or_else(|| {
        std::env::current_dir().unwrap_or_default()
    })))
}
pub fn creation(id: &str) -> Option<u128> {
    let parts: Vec<_> = id.split('-').collect();
    if !journal::valid_journal_id(id)
        || parts
            .iter()
            .take(3)
            .any(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return None;
    }
    if parts.len() != 4
        || parts[3].is_empty()
        || !parts[3].bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    parts[1].parse::<u32>().ok()?;
    parts[2].parse::<u64>().ok()?;
    parts[0].parse().ok()
}
pub fn holding(journal: &Journal) -> Result<PathBuf, JournalError> {
    let first = journal
        .entries
        .first()
        .ok_or(JournalError::Malformed("batch has no entries"))?;
    let component = first
        .to
        .strip_prefix(&journal.root)
        .ok()
        .and_then(|path| path.components().next())
        .ok_or(JournalError::Malformed("batch holding is outside root"))?;
    let name = component
        .as_os_str()
        .to_str()
        .ok_or(JournalError::Malformed("invalid holding name"))?;
    if !name.starts_with(".stash-") {
        return Err(JournalError::Malformed("not stash holding storage"));
    }
    let holding = journal.root.join(component.as_os_str());
    if journal
        .entries
        .iter()
        .any(|entry| !entry.to.starts_with(&holding))
    {
        return Err(JournalError::Malformed(
            "batch has inconsistent holding storage",
        ));
    }
    Ok(holding)
}
pub fn due(journal: &Journal) -> Result<Option<u64>, JournalError> {
    let path = holding(journal)?;
    Ok(super::deadline_of(
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default(),
    ))
}
pub fn live(journal: &Journal) -> bool {
    !super::journal_is_fully_undone(journal)
}
pub fn inventory(sealer: &dyn Sealer) -> Result<Vec<Journal>, JournalError> {
    let candidates = match journal::candidates_unordered("stash") {
        Ok(value) => value,
        Err(JournalError::NotFound) => return Ok(vec![]),
        Err(error) => return Err(error),
    };
    let mut output = Vec::new();
    for candidate in candidates {
        let journal = candidate.load("stash", sealer)?;
        if live(&journal) {
            holding(&journal)?;
            output.push(journal);
        }
    }
    Ok(output)
}
pub fn matches_root(journal: &Journal, root: &Path) -> bool {
    journal.root.canonicalize().is_ok_and(|path| path == root)
}
pub fn select(selector: Selector, sealer: &dyn Sealer) -> Result<Option<Journal>, JournalError> {
    if let Selector::Id(id) = selector {
        return Journal::load_sealed_exact("stash", &id, sealer)
            .map(Some)
            .or_else(|error| {
                if matches!(error, JournalError::NotFound) {
                    Ok(None)
                } else {
                    Err(error)
                }
            });
    }
    let latest = matches!(selector, Selector::Latest(_));
    let path = match selector {
        Selector::Root(path) => Some(path),
        Selector::Latest(path) => path,
        Selector::Id(_) => unreachable!(),
    };
    let root = match path {
        Some(path) => match path.canonicalize() {
            Ok(path) => Some(path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(JournalError::Io(error)),
        },
        None => None,
    };
    if !latest {
        match journal::candidates_by_recency("stash") {
            Ok(_) | Err(JournalError::NotFound) => {}
            Err(error) => return Err(error),
        }
    }
    let mut values = inventory(sealer)?;
    values.retain(|journal| root.as_ref().is_none_or(|root| matches_root(journal, root)));
    if values.is_empty() {
        return Ok(None);
    }
    if !latest {
        if values.len() > 1 {
            return Err(JournalError::Malformed(
                "multiple live batches match; use an id or --latest",
            ));
        }
        return Ok(values.pop());
    }
    if values.iter().all(|journal| creation(&journal.id).is_some()) {
        values.sort_by_key(|journal| std::cmp::Reverse(creation(&journal.id).unwrap()));
        if values.len() > 1 && creation(&values[0].id) == creation(&values[1].id) {
            return Err(JournalError::Malformed("batch creation order is ambiguous"));
        }
        return Ok(Some(values.remove(0)));
    }
    // Legacy ids do not encode creation time; preserve their existing modification-time ordering.
    for candidate in journal::candidates_by_recency("stash")? {
        if let Some(index) = values.iter().position(|journal| journal.id == candidate.id) {
            return Ok(Some(values.remove(index)));
        }
    }
    Err(JournalError::NotFound)
}
pub fn held(journal: &Journal) -> Result<usize, JournalError> {
    let mut held = 0;
    for entry in &journal.entries {
        if entry.state == journal::EntryState::Reversed {
            continue;
        }
        match etude_core::scan::observe_read("metadata", std::fs::symlink_metadata(&entry.to)) {
            Ok(_) => held += 1,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(JournalError::Io(error)),
        }
    }
    Ok(held)
}
pub fn row(journal: &Journal, show_paths: bool) -> Result<String, JournalError> {
    use etude_core::json as j;
    let due = due(journal)?;
    Ok(j::obj(&[
        ("id", j::str(&journal.id)),
        (
            "root",
            if show_paths {
                j::path(&journal.root)
            } else {
                j::str("<redacted>")
            },
        ),
        ("stashed", j::num(held(journal)?)),
        ("entries", j::num(journal.entries.len())),
        (
            "due",
            due.map(|value| j::str(&super::iso_utc(value)))
                .unwrap_or_else(|| "null".into()),
        ),
        (
            "overdue",
            j::bool(due.is_some_and(|value| value <= super::now_secs())),
        ),
        (
            "journal_progress_damaged",
            j::bool(journal.progress_tail_damaged),
        ),
    ]))
}
