//! Apply and undo.
//!
//! Ordering rule: the journal is written before the first move, and each entry
//! is marked done only after its move succeeds. A crash at any point leaves a
//! journal that describes exactly what happened.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::journal::{Entry, Journal, Method, Sealer, fingerprint};
use crate::plan::Plan;

#[derive(Debug)]
pub enum ApplyError {
    Io(io::Error),
    Journal(crate::journal::JournalError),
    DestinationExists(PathBuf),
    DestinationCollision(PathBuf),
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
            ApplyError::DestinationCollision(p) => write!(
                f,
                "two files in this plan would move to the same destination: {}",
                crate::redact::path(p)
            ),
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
    let mut planned_destinations = HashSet::new();
    for g in plan.groups.iter().filter(|g| g.accepted) {
        let dest_dir = plan.root.join(&g.name);
        if crate::scan::is_synced(&dest_dir) && !plan.allow_sync {
            return Err(ApplyError::DestinationIsSynced(dest_dir));
        }
        for src in &g.members {
            let Some(name) = src.file_name() else {
                continue;
            };
            let dst = dest_dir.join(name);
            if dst.exists() {
                return Err(ApplyError::DestinationExists(dst));
            }
            if !planned_destinations.insert(dst.to_string_lossy().to_lowercase()) {
                return Err(ApplyError::DestinationCollision(dst));
            }
            let (size, mtime_secs, inode, edge_hash) = fingerprint(src).map_err(ApplyError::Io)?;
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
        return Ok(ApplyReport {
            moved: 0,
            journal_id: id,
            journal_path: None,
        });
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
        // Append a sealed done-record (index + corrected method). The base
        // journal was written before the loop; rewriting it here was O(n²).
        if let Some(sl) = sealer {
            j.record_done(i, method, sl).map_err(ApplyError::Journal)?;
        }
    }

    Ok(ApplyReport {
        moved,
        journal_id: id,
        journal_path: sealer.map(|_| j.path()),
    })
}

/// link → unlink within one device; copy → verify → unlink across devices.
fn move_one(from: &Path, to: &Path) -> io::Result<Method> {
    // link(2) follows symlinks to their targets and refuses directories, so
    // only regular files can safely use link → unlink in place of rename.
    if fs::symlink_metadata(from)?.file_type().is_file() {
        match fs::hard_link(from, to) {
            Ok(()) => {
                // A crash before unlink leaves two readable names for one inode,
                // trading atomicity for durability without clobbering either file.
                fs::remove_file(from)?;
                return Ok(Method::Rename);
            }
            Err(e) if e.raw_os_error() == Some(18) => {}
            Err(e) => return Err(e),
        }
    }

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
    /// Set when a move failed and undo stopped early. `restored` and the
    /// `skipped_*` lists above still describe everything that happened
    /// *before* the failure — they are never discarded just because the walk
    /// did not finish. The caller must still persist `j` (e.g. via
    /// `save_sealed`) so the on-disk journal matches what was actually
    /// restored; `undo` mutates the in-memory entries but does not know how
    /// to seal them.
    pub error: Option<ApplyError>,
}

/// Reverse a journal, verifying each file before touching it.
///
/// A file that changed since apply is **reported and skipped**, never
/// overwritten.
///
/// Always returns a report, even when a move fails partway through: this
/// never discards what it already did. Contrast with `apply()`, which
/// persists progress to disk after every move via `record_done`; `undo` has
/// no equivalent incremental on-disk format, so it mutates `j.entries[i].done`
/// in memory only. The caller — which holds the sealer used to load `j` in
/// the first place — is responsible for calling `j.save_sealed(..)` after
/// this returns, on *both* the success and the `error.is_some()` path, or
/// the journal will drift from physical reality exactly the way it used to.
pub fn undo(j: &mut Journal) -> UndoReport {
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
        if let Some(parent) = e.from.parent()
            && let Err(err) = fs::create_dir_all(parent)
        {
            r.error = Some(ApplyError::Io(err));
            break;
        }
        match move_one(&e.to, &e.from) {
            Ok(_) => {
                j.entries[i].done = false;
                r.restored += 1;
            }
            Err(err) => {
                r.error = Some(ApplyError::Io(err));
                break;
            }
        }
    }

    // Remove destination directories that we created and that are now empty.
    // Safe to run even after a partial failure: it only ever removes
    // directories that are already empty, so it can't erase evidence of
    // entries that did not get restored.
    let mut dirs: Vec<&Path> = j.entries.iter().filter_map(|e| e.to.parent()).collect();
    dirs.sort();
    dirs.dedup();
    for d in dirs {
        if d != j.root {
            let _ = fs::remove_dir(d); // fails harmlessly when not empty
        }
    }

    r
}

// The counter makes same-process collisions impossible (monotonic, never
// repeats). pid + nanos only make cross-process collisions astronomically
// unlikely — not impossible (pid reuse after wraparound with a repeated or
// backward-stepping clock reading remains a theoretical residual risk).
fn journal_id(plan: &Plan) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in plan.root.to_string_lossy().as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    format!("{nanos}-{pid}-{n}-{h:x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journal_id_is_unique_across_tight_loop() {
        // Tight loop finishes well under one second; under seconds+hash-only
        // ids this fails every time, proving the counter (not wall-clock luck).
        let plan = Plan {
            root: PathBuf::from("/tmp/journal_id_test_root"),
            groups: Vec::new(),
            untouched: Vec::new(),
            scanned: 0,
            skipped_hidden: 0,
            skipped_symlink: 0,
            root_is_synced: false,
            allow_sync: false,
        };
        const N: usize = 200;
        let mut ids = HashSet::with_capacity(N);
        for _ in 0..N {
            assert!(ids.insert(journal_id(&plan)));
        }
        assert_eq!(ids.len(), N);
    }
}
