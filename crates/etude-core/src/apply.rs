//! Apply and undo.
//!
//! Ordering rule: the journal is written before the first
//! move, and each entry is marked done only after its move succeeds. A crash at
//! any point leaves a journal that describes exactly what happened.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::journal::{fingerprint, Entry, Journal, Method, Sealer};
use crate::plan::Plan;

#[derive(Debug)]
pub enum ApplyError {
    Io(io::Error),
    Journal(crate::journal::JournalError),
    DestinationExists(PathBuf),
    DestinationIsSynced(PathBuf),
    /// Injected by tests to prove the journal stays resumable.
    Injected(usize),
}

impl std::fmt::Display for ApplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApplyError::Io(e) => write!(f, "io error: {}", e.kind()),
            ApplyError::Journal(e) => write!(f, "{e}"),
            ApplyError::DestinationExists(p) => {
                write!(f, "destination already exists: {}", crate::redact::path(p))
            }
            ApplyError::DestinationIsSynced(p) => write!(
                f,
                "refused: destination {} is inside a cloud-synced folder",
                crate::redact::path(p)
            ),
            ApplyError::Injected(n) => write!(f, "injected failure at move {n}"),
        }
    }
}

#[derive(Debug, Default)]
pub struct ApplyReport {
    pub moved: usize,
    pub journal_id: String,
    pub journal_path: Option<PathBuf>,
}

/// Test hook. Production callers pass `None`.
pub type FailAt = Option<usize>;

/// Execute the accepted groups of `plan`.
///
/// Pass `sealer: None` for the privacy-maximal mode: nothing is recorded and
/// undo becomes impossible. The caller must have told the user so.
///
/// There is no plaintext journal path. Either it is sealed or it is absent.
pub fn apply(
    plan: &Plan,
    tool: &str,
    sealer: Option<&dyn Sealer>,
    fail_at: FailAt,
) -> Result<ApplyReport, ApplyError> {
    let id = journal_id(plan);
    let mut j = Journal {
        id: id.clone(),
        tool: tool.to_string(),
        root: plan.root.clone(),
        entries: Vec::new(),
    };

    // Build the full entry list first, so the journal describes the whole
    // intended operation before any of it happens.
    for g in plan.groups.iter().filter(|g| g.accepted) {
        let dest_dir = plan.root.join(&g.name);
        if crate::scan::is_synced(&dest_dir) {
            return Err(ApplyError::DestinationIsSynced(dest_dir));
        }
        for src in &g.members {
            let Some(name) = src.file_name() else { continue };
            let dst = dest_dir.join(name);
            if dst.exists() {
                return Err(ApplyError::DestinationExists(dst));
            }
            let (size, mtime_secs, inode, edge_hash) =
                fingerprint(src).map_err(ApplyError::Io)?;
            j.entries.push(Entry {
                from: src.clone(),
                to: dst,
                method: Method::Rename, // corrected below if cross-device
                size,
                mtime_secs,
                inode,
                edge_hash,
                done: false,
            });
        }
    }

    if j.entries.is_empty() {
        return Ok(ApplyReport { moved: 0, journal_id: id, journal_path: None });
    }

    // Journal first. Nothing has moved yet.
    if let Some(sl) = sealer {
        j.save_sealed(sl).map_err(ApplyError::Journal)?;
    }

    let mut moved = 0usize;
    for i in 0..j.entries.len() {
        if fail_at == Some(i) {
            return Err(ApplyError::Injected(i));
        }
        let (from, to) = (j.entries[i].from.clone(), j.entries[i].to.clone());
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent).map_err(ApplyError::Io)?;
        }
        let method = move_one(&from, &to).map_err(ApplyError::Io)?;
        j.entries[i].method = method;
        j.entries[i].done = true;
        moved += 1;
        // Rewrite after each success. The journal never claims more than the
        // filesystem actually shows.
        if let Some(sl) = sealer {
            j.save_sealed(sl).map_err(ApplyError::Journal)?;
        }
    }

    Ok(ApplyReport {
        moved,
        journal_id: id,
        journal_path: sealer.map(|_| j.path()),
    })
}

/// `rename(2)` when possible; copy → verify → unlink across devices.
fn move_one(from: &Path, to: &Path) -> io::Result<Method> {
    match fs::rename(from, to) {
        Ok(()) => Ok(Method::Rename),
        Err(e) if e.raw_os_error() == Some(18) => {
            // EXDEV — cross-device. rename(2) cannot do this, so copy, verify
            // the copy landed intact, and only then unlink the source.
            fs::copy(from, to)?;
            let src_md = fs::metadata(from)?;
            let dst_md = fs::metadata(to)?;
            if src_md.len() != dst_md.len() {
                let _ = fs::remove_file(to);
                return Err(io::Error::other("cross-device copy size mismatch"));
            }
            fs::remove_file(from)?;
            Ok(Method::CopyUnlink)
        }
        Err(e) => Err(e),
    }
}

#[derive(Debug, Default)]
pub struct UndoReport {
    pub restored: usize,
    /// Entries skipped because the file changed after apply. Never forced.
    pub skipped_changed: Vec<PathBuf>,
    /// Entries whose destination no longer exists.
    pub skipped_missing: Vec<PathBuf>,
}

/// Reverse a journal, verifying each file before touching it.
///
/// A file that changed since apply is **reported and skipped**, never
/// overwritten.
pub fn undo(j: &mut Journal) -> Result<UndoReport, ApplyError> {
    let mut r = UndoReport::default();

    // Reverse order, so nested destinations empty before their parents.
    for i in (0..j.entries.len()).rev() {
        let e = j.entries[i].clone();
        if !e.done {
            continue;
        }
        if !e.to.exists() {
            r.skipped_missing.push(e.to.clone());
            continue;
        }
        let (size, mtime, inode, hash) = match fingerprint(&e.to) {
            Ok(f) => f,
            Err(_) => {
                r.skipped_missing.push(e.to.clone());
                continue;
            }
        };
        // Inode survives rename(2) but not a cross-device copy, so it is only
        // evidence for the Rename case.
        let inode_ok = e.method != Method::Rename || inode == e.inode;
        if size != e.size || mtime != e.mtime_secs || hash != e.edge_hash || !inode_ok {
            r.skipped_changed.push(e.to.clone());
            continue;
        }
        if e.from.exists() {
            r.skipped_changed.push(e.to.clone());
            continue;
        }
        if let Some(parent) = e.from.parent() {
            fs::create_dir_all(parent).map_err(ApplyError::Io)?;
        }
        move_one(&e.to, &e.from).map_err(ApplyError::Io)?;
        j.entries[i].done = false;
        r.restored += 1;
    }

    // Remove destination directories that we created and that are now empty.
    let mut dirs: Vec<&Path> = j.entries.iter().filter_map(|e| e.to.parent()).collect();
    dirs.sort();
    dirs.dedup();
    for d in dirs {
        if d != j.root {
            let _ = fs::remove_dir(d); // fails harmlessly when not empty
        }
    }

    Ok(r)
}

/// Stable-ish identifier derived from the root and the current time.
fn journal_id(plan: &Plan) -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in plan.root.to_string_lossy().as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    format!("{secs}-{h:x}")
}
