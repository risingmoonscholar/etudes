//! The undo journal.
//!
//! # Encryption
//!
//! The journal is sealed by a [`Sealer`] supplied by the caller. `etude-core`
//! deliberately does not know how. Keeping the engine dependency-free makes
//! the no-network claim cheap to check. The cipher lives in `etude-keep` and
//! is injected here.
//!
//! There is no plaintext fallback. If sealing is unavailable the journal is not
//! written and the caller is told, because silently degrading to plaintext is
//! exactly the failure a privacy tool must not have.
//!
//! # Ordering
//!
//! The journal is written *before* the first move and each entry is marked
//! complete only after its move succeeds. A crash therefore leaves a journal
//! describing exactly what happened, never more.

use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// How a file got to its destination. Undo reverses each differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// One same-device move. On macOS this is a single atomic renamex_np
    /// with RENAME_EXCL; on filesystems without it, link then unlink.
    /// Reversed the same way.
    Rename,
    /// copy → verify → unlink across devices. Reversed the same way.
    CopyUnlink,
    /// Finder tags added in place. Nothing moved: `from` and `to` are the same
    /// path, and the entry's [`TagChange`] says what to put back.
    Tag,
}

impl Method {
    fn tag(self) -> &'static str {
        match self {
            Method::Rename => "rename",
            Method::CopyUnlink => "copy",
            Method::Tag => "tag",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        match s {
            "rename" => Some(Method::Rename),
            "copy" => Some(Method::CopyUnlink),
            "tag" => Some(Method::Tag),
            _ => None,
        }
    }
}

/// What a tag-adding apply did to one file's tag attribute, as raw bytes.
///
/// `previous` is the attribute exactly as found (`None` when the file had
/// none) and is what undo puts back. `written` is the attribute exactly as
/// apply wrote it; undo acts only while the file still holds those bytes, so a
/// tag someone else changed in between is never overwritten.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagChange {
    pub previous: Option<Vec<u8>>,
    pub written: Vec<u8>,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || !text.is_ascii() {
        return None;
    }
    (0..text.len() / 2)
        .map(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok())
        .collect()
}

/// One recorded move, with the facts undo needs to verify before reversing.
#[derive(Debug, Clone)]
pub struct Entry {
    pub from: PathBuf,
    pub to: PathBuf,
    pub method: Method,
    /// Recorded at apply time, checked at undo time.
    pub size: u64,
    pub mtime_secs: i64,
    pub inode: u64,
    /// FNV-1a over the first and last 4 KiB. Change detection, **not**
    /// integrity. It defeats accident, not an adversary.
    pub edge_hash: u64,
    /// Where this entry stands. See [`EntryState`] — a bool used to carry
    /// three meanings and two of them shared a value.
    pub state: EntryState,
    /// Set only for a tag-adding entry ([`Method::Tag`]).
    pub tag: Option<TagChange>,
}

/// What has physically happened to one entry's file.
///
/// This was a `done: bool` until issue #7 needed undo to record its own
/// progress. Then `false` meant both "apply never moved this" and "undo
/// already moved it back" — the same value for opposite situations, and the
/// crash-recovery added for issue #5 reads the first not-yet-moved entry as
/// apply's crash point. With one bool, a resumed undo made that index mean
/// undo's progress boundary instead, and two correct features quietly
/// disagreed about what the field said.
///
/// On disk the base frame gains one value: `Reversed` writes `2` where
/// `Moved` writes `1` and `Planned` writes `0`. Journals written before this
/// only ever held 0 or 1 and load unchanged. An older binary reading a newer
/// journal tests `== "1"`, so it reads a 2 as not-moved — true, since the
/// file is at its origin; it loses only the knowledge that undo put it there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryState {
    /// Apply has not moved this file yet.
    Planned,
    /// Apply moved it. It is at the destination.
    Moved,
    /// Undo moved it back. It is at the origin, and undo is finished with it.
    Reversed,
    /// Apply left it where it was: it carried a Finder tag at move time, or
    /// would have crossed volumes and lost the tag. It never moved.
    Held,
    /// Apply moved it, found a tag that appeared during the move, and moved it
    /// straight back. It is at the origin and there is nothing to undo.
    Returned,
    /// Apply moved it, found a tag that appeared during the move, and could
    /// not move it back (the origin was occupied, or the return failed). It is
    /// at the destination, so undo treats it as moved.
    Stranded,
}

impl Entry {
    /// Whether the file is at its destination. The old `done` bit, kept as a
    /// question rather than a field so the two not-at-destination cases stay
    /// distinguishable.
    #[must_use]
    pub fn is_moved(&self) -> bool {
        matches!(self.state, EntryState::Moved | EntryState::Stranded)
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CreatedDirectory {
    pub path: PathBuf,
    pub device: u64,
    pub inode: u64,
}

/// A directory whose identity was validated before a journalled move.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ValidatedDirectory {
    pub path: PathBuf,
    pub device: u64,
    pub inode: u64,
}

#[derive(Debug, Default)]
pub struct Journal {
    pub id: String,
    /// Which tool wrote this. Journals are namespaced by tool because the
    /// etudes share one state directory: without it, `sweep undo` after a
    /// `stash` reverses the stash. Found the moment stash became real.
    pub tool: String,
    pub root: PathBuf,
    pub entries: Vec<Entry>,
    /// Destination directories this operation created, with their filesystem identity.
    pub created_directories: Vec<CreatedDirectory>,
    /// Directory identities captured before apply so undo can verify the
    /// folders it will move files back into. Older journals have no guards.
    pub validated_directories: Vec<ValidatedDirectory>,
    /// Set at load time when the progress records end mid-frame: a crash
    /// between a length prefix and its body leaves bytes describing a record
    /// that was never written. The frames before it are individually sealed
    /// and stay usable; this lets a caller say the tail was lost rather than
    /// imply the journal was whole.
    ///
    /// Not encoded: it describes the bytes that were on disk at load time,
    /// and `encode` writes a fresh base frame with no progress records.
    pub progress_tail_damaged: bool,
}

/// Path-free, read-only counts for an operation status lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JournalState {
    /// No entries: the apply was refused before it moved anything.
    Refused,
    Completed,
    Interrupted,
    Reversed,
    PartlyReversed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalSummary {
    pub id: String,
    pub tool: String,
    pub state: JournalState,
    pub planned: usize,
    pub moved: usize,
    pub reversed: usize,
}

impl Journal {
    /// Summarize decrypted state without exposing paths or changing the journal.
    pub fn summary(&self) -> JournalSummary {
        let planned = self
            .entries
            .iter()
            .filter(|entry| entry.state == EntryState::Planned)
            .count();
        let moved = self.entries.iter().filter(|entry| entry.is_moved()).count();
        let reversed = self
            .entries
            .iter()
            .filter(|entry| entry.state == EntryState::Reversed)
            .count();
        // Entries that were planned and never moved have nothing to put
        // back. Once nothing is still moved and something was reversed, the
        // operation is reversed; "partly" means some entries are still moved.
        // The only records written with no entries are for an apply refused
        // before it moved anything (see `apply::record_refused_plan_with_id`
        // and the stale live-plan path). An apply with nothing to move
        // returns before it writes a journal at all.
        let state = if self.entries.is_empty() {
            JournalState::Refused
        } else if reversed > 0 && moved == 0 {
            JournalState::Reversed
        } else if reversed > 0 {
            JournalState::PartlyReversed
        } else if planned == 0 {
            JournalState::Completed
        } else {
            JournalState::Interrupted
        };
        JournalSummary {
            id: self.id.clone(),
            tool: self.tool.clone(),
            state,
            planned,
            moved,
            reversed,
        }
    }
}

/// Supplied by the caller so `etude-core` need not depend on a cipher.
pub trait Sealer {
    fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>, &'static str>;
    fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, &'static str>;
}

#[derive(Debug)]
pub enum JournalError {
    Io(io::Error),
    Malformed(&'static str),
    NotFound,
    Seal(&'static str),
}

impl std::fmt::Display for JournalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // The OS text, not the Rust category. See ApplyError::Io.
            JournalError::Io(e) => write!(f, "journal io: {e}"),
            JournalError::Malformed(w) => write!(f, "journal malformed: {w}"),
            JournalError::NotFound => write!(f, "no journal found"),
            JournalError::Seal(m) => write!(f, "journal seal: {m}"),
        }
    }
}

/// Where journals live.
///
/// Source: Apple's own documentation for
/// `FileManager.SearchPathDirectory.applicationSupportDirectory`
/// (developer.apple.com), read directly rather than assumed -- per-user
/// application files belong at `~/Library/Application Support/<AppName>/`,
/// retrieved via `NSApplicationSupportDirectory` + `NSUserDomainMask`.
///
/// The previous default, `~/.local/state/etudes`, was an XDG (Linux)
/// convention on a macOS-only tool. Its own comment justified that as
/// "outside the default sync roots of iCloud Drive, Dropbox and OneDrive" --
/// true, but `~/Library` is equally outside those roots (iCloud
/// Desktop/Documents syncs `~/Desktop` and `~/Documents`, not `~/Library`),
/// so the reasoning never actually required an XDG path. Issue #23.
///
/// `XDG_STATE_HOME` is gone as a fallback: it was a second override doing
/// the same job as `ETUDE_STATE_DIR` while also being the Linux convention
/// this fix removes. `ETUDE_STATE_DIR` stays -- it is the primary
/// test-isolation mechanism across the suite, unrelated to which OS
/// convention the un-overridden default follows.
pub fn state_dir() -> PathBuf {
    if let Ok(x) = std::env::var("ETUDE_STATE_DIR") {
        return PathBuf::from(x);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join("Library/Application Support/etudes")
}

/// Where journals lived before issue #23. Only ever read by
/// `migrate_legacy_state_dir`, and only to move what is there forward once.
fn legacy_state_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".local/state/etudes")
}

/// One-time migration from the old XDG-style location to the new
/// Application Support one. A user's undo history is not something to
/// silently orphan by moving where the tool looks for it -- the same
/// principle behind the 30-day TTL being visible rather than quietly
/// dropping journals.
///
/// Only runs when nothing has already overridden `state_dir()`: an explicit
/// `ETUDE_STATE_DIR` means the caller has their own idea of where state
/// lives, and migrating a directory it never asked about would be a
/// surprise in the other direction.
///
/// Safe to call every time: it is a no-op unless the new location has never
/// been used and the old one has something in it, and it moves the whole
/// directory in one `fs::rename` rather than merging file-by-file, so there
/// is no partial-migration state to reason about.
pub fn migrate_legacy_state_dir() {
    if std::env::var("ETUDE_STATE_DIR").is_ok() {
        return;
    }
    let new = state_dir();
    let old = legacy_state_dir();
    if new.exists() || !old.exists() {
        return;
    }
    if let Some(parent) = new.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::rename(&old, &new);
}

fn esc(p: &Path) -> String {
    // Tabs and newlines are legal in filenames, so both must be escaped or the
    // line format is ambiguous. The fixture tree contains a tab on purpose.
    p.to_string_lossy()
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
}

fn unesc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('\\') => out.push('\\'),
            Some(other) => out.push(other),
            None => break,
        }
    }
    out
}

/// Validate an exact batch selector before consulting state or constructing a path.
pub fn validate_id(id: &str) -> Result<(), JournalError> {
    if valid_journal_id(id) {
        Ok(())
    } else {
        Err(JournalError::Malformed("invalid journal id"))
    }
}

/// Filename-safe legacy and current IDs; no separators, traversal or unbounded names.
pub fn valid_journal_id(id: &str) -> bool {
    valid_component(id, 128)
}

fn valid_component(value: &str, limit: usize) -> bool {
    !value.is_empty()
        && value.len() <= limit
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn append_progress(
    journal: &Journal,
    payload: &[u8],
    sealer: &dyn Sealer,
) -> Result<(), JournalError> {
    let sealed = sealer.seal(payload).map_err(JournalError::Seal)?;
    let len = u32::try_from(sealed.len())
        .map_err(|_| JournalError::Malformed("progress record too large"))?;
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(journal.path())
        .map_err(JournalError::Io)?;
    file.write_all(&len.to_le_bytes())
        .map_err(JournalError::Io)?;
    file.write_all(&sealed).map_err(JournalError::Io)?;
    file.sync_all().map_err(JournalError::Io)
}

impl Journal {
    pub fn path(&self) -> PathBuf {
        state_dir().join(format!("{}-{}.journal", self.tool, self.id))
    }

    /// Serialise. Line-oriented so a truncated write loses at most one entry.
    pub fn encode(&self) -> String {
        let mut s = String::new();
        if self.tool == "stash" {
            s.push_str("sweep-journal 2\n");
            s.push_str(&format!("identity\t{}\t{}\n", self.tool, self.id));
        } else {
            s.push_str("sweep-journal 1\n");
        }
        s.push_str(&format!("root\t{}\n", esc(&self.root)));
        for directory in &self.created_directories {
            s.push_str(&format!(
                "dir\t{}\t{}\t{}\n",
                esc(&directory.path),
                directory.device,
                directory.inode
            ));
        }
        for directory in &self.validated_directories {
            s.push_str(&format!(
                "guard\t{}\t{}\t{}\n",
                esc(&directory.path),
                directory.device,
                directory.inode
            ));
        }
        for e in &self.entries {
            s.push_str(&format!(
                "mv\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                esc(&e.from),
                esc(&e.to),
                e.method.tag(),
                e.size,
                e.mtime_secs,
                e.inode,
                e.edge_hash,
                match e.state {
                    EntryState::Planned => 0,
                    EntryState::Moved => 1,
                    // 2, so a reversal survives save_sealed. A review caught
                    // this encoding as 0 and reloading as Planned, which put
                    // an entry undo had already finished back in reach of
                    // apply's crash recovery — the exact collapse the three
                    // states exist to prevent.
                    //
                    // Readable by older binaries in the useful direction:
                    // their check is `== "1"`, so a 2 reads as not-moved, and
                    // the file really is at its origin. They lose only the
                    // knowledge that undo was what put it there.
                    EntryState::Reversed => 2,
                    // Written only if a journal that holds these states is
                    // saved whole. Apply records them as progress frames.
                    EntryState::Held => 3,
                    EntryState::Returned => 4,
                    EntryState::Stranded => 5,
                }
            ));
            // Tag entries append two fields. Journals without tag entries are
            // encoded exactly as before, and a reader that ignores trailing
            // fields sees the same nine.
            if let Some(change) = &e.tag {
                s.pop();
                s.push_str(&format!(
                    "\t{}\t{}\n",
                    change.previous.as_deref().map_or("-".to_string(), hex),
                    hex(&change.written)
                ));
            }
        }
        s
    }

    pub fn decode(text: &str) -> Result<Journal, JournalError> {
        let mut j = Journal::default();
        let mut lines = text.lines();
        match lines.next() {
            Some("sweep-journal 1") => {}
            Some("sweep-journal 2") => {
                let fields: Vec<_> = lines
                    .next()
                    .ok_or(JournalError::Malformed("missing journal identity"))?
                    .split('\t')
                    .collect();
                if fields.len() != 3 || fields[0] != "identity" || !valid_component(fields[1], 64) {
                    return Err(JournalError::Malformed("invalid journal identity"));
                }
                validate_id(fields[2])?;
                j.tool = fields[1].to_string();
                j.id = fields[2].to_string();
            }
            _ => return Err(JournalError::Malformed("unsupported journal header")),
        }
        for line in lines {
            let f: Vec<&str> = line.split('\t').collect();
            match f.first().copied() {
                Some("root") => {
                    j.root = PathBuf::from(unesc(f.get(1).copied().unwrap_or_default()))
                }
                Some("dir") if f.len() == 4 => {
                    j.created_directories.push(CreatedDirectory {
                        path: PathBuf::from(unesc(f[1])),
                        device: f[2]
                            .parse()
                            .map_err(|_| JournalError::Malformed("directory device"))?,
                        inode: f[3]
                            .parse()
                            .map_err(|_| JournalError::Malformed("directory inode"))?,
                    });
                }
                Some("guard") if f.len() == 4 => {
                    j.validated_directories.push(ValidatedDirectory {
                        path: PathBuf::from(unesc(f[1])),
                        device: f[2]
                            .parse()
                            .map_err(|_| JournalError::Malformed("directory guard device"))?,
                        inode: f[3]
                            .parse()
                            .map_err(|_| JournalError::Malformed("directory guard inode"))?,
                    });
                }
                Some("mv") => {
                    if f.len() < 9 {
                        return Err(JournalError::Malformed("short mv record"));
                    }
                    let method =
                        Method::parse(f[3]).ok_or(JournalError::Malformed("unknown method"))?;
                    let tag = if method == Method::Tag {
                        if f.len() < 11 {
                            return Err(JournalError::Malformed("short tag record"));
                        }
                        Some(TagChange {
                            previous: if f[9] == "-" {
                                None
                            } else {
                                Some(unhex(f[9]).ok_or(JournalError::Malformed("tag bytes"))?)
                            },
                            written: unhex(f[10]).ok_or(JournalError::Malformed("tag bytes"))?,
                        })
                    } else {
                        None
                    };
                    j.entries.push(Entry {
                        tag,
                        from: PathBuf::from(unesc(f[1])),
                        to: PathBuf::from(unesc(f[2])),
                        method,
                        size: f[4].parse().map_err(|_| JournalError::Malformed("size"))?,
                        mtime_secs: f[5].parse().map_err(|_| JournalError::Malformed("mtime"))?,
                        inode: f[6].parse().map_err(|_| JournalError::Malformed("inode"))?,
                        edge_hash: f[7].parse().map_err(|_| JournalError::Malformed("hash"))?,
                        // 0 covers both Planned and Reversed: the file is at
                        // its origin either way, and which of the two is
                        // reconstructed from the progress frames below.
                        state: match f[8] {
                            "1" => EntryState::Moved,
                            "2" => EntryState::Reversed,
                            "3" => EntryState::Held,
                            "4" => EntryState::Returned,
                            "5" => EntryState::Stranded,
                            // Anything else is Planned. Journals written
                            // before this only ever wrote 0 or 1.
                            _ => EntryState::Planned,
                        },
                    });
                }
                _ => {}
            }
        }
        Ok(j)
    }

    /// Persist, sealed. Written once before the first move (and whenever the
    /// caller wants a full rewrite). On disk the sealed blob is length-framed
    /// so [`Self::record_done`] can append progress frames to the same file.
    /// Rewriting the whole journal after every move was O(n²).
    pub fn save_sealed(&self, sealer: &dyn Sealer) -> Result<(), JournalError> {
        let bytes = sealer
            .seal(self.encode().as_bytes())
            .map_err(JournalError::Seal)?;
        self.write_bytes(&bytes)
    }

    /// Append one sealed "entry i finished via method" frame to the journal
    /// file. Apply walks entries in index order, so an in-order prefix of
    /// these frames is exactly the done-prefix of the entry list.
    pub fn record_done(
        &self,
        index: usize,
        method: Method,
        sealer: &dyn Sealer,
    ) -> Result<(), JournalError> {
        let payload = format!("done\t{index}\t{}\n", method.tag());
        let sealed = sealer
            .seal(payload.as_bytes())
            .map_err(JournalError::Seal)?;
        let len = u32::try_from(sealed.len())
            .map_err(|_| JournalError::Malformed("progress record too large"))?;
        // No create(true): the base frame must already exist from save_sealed.
        // Opening a missing journal here would paper over a real caller bug.
        let mut f = fs::OpenOptions::new()
            .append(true)
            .open(self.path())
            .map_err(JournalError::Io)?;
        f.write_all(&len.to_le_bytes()).map_err(JournalError::Io)?;
        f.write_all(&sealed).map_err(JournalError::Io)?;
        // Append to an existing file creates no new directory entry, so the
        // directory fsync from write_bytes already covers the name. sync_all
        // on the file is enough. That's why this is O(1).
        f.sync_all().map_err(JournalError::Io)?;
        Ok(())
    }

    /// Append one sealed "entry i was held and never moved" frame. It takes
    /// the place of a done frame in apply's in-order stream, so the stream of
    /// done frames stays a gap-free prefix.
    pub fn record_held(&self, index: usize, sealer: &dyn Sealer) -> Result<(), JournalError> {
        append_progress(self, format!("held\t{index}\n").as_bytes(), sealer)
    }

    /// Append one sealed "entry i was moved and straight back" frame. Written
    /// after the entry's done frame.
    pub fn record_returned(&self, index: usize, sealer: &dyn Sealer) -> Result<(), JournalError> {
        append_progress(self, format!("returned\t{index}\n").as_bytes(), sealer)
    }

    /// Append one sealed "entry i is tagged and could not be returned" frame.
    /// Written after the entry's done frame; the entry stays reversible.
    pub fn record_stranded(&self, index: usize, sealer: &dyn Sealer) -> Result<(), JournalError> {
        append_progress(self, format!("stranded\t{index}\n").as_bytes(), sealer)
    }

    /// Record a destination directory created by this apply operation.
    /// The no-follow identity lets undo remove only the same still-empty folder.
    pub fn record_created_dir(
        &mut self,
        path: &Path,
        device: u64,
        inode: u64,
        sealer: &dyn Sealer,
    ) -> Result<(), JournalError> {
        let metadata = fs::symlink_metadata(path).map_err(JournalError::Io)?;
        if !metadata.is_dir() {
            return Err(JournalError::Malformed(
                "created destination is not a directory",
            ));
        }
        #[cfg(unix)]
        let current_identity = {
            use std::os::unix::fs::MetadataExt;
            (metadata.dev(), metadata.ino())
        };
        #[cfg(not(unix))]
        let current_identity = (0, 0);
        if current_identity != (device, inode) {
            return Err(JournalError::Malformed(
                "created destination changed before it was recorded",
            ));
        }
        let record = CreatedDirectory {
            path: path.to_path_buf(),
            device,
            inode,
        };
        let payload = format!("created-dir\t{}\t{device}\t{inode}\n", esc(path));
        append_progress(self, payload.as_bytes(), sealer)?;
        if !self.created_directories.contains(&record) {
            self.created_directories.push(record);
        }
        Ok(())
    }

    /// Append one sealed "entry i has been reversed" frame.
    ///
    /// Undo's counterpart to [`Self::record_done`]. Undo walks entries in
    /// reverse index order, so a suffix of these frames with strictly
    /// decreasing indices is exactly what undo has already put back on disk.
    /// Without it a killed undo leaves the journal claiming every entry is
    /// still at its destination when some are already home, so the next run
    /// walks them all again and the journal never converges to "nothing left
    /// to undo".
    pub fn record_undone(&self, index: usize, sealer: &dyn Sealer) -> Result<(), JournalError> {
        let payload = format!("undone\t{index}\n");
        let sealed = sealer
            .seal(payload.as_bytes())
            .map_err(JournalError::Seal)?;
        let len = u32::try_from(sealed.len())
            .map_err(|_| JournalError::Malformed("progress record too large"))?;
        // Same append-only, no-create discipline as record_done: the base
        // frame must already be there.
        let mut f = fs::OpenOptions::new()
            .append(true)
            .open(self.path())
            .map_err(JournalError::Io)?;
        f.write_all(&len.to_le_bytes()).map_err(JournalError::Io)?;
        f.write_all(&sealed).map_err(JournalError::Io)?;
        f.sync_all().map_err(JournalError::Io)?;
        Ok(())
    }

    /// Load and unseal. `id` is the bare id; `tool` selects the namespace.
    ///
    /// On-disk layout is length-framed: a required base frame (sealed encode
    /// of all entries), then zero or more sealed progress frames appended by
    /// [`Self::record_done`].
    ///
    /// A progress frame is only ever appended *after* the move it describes
    /// has already succeeded (see `apply`'s ordering rule), so a partially
    /// written trailing frame is not "work that never happened". It is
    /// proof the write was interrupted after the real move, not evidence the
    /// move didn't occur. Silently discarding it would misreport a completed
    /// move as pending, which strands a file: `undo` would see an entry that
    /// is not moved and skip a file that is in fact sitting at its
    /// destination.
    ///
    /// A damaged tail therefore stops the replay and sets
    /// `progress_tail_damaged`, keeping every complete frame before it. It
    /// used to refuse the whole load; that traded one stranding for a larger
    /// one, since voiding 143 intact records over a 4-byte tail strands every
    /// file they describe. The entry whose record was cut stays Planned, and
    /// undo recovers it by looking at the filesystem.
    ///
    /// A *clean* end of file (the boundary between two intact frames,
    /// including "no progress written yet") is not damage and loads normally
    /// -- which is why undo cannot rely on this flag alone, and counts
    /// moved-but-unrecorded entries on disk as well.
    ///
    /// Journals written before length-framing (a single unframed sealed blob)
    /// are still accepted. Framed parse is tried first; any failure falls back
    /// to opening the whole file as one sealed blob (the pre-framing format).
    pub fn load_sealed(tool: &str, id: &str, sealer: &dyn Sealer) -> Result<Journal, JournalError> {
        Self::load_sealed_exact(tool, id, sealer)
    }

    /// Load one identity directly without timestamp discovery or unrelated journals.
    pub fn load_sealed_exact(
        tool: &str,
        id: &str,
        sealer: &dyn Sealer,
    ) -> Result<Journal, JournalError> {
        validate_id(id)?;
        if !valid_component(tool, 64) {
            return Err(JournalError::Malformed("invalid journal tool"));
        }
        let path = state_dir().join(format!("{tool}-{id}.journal"));
        let metadata =
            crate::scan::observe_read("metadata", fs::symlink_metadata(&path)).map_err(|e| {
                if e.kind() == io::ErrorKind::NotFound {
                    JournalError::NotFound
                } else {
                    JournalError::Io(e)
                }
            })?;
        JournalCandidate {
            path,
            metadata,
            id: id.to_string(),
        }
        .load(tool, sealer)
    }

    fn decode_sealed(
        tool: &str,
        id: &str,
        raw: &[u8],
        sealer: &dyn Sealer,
    ) -> Result<Journal, JournalError> {
        // Current format: 4-byte LE length + sealed base (+ optional progress).
        if raw.len() >= 4 {
            let base_len = u32::from_le_bytes(raw[0..4].try_into().unwrap()) as usize;
            if 4 + base_len <= raw.len()
                && let Ok(plain) = sealer.open(&raw[4..4 + base_len])
                && let Ok(text) = String::from_utf8(plain)
                && let Ok(mut j) = Journal::decode(&text)
            {
                j.bind_requested_identity(tool, id)?;
                j.apply_progress(&raw[4 + base_len..], sealer)?;
                return Ok(j);
            }
        }

        // Legacy: whole file is one unframed sealed blob (no progress frames).
        let plain = sealer.open(raw).map_err(JournalError::Seal)?;
        let text = String::from_utf8(plain).map_err(|_| JournalError::Malformed("not utf-8"))?;
        let mut j = Journal::decode(&text)?;
        j.bind_requested_identity(tool, id)?;
        Ok(j)
    }

    fn bind_requested_identity(&mut self, tool: &str, id: &str) -> Result<(), JournalError> {
        if (!self.id.is_empty() || !self.tool.is_empty()) && (self.id != id || self.tool != tool) {
            return Err(JournalError::Malformed(
                "journal identity does not match selector",
            ));
        }
        self.id = id.to_string();
        self.tool = tool.to_string();
        Ok(())
    }

    /// Replay length-framed progress records that follow the base frame.
    ///
    /// Every record here was written by a completed move (ordering rule in
    /// `apply`), so a frame that is COMPLETE but fails to authenticate is
    /// refused rather than dropped: that is alteration, and dropping it would
    /// silently downgrade a finished move to "not done" and strand the file.
    ///
    /// A frame that is merely CUT SHORT is different. It was never finished
    /// being written, so it records nothing, and the entry it would have
    /// described is left Planned for undo's successor-entry recovery. Replay
    /// stops there and `progress_tail_damaged` is set; the complete frames
    /// before it are each sealed independently and stay usable.
    fn apply_progress(&mut self, raw: &[u8], sealer: &dyn Sealer) -> Result<(), JournalError> {
        let mut offset = 0usize;
        // Two cursors, because two writers append here. Apply counts up from
        // 0; undo counts down from the last entry it reversed. The first
        // undone frame may land at any index — undo starts at the highest
        // entry that was actually moved — so the cursor is set on first sight
        // and strict from then on.
        let mut expected_done = 0usize;
        let mut expected_undone: Option<usize> = None;
        while offset < raw.len() {
            if offset + 4 > raw.len() {
                // A partial length prefix: nothing readable, and nothing
                // before it is affected. See the note on the body check.
                self.progress_tail_damaged = true;
                return Ok(());
            }
            let len = u32::from_le_bytes(raw[offset..offset + 4].try_into().unwrap()) as usize;
            offset += 4;
            if offset + len > raw.len() {
                // TRUNCATION, which is not the same as damage. This
                // distinction is the fix.
                //
                // Refusing the whole journal here was deliberate, and the
                // reasoning went: dropping a frame downgrades a finished
                // move to "not done", so undo skips the file and strands it
                // without a word.
                //
                // But a truncated tail is not a finished move being dropped.
                // It is a record that never finished being written. Apply
                // appends one sealed frame per completed move, so a partial
                // frame means the crash landed inside that write: the entry
                // it would have described is still Planned, and undo's
                // successor-entry recovery covers exactly that entry.
                //
                // Every earlier frame was sealed and authenticated on its
                // own, so a whole, authentic frame is trustworthy whatever
                // follows it. Voiding 143 of them over 4 trailing bytes
                // strands every file they describe. A CI artifact showed
                // exactly that, and a controlled repeat confirmed it: the
                // same journal restored 100 files with the 4 bytes removed
                // and stranded all 100 with them present.
                //
                // A COMPLETE frame that fails to authenticate is a different
                // fact -- alteration, not an interrupted write -- and is
                // still refused, below.
                self.progress_tail_damaged = true;
                return Ok(());
            }
            let sealed = &raw[offset..offset + len];
            offset += len;
            let plain = sealer.open(sealed).map_err(|_| {
                JournalError::Malformed("journal damaged: progress record failed to authenticate")
            })?;
            let text = std::str::from_utf8(&plain).map_err(|_| {
                JournalError::Malformed("journal damaged: progress record not utf-8")
            })?;
            let text = text.trim_end_matches('\n');
            let f: Vec<&str> = text.split('\t').collect();
            match f.first().copied() {
                Some("created-dir") if f.len() == 4 => {
                    let directory = CreatedDirectory {
                        path: PathBuf::from(unesc(f[1])),
                        device: f[2]
                            .parse()
                            .map_err(|_| JournalError::Malformed("created directory device"))?,
                        inode: f[3]
                            .parse()
                            .map_err(|_| JournalError::Malformed("created directory inode"))?,
                    };
                    if !self.created_directories.contains(&directory) {
                        self.created_directories.push(directory);
                    }
                }
                // Apply's frames, written 0..n as it walks forward.
                Some("done") if f.len() == 3 => {
                    let index: usize = f[1].parse().map_err(|_| {
                        JournalError::Malformed("journal damaged: progress record index")
                    })?;
                    let method = Method::parse(f[2]).ok_or(JournalError::Malformed(
                        "journal damaged: progress record method",
                    ))?;
                    if index != expected_done || index >= self.entries.len() {
                        return Err(JournalError::Malformed(
                            "journal damaged: progress record out of order",
                        ));
                    }
                    self.entries[index].state = EntryState::Moved;
                    self.entries[index].method = method;
                    expected_done += 1;
                }
                // Apply's frames for an entry it did not leave moved. `held`
                // stands in for the done frame; `returned` and `stranded`
                // follow the done frame of an entry that moved.
                Some("held") if f.len() == 2 => {
                    let index: usize = f[1].parse().map_err(|_| {
                        JournalError::Malformed("journal damaged: progress record index")
                    })?;
                    if index != expected_done
                        || index >= self.entries.len()
                        || self.entries[index].state != EntryState::Planned
                    {
                        return Err(JournalError::Malformed(
                            "journal damaged: progress record out of order",
                        ));
                    }
                    self.entries[index].state = EntryState::Held;
                    expected_done += 1;
                }
                Some(kind @ ("returned" | "stranded")) if f.len() == 2 => {
                    let index: usize = f[1].parse().map_err(|_| {
                        JournalError::Malformed("journal damaged: progress record index")
                    })?;
                    if index >= self.entries.len() || self.entries[index].state != EntryState::Moved
                    {
                        return Err(JournalError::Malformed(
                            "journal damaged: a return for an entry that was not moved",
                        ));
                    }
                    self.entries[index].state = if kind == "returned" {
                        EntryState::Returned
                    } else {
                        EntryState::Stranded
                    };
                }
                // Undo's frames, written n-1..0 as it walks back. Its own
                // cursor, counting down, so an out-of-order frame of either
                // kind is still caught rather than trusted.
                Some("undone") if f.len() == 2 => {
                    let index: usize = f[1].parse().map_err(|_| {
                        JournalError::Malformed("journal damaged: progress record index")
                    })?;
                    // Strictly decreasing, with holes allowed. Undo walks
                    // backwards but writes no frame for an entry it skips as
                    // changed or missing, so 2 then 0 is a normal stream. A
                    // review caught the first version demanding index - 1
                    // exactly, which would have failed to load a journal from
                    // any run where a skip sat between two reversals — the
                    // runs where these frames matter most.
                    //
                    // Duplicates and replays are still caught by the check
                    // below that the entry is currently Moved.
                    if index >= self.entries.len() || expected_undone.is_some_and(|e| index >= e) {
                        return Err(JournalError::Malformed(
                            "journal damaged: progress record out of order",
                        ));
                    }
                    // Undo can only reverse what apply moved. An undone frame
                    // for an entry that is not currently Moved means the
                    // stream is inconsistent — replayed out of order, or two
                    // undone frames for one entry — and a review asked for it
                    // to be caught rather than absorbed.
                    // A tag entry whose write landed but whose done frame was
                    // lost is still Planned here; undo restores it by reading
                    // the file, so its reversal frame is legitimate.
                    let entry = &self.entries[index];
                    let unrecorded_tag_write =
                        entry.tag.is_some() && entry.state == EntryState::Planned;
                    if !entry.is_moved() && !unrecorded_tag_write {
                        return Err(JournalError::Malformed(
                            "journal damaged: a reversal for an entry that was not moved",
                        ));
                    }
                    // Reversed, not Planned. The file is at its origin either
                    // way, but only Reversed says undo already handled it, and
                    // apply's crash recovery keys off the first Planned entry.
                    self.entries[index].state = EntryState::Reversed;
                    expected_undone = Some(index);
                }
                _ => {
                    return Err(JournalError::Malformed(
                        "journal damaged: progress record has the wrong shape",
                    ));
                }
            }
        }
        Ok(())
    }

    /// Most recent sealed journal **written by `tool`**, by modification time.
    pub fn latest_sealed(tool: &str, sealer: &dyn Sealer) -> Result<Journal, JournalError> {
        candidates_by_recency(tool)?
            .into_iter()
            .next()
            .ok_or(JournalError::NotFound)?
            .load(tool, sealer)
    }

    fn write_bytes(&self, bytes: &[u8]) -> Result<(), JournalError> {
        let dir = state_dir();
        fs::create_dir_all(&dir).map_err(JournalError::Io)?;
        // 0700, not 0600: a directory needs the execute bit to be entered, so
        // 0600 makes it impossible to create the journal inside it.
        restrict_dir(&dir);
        let p = self.path();
        // Write to a temp file in the same directory, then rename: a crash
        // mid-write cannot leave a half-parsed journal. The rename also
        // atomically discards any previously-appended progress frames. There
        // is no separate truncate step, so a crash cannot pair a new base with
        // stale progress that would over-claim on the next load.
        let tmp = p.with_extension("journal.tmp");
        let mut f = fs::File::create(&tmp).map_err(JournalError::Io)?;
        restrict(&tmp);
        let len =
            u32::try_from(bytes.len()).map_err(|_| JournalError::Malformed("journal too large"))?;
        f.write_all(&len.to_le_bytes()).map_err(JournalError::Io)?;
        f.write_all(bytes).map_err(JournalError::Io)?;
        f.sync_all().map_err(JournalError::Io)?;
        drop(f);
        fs::rename(&tmp, &p).map_err(JournalError::Io)?;
        // fsyncing the file's data does not make the rename durable: the
        // directory entry that makes the new name visible is separate on-disk
        // state. Without syncing the parent, a crash right after rename can
        // leave the entry pointing at the old file (or nothing) even though
        // the new bytes are safely on disk.
        sync_dir(p.parent().unwrap_or(&dir))?;
        Ok(())
    }

    pub fn forget(&self) -> Result<(), JournalError> {
        fs::remove_file(self.path()).map_err(JournalError::Io)
    }
}

/// Journals older than this are dropped on the next run.
///
/// A journal is an index of the user's filenames. Even sealed, keeping it
/// forever means keeping the exposure forever, and undo is only useful for as
/// long as the user remembers what they ran.
pub const TTL_DAYS: u64 = 30;

/// Delete journals older than [`TTL_DAYS`]. Returns how many were removed.
///
/// Called at the start of every run that touches the state directory. Failure
/// is not fatal: a journal that cannot be removed is a housekeeping problem,
/// not a reason to refuse the user's actual request.
pub fn prune_expired() -> usize {
    let cutoff =
        match SystemTime::now().checked_sub(std::time::Duration::from_secs(TTL_DAYS * 86_400)) {
            Some(c) => c,
            None => return 0,
        };
    let Ok(rd) = crate::scan::observe_read("directory_enumeration", fs::read_dir(state_dir()))
    else {
        return 0;
    };
    let mut removed = 0;
    for e in rd.flatten() {
        if !e.file_name().to_string_lossy().ends_with(".journal") {
            continue;
        }
        let Ok(md) = e.metadata() else { continue };
        let Ok(t) = md.modified() else { continue };
        if t < cutoff && fs::remove_file(e.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// Newest journal id written by `tool`, by modification time.
pub fn latest_id(tool: &str) -> Result<String, JournalError> {
    ids_by_recency(tool)?
        .into_iter()
        .next()
        .ok_or(JournalError::NotFound)
}

/// Discovery records the exact regular file whose timestamp establishes order.
/// Loading validates that identity on the opened descriptor and again after
/// reading; a replaced path or changed file cannot supply different history.
pub struct JournalCandidate {
    path: PathBuf,
    metadata: fs::Metadata,
    pub id: String,
}

fn same_journal(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        a.is_file()
            && b.is_file()
            && a.dev() == b.dev()
            && a.ino() == b.ino()
            && a.len() == b.len()
            && a.mtime() == b.mtime()
            && a.mtime_nsec() == b.mtime_nsec()
            && a.ctime() == b.ctime()
            && a.ctime_nsec() == b.ctime_nsec()
    }
    #[cfg(not(unix))]
    {
        let _ = (a, b);
        false
    }
}

impl JournalCandidate {
    pub fn load(&self, tool: &str, sealer: &dyn Sealer) -> Result<Journal, JournalError> {
        let changed = || JournalError::Malformed("journal changed after discovery");
        if !self.metadata.is_file() {
            return Err(changed());
        }
        let mut options = fs::OpenOptions::new();
        options.read(true);
        // Do not follow a replacement symlink or block on a replacement FIFO.
        #[cfg(target_os = "macos")]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // Darwin SDK sys/fcntl.h.
            const O_NOFOLLOW: i32 = 0x100;
            const O_NONBLOCK: i32 = 0x4;
            options.custom_flags(O_NOFOLLOW | O_NONBLOCK);
        }
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(0x20000 | 0x800);
        }
        let mut file = crate::scan::observe_read("file_open", options.open(&self.path))
            .map_err(JournalError::Io)?;
        if !same_journal(
            &self.metadata,
            &crate::scan::observe_read("metadata", file.metadata()).map_err(JournalError::Io)?,
        ) {
            return Err(changed());
        }
        let mut raw = Vec::new();
        crate::scan::observe_read("journal_bytes", file.read_to_end(&mut raw))
            .map_err(JournalError::Io)?;
        if !same_journal(
            &self.metadata,
            &crate::scan::observe_read("metadata", file.metadata()).map_err(JournalError::Io)?,
        ) || !same_journal(
            &self.metadata,
            &crate::scan::observe_read("metadata", fs::symlink_metadata(&self.path))
                .map_err(JournalError::Io)?,
        ) {
            return Err(changed());
        }
        let decoded = Journal::decode_sealed(tool, &self.id, &raw, sealer);
        if decoded.is_ok() {
            crate::scan::verify_read("journal_bytes");
        }
        decoded
    }
}

/// Every id for journals written by `tool`, newest-first by modification time.
/// `latest_id` only ever returns the single most recent one; pop needs to search
/// across all of them to find the journal for a specific folder, not just the
/// newest.
pub fn ids_by_recency(tool: &str) -> Result<Vec<String>, JournalError> {
    Ok(candidates_by_recency(tool)?
        .into_iter()
        .map(|candidate| candidate.id)
        .collect())
}

pub fn candidates_by_recency(tool: &str) -> Result<Vec<JournalCandidate>, JournalError> {
    let mut journals = candidates_with_times(tool)?;
    journals.sort_by(|(a, _), (b, _)| b.cmp(a));
    if journals.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return Err(JournalError::Malformed(
            "journal modification times are ambiguous",
        ));
    }
    Ok(journals
        .into_iter()
        .map(|(_, candidate)| candidate)
        .collect())
}

/// Inventory exact candidates without imposing mutable timestamp order or ties.
pub fn candidates_unordered(tool: &str) -> Result<Vec<JournalCandidate>, JournalError> {
    Ok(candidates_with_times(tool)?
        .into_iter()
        .map(|(_, candidate)| candidate)
        .collect())
}

fn candidates_with_times(tool: &str) -> Result<Vec<(SystemTime, JournalCandidate)>, JournalError> {
    let prefix = format!("{tool}-");
    let dir = state_dir();
    let mut journals = Vec::new();
    let entries =
        crate::scan::observe_read("directory_enumeration", fs::read_dir(&dir)).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                JournalError::NotFound
            } else {
                JournalError::Io(e)
            }
        })?;
    for e in entries {
        let e = e.map_err(JournalError::Io)?;
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(stem) = name.strip_suffix(".journal") else {
            continue;
        };
        let Some(id) = stem.strip_prefix(&prefix) else {
            continue;
        };
        // A symlink, directory, or missing metadata cannot establish a
        // journal's position in history. Never silently omit that barrier.
        let md = crate::scan::observe_read("metadata", fs::symlink_metadata(e.path()))
            .map_err(JournalError::Io)?;
        if !md.is_file() {
            return Err(JournalError::Malformed("journal is not a regular file"));
        }
        let t = md.modified().map_err(JournalError::Io)?;
        journals.push((
            t,
            JournalCandidate {
                path: e.path(),
                metadata: md,
                id: id.to_string(),
            },
        ));
    }
    if journals.is_empty() {
        return Err(JournalError::NotFound);
    }
    Ok(journals)
}

#[cfg(test)]
static SYNC_DIR_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[cfg(unix)]
fn restrict(p: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(p, fs::Permissions::from_mode(0o600));
}
#[cfg(unix)]
fn restrict_dir(p: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(p, fs::Permissions::from_mode(0o700));
}
#[cfg(unix)]
fn sync_dir(dir: &Path) -> Result<(), JournalError> {
    #[cfg(test)]
    SYNC_DIR_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let d =
        crate::scan::observe_read("file_open", fs::File::open(dir)).map_err(JournalError::Io)?;
    d.sync_all().map_err(JournalError::Io)
}
#[cfg(not(unix))]
fn restrict(_p: &Path) {}
#[cfg(not(unix))]
fn restrict_dir(_p: &Path) {}
#[cfg(not(unix))]
fn sync_dir(_dir: &Path) -> Result<(), JournalError> {
    Ok(())
}

/// Facts about a file, captured at apply time and re-checked at undo time.
///
/// Package directories (`.app`, `.photoslibrary`) are moved as single units, so
/// this must accept a directory. There is nothing to hash. Opening a directory
/// fails. The edge hash is then 0. Identity rests on inode and mtime instead.
/// Found by
/// stash, which moves everything including packages; sweep would have hit the
/// same panic the first time a `.app` landed in a group.
pub fn fingerprint(p: &Path) -> io::Result<(u64, i64, u64, u64)> {
    // symlink_metadata, not metadata: a symlink must be identified by the LINK,
    // never by its target. Following it makes the link's fingerprint change
    // whenever the target does. stash found this with a link pointing at the
    // folder being emptied. It reported the link as modified and refused to
    // restore it.
    let md = crate::scan::observe_read("metadata", fs::symlink_metadata(p))?;
    let size = md.len();
    // Neither a directory nor a symlink can be opened for hashing.
    let opaque = md.is_dir() || md.file_type().is_symlink();
    let mtime = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    #[cfg(unix)]
    let inode = {
        use std::os::unix::fs::MetadataExt;
        md.ino()
    };
    #[cfg(not(unix))]
    let inode = 0u64;
    let hash = if opaque { 0 } else { edge_hash(p, size)? };
    Ok((size, mtime, inode, hash))
}

/// FNV-1a over the first and last 4 KiB.
///
/// This detects accidental change: an edit, a replacement, a different file at
/// the same path. It is **not** an integrity check and must never be described
/// as one: an adversary can trivially preserve it.
pub fn edge_hash(p: &Path, size: u64) -> io::Result<u64> {
    const EDGE: u64 = 4096;
    let mut f = crate::scan::observe_read("file_open", fs::File::open(p))?;
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let feed = |buf: &[u8], h: &mut u64| {
        for b in buf {
            *h ^= *b as u64;
            *h = h.wrapping_mul(0x100_0000_01b3);
        }
    };

    let head_len = size.min(EDGE) as usize;
    let mut head = vec![0u8; head_len];
    crate::scan::observe_read("fingerprint_bytes", f.read_exact(&mut head))?;
    feed(&head, &mut h);

    if size > EDGE {
        let tail_len = EDGE.min(size - EDGE) as usize;
        f.seek(SeekFrom::End(-(tail_len as i64)))?;
        let mut tail = vec![0u8; tail_len];
        crate::scan::observe_read("fingerprint_bytes", f.read_exact(&mut tail))?;
        feed(&tail, &mut h);
    }
    // Length is part of the identity, so truncation alone changes the hash.
    feed(&size.to_le_bytes(), &mut h);
    Ok(h)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn golden_journal(tool: &str) -> Journal {
        let states = [
            EntryState::Planned,
            EntryState::Moved,
            EntryState::Reversed,
            EntryState::Held,
            EntryState::Returned,
            EntryState::Stranded,
        ];
        Journal {
            id: "golden-id".into(),
            tool: tool.into(),
            root: PathBuf::from("/golden/root"),
            entries: states
                .into_iter()
                .enumerate()
                .map(|(i, state)| Entry {
                    from: PathBuf::from(format!("/golden/root/f{i}.txt")),
                    to: PathBuf::from(format!("/golden/root/Group/f{i}.txt")),
                    method: if i % 2 == 0 {
                        Method::Rename
                    } else {
                        Method::CopyUnlink
                    },
                    size: 10 + i as u64,
                    mtime_secs: 1000 + i as i64,
                    inode: 77 + i as u64,
                    edge_hash: 9000 + i as u64,
                    state,
                    tag: None,
                })
                .collect(),
            created_directories: vec![CreatedDirectory {
                path: PathBuf::from("/golden/root/Group"),
                device: 5,
                inode: 6,
            }],
            validated_directories: vec![ValidatedDirectory {
                path: PathBuf::from("/golden/root"),
                device: 5,
                inode: 4,
            }],
            progress_tail_damaged: false,
        }
    }

    /// Journals that move files encode exactly as they did before tag entries
    /// existed. The digests were taken from the encoding of the previous
    /// release for the same journal.
    #[test]
    fn journals_without_tag_entries_keep_their_earlier_encoding_byte_for_byte() {
        for (tool, length, digest) in [
            (
                "sweep",
                522,
                "d5f76d76ca13b338b52a0e343619e667c1a737cfbeb7dea476a7440586f38774",
            ),
            (
                "stash",
                547,
                "4ea85b2760ddf885700c375fe7a3917b3132a2d5328e553bc0832324b18e6b4e",
            ),
        ] {
            let text = golden_journal(tool).encode();
            assert_eq!(text.len(), length, "{tool}");
            assert_eq!(
                crate::plan::binding_digest(text.as_bytes()),
                digest,
                "{tool}"
            );
            // And it still loads to the same journal.
            let loaded = Journal::decode(&text).expect("an earlier journal loads");
            assert_eq!(loaded.entries.len(), 6);
            assert!(loaded.entries.iter().all(|e| e.tag.is_none()));
            assert_eq!(loaded.encode(), text);
        }
    }

    #[test]
    fn a_tag_entry_round_trips_with_its_previous_and_written_bytes() {
        let mut journal = golden_journal("sweep");
        journal.entries.truncate(2);
        for (i, entry) in journal.entries.iter_mut().enumerate() {
            entry.method = Method::Tag;
            entry.to = entry.from.clone();
            entry.tag = Some(TagChange {
                previous: if i == 0 {
                    None
                } else {
                    Some(vec![0, 1, 254, 255])
                },
                written: vec![0x62, 0x70, 0x6c, 0x69, 0x73, 0x74, i as u8],
            });
        }
        let text = journal.encode();
        let loaded = Journal::decode(&text).expect("loads");
        assert_eq!(loaded.entries[0].tag, journal.entries[0].tag);
        assert_eq!(loaded.entries[1].tag, journal.entries[1].tag);
        assert_eq!(loaded.entries[0].method, Method::Tag);
        assert_eq!(loaded.encode(), text);
        // A tag record that lost its fields is refused, not read as a move.
        let short = text.lines().map(|l| {
            if l.starts_with("mv\t") {
                l.split('\t').take(9).collect::<Vec<_>>().join("\t")
            } else {
                l.to_string()
            }
        });
        let short: Vec<String> = short.collect();
        assert!(Journal::decode(&short.join("\n")).is_err());
    }

    use std::sync::Mutex;

    // ETUDE_STATE_DIR is process-global; serialize tests that touch it.
    static STATE_DIR_LOCK: Mutex<()> = Mutex::new(());

    /// A record with no entries is written only when an apply is refused
    /// before it moves anything. Nothing was planned into it and nothing
    /// moved, so calling it `Completed` would tell a caller the work was done.
    #[test]
    fn a_record_with_no_entries_is_refused_not_completed() {
        let summary = Journal {
            id: "durable-id".into(),
            tool: "sweep".into(),
            ..Journal::default()
        }
        .summary();
        assert_eq!(summary.state, JournalState::Refused);
        assert_eq!(
            (summary.planned, summary.moved, summary.reversed),
            (0, 0, 0)
        );
    }

    /// An apply cut short leaves entries that were planned and never moved.
    /// Undoing it reverses everything that did move, so nothing is left to
    /// put back: that is `Reversed`. "Partly" is only for entries still moved.
    #[test]
    fn undoing_an_interrupted_apply_is_reversed_not_partly_reversed() {
        let entry = |state| Entry {
            from: PathBuf::from("/private/source/name"),
            to: PathBuf::from("/private/destination/name"),
            method: Method::Rename,
            size: 1,
            mtime_secs: 1,
            inode: 1,
            edge_hash: 1,
            state,

            tag: None,
        };
        let summarize = |states: &[EntryState]| {
            Journal {
                id: "durable-id".into(),
                tool: "sweep".into(),
                entries: states.iter().copied().map(entry).collect(),
                ..Journal::default()
            }
            .summary()
        };

        let undone = summarize(&[
            EntryState::Reversed,
            EntryState::Planned,
            EntryState::Planned,
        ]);
        assert_eq!(undone.state, JournalState::Reversed);
        assert_eq!((undone.planned, undone.moved, undone.reversed), (2, 0, 1));

        let still_moved =
            summarize(&[EntryState::Reversed, EntryState::Moved, EntryState::Planned]);
        assert_eq!(still_moved.state, JournalState::PartlyReversed);
        assert_eq!(
            (still_moved.planned, still_moved.moved, still_moved.reversed),
            (1, 1, 1)
        );
    }

    #[test]
    fn journal_summary_reports_path_free_states_and_exact_entry_counts() {
        let entry = |state| Entry {
            from: PathBuf::from("/private/source/name"),
            to: PathBuf::from("/private/destination/name"),
            method: Method::Rename,
            size: 1,
            mtime_secs: 1,
            inode: 1,
            edge_hash: 1,
            state,

            tag: None,
        };
        let summarize = |states: &[EntryState]| {
            Journal {
                id: "durable-id".into(),
                tool: "sweep".into(),
                entries: states.iter().copied().map(entry).collect(),
                ..Journal::default()
            }
            .summary()
        };

        let completed = summarize(&[EntryState::Moved, EntryState::Moved]);
        assert_eq!(completed.state, JournalState::Completed);
        assert_eq!(
            (completed.planned, completed.moved, completed.reversed),
            (0, 2, 0)
        );

        let interrupted = summarize(&[EntryState::Planned, EntryState::Moved]);
        assert_eq!(interrupted.state, JournalState::Interrupted);
        assert_eq!(
            (interrupted.planned, interrupted.moved, interrupted.reversed),
            (1, 1, 0)
        );

        let reversed = summarize(&[EntryState::Reversed, EntryState::Reversed]);
        assert_eq!(reversed.state, JournalState::Reversed);
        assert_eq!(
            (reversed.planned, reversed.moved, reversed.reversed),
            (0, 0, 2)
        );

        let partly_reversed = summarize(&[EntryState::Reversed, EntryState::Moved]);
        assert_eq!(partly_reversed.state, JournalState::PartlyReversed);
        assert_eq!(
            (
                partly_reversed.planned,
                partly_reversed.moved,
                partly_reversed.reversed
            ),
            (0, 1, 1)
        );
        assert_eq!(
            (completed.id.as_str(), completed.tool.as_str()),
            ("durable-id", "sweep")
        );
    }

    #[test]
    #[cfg(unix)]
    fn discovery_binds_the_file_subsequently_read() {
        use std::os::unix::fs::symlink;
        let _guard = STATE_DIR_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("journal_identity_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let _env = RestoreEnvVar::set("ETUDE_STATE_DIR", &dir);
        struct Identity;
        impl Sealer for Identity {
            fn seal(&self, p: &[u8]) -> Result<Vec<u8>, &'static str> {
                Ok(p.to_vec())
            }
            fn open(&self, p: &[u8]) -> Result<Vec<u8>, &'static str> {
                Ok(p.to_vec())
            }
        }
        let journal = Journal {
            id: "identity".into(),
            tool: "test".into(),
            root: dir.join("root"),
            ..Journal::default()
        };
        for fault in [
            "unchanged",
            "replace",
            "rewrite",
            "symlink",
            "missing",
            "fifo",
        ] {
            let path = journal.path();
            let _ = fs::remove_file(&path);
            journal.save_sealed(&Identity).unwrap();
            let candidate = candidates_by_recency("test").unwrap().remove(0);
            let bytes = fs::read(&path).unwrap();
            match fault {
                "replace" => {
                    let replacement = dir.join("replacement");
                    fs::write(&replacement, &bytes).unwrap();
                    fs::rename(replacement, &path).unwrap();
                }
                "rewrite" => {
                    // Same length and valid framing: metadata must detect it,
                    // rather than relying on a later decode error.
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    fs::write(&path, &bytes).unwrap();
                }
                "symlink" => {
                    let target = dir.join("target");
                    fs::write(&target, &bytes).unwrap();
                    fs::remove_file(&path).unwrap();
                    symlink(target, &path).unwrap();
                }
                "missing" => fs::remove_file(&path).unwrap(),
                "fifo" => {
                    fs::remove_file(&path).unwrap();
                    assert!(
                        std::process::Command::new("mkfifo")
                            .arg(&path)
                            .status()
                            .unwrap()
                            .success()
                    );
                }
                _ => {}
            }
            let result = candidate.load("test", &Identity);
            if fault == "unchanged" {
                assert_eq!(result.unwrap().root, journal.root);
            } else {
                assert!(result.is_err(), "{fault} must refuse before decoding");
            }
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn expired_journals_are_pruned_and_fresh_ones_are_kept() {
        let _guard = STATE_DIR_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("sweep_ttl_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("mkdir");
        unsafe { std::env::set_var("ETUDE_STATE_DIR", &dir) };

        let old = dir.join("old.journal");
        let new = dir.join("new.journal");
        fs::write(&old, b"x").expect("write");
        fs::write(&new, b"x").expect("write");

        // Backdate one past the TTL. filetime is not in the dependency tree, so
        // this uses the libc utimes we already link.
        let past = (TTL_DAYS + 5) * 86_400;
        set_mtime_secs_ago(&old, past);

        let removed = prune_expired();

        assert_eq!(removed, 1, "prune removed {removed} journals, expected 1");
        assert!(!old.exists(), "an expired journal survived");
        assert!(new.exists(), "a fresh journal was destroyed");

        let _ = fs::remove_dir_all(&dir);
        unsafe { std::env::remove_var("ETUDE_STATE_DIR") };
    }

    fn set_mtime_secs_ago(p: &Path, secs: u64) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_secs() as i64;
        let t = now - secs as i64;
        let times = [TimeVal { sec: t, usec: 0 }, TimeVal { sec: t, usec: 0 }];
        let c = std::ffi::CString::new(p.to_string_lossy().as_bytes()).expect("path");
        // SAFETY: valid path pointer and a two-element timeval array, as utimes requires.
        unsafe { utimes(c.as_ptr(), times.as_ptr()) };
    }

    #[repr(C)]
    struct TimeVal {
        sec: i64,
        usec: i64,
    }
    unsafe extern "C" {
        fn utimes(path: *const std::ffi::c_char, times: *const TimeVal) -> i32;
    }

    /// Smoke test: proves write_bytes actually calls sync_dir after rename.
    /// Not a full crash-consistency proof.
    #[test]
    fn a_saved_journal_leaves_its_directory_fsyncable() {
        let _guard = STATE_DIR_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("sweep_dirsync_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("mkdir");
        unsafe { std::env::set_var("ETUDE_STATE_DIR", &dir) };

        struct Identity;
        impl Sealer for Identity {
            fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>, &'static str> {
                Ok(plaintext.to_vec())
            }
            fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, &'static str> {
                Ok(sealed.to_vec())
            }
        }

        let j = Journal {
            id: "dirsync".into(),
            tool: "test".into(),
            root: PathBuf::from("/tmp/root"),
            entries: vec![],
            created_directories: Vec::new(),
            validated_directories: Vec::new(),
            progress_tail_damaged: false,
        };
        #[cfg(unix)]
        let before = SYNC_DIR_CALLS.load(std::sync::atomic::Ordering::SeqCst);
        j.save_sealed(&Identity).expect("save_sealed");
        assert!(j.path().exists(), "journal file missing after save");
        #[cfg(unix)]
        {
            let after = SYNC_DIR_CALLS.load(std::sync::atomic::Ordering::SeqCst);
            assert_eq!(
                after - before,
                1,
                "write_bytes must call sync_dir exactly once"
            );
        }

        let _ = fs::remove_dir_all(&dir);
        unsafe { std::env::remove_var("ETUDE_STATE_DIR") };
    }

    #[test]
    fn round_trips_names_containing_tabs_and_newlines() {
        // The fixture tree contains a tab in a filename on purpose.
        let j = Journal {
            id: "t".into(),
            tool: "test".into(),
            root: PathBuf::from("/tmp/root"),
            entries: vec![Entry {
                from: PathBuf::from("/tmp/root/weird\tname.txt"),
                to: PathBuf::from("/tmp/root/g/weird\tname.txt"),
                method: Method::Rename,
                size: 12,
                mtime_secs: 99,
                inode: 7,
                edge_hash: 1234,
                state: EntryState::Moved,

                tag: None,
            }],
            created_directories: Vec::new(),
            validated_directories: Vec::new(),
            progress_tail_damaged: false,
        };
        let back = Journal::decode(&j.encode()).expect("decode");
        assert_eq!(back.entries[0].from, j.entries[0].from);
        assert_eq!(back.entries[0].to, j.entries[0].to);
        assert!(back.entries[0].is_moved());
    }

    /// The three tag outcomes are progress frames on top of the base journal,
    /// so a journal written before them loads unchanged and a stream that uses
    /// them replays to the right states. Out-of-order frames are refused like
    /// every other progress frame.
    #[test]
    fn held_returned_and_stranded_frames_replay_and_are_checked() {
        let _guard = STATE_DIR_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("sweep_tag_frames_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("mkdir");
        unsafe { std::env::set_var("ETUDE_STATE_DIR", &dir) };

        struct Identity;
        impl Sealer for Identity {
            fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>, &'static str> {
                Ok(plaintext.to_vec())
            }
            fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, &'static str> {
                Ok(sealed.to_vec())
            }
        }
        let entry = |name: &str| Entry {
            from: PathBuf::from(format!("/tmp/root/{name}")),
            to: PathBuf::from(format!("/tmp/root/G/{name}")),
            method: Method::Rename,
            size: 1,
            mtime_secs: 0,
            inode: 1,
            edge_hash: 0,
            state: EntryState::Planned,

            tag: None,
        };
        let journal = |id: &str| Journal {
            id: id.into(),
            tool: "test".into(),
            root: PathBuf::from("/tmp/root"),
            entries: vec![entry("a"), entry("b"), entry("c"), entry("d")],
            created_directories: Vec::new(),
            validated_directories: Vec::new(),
            progress_tail_damaged: false,
        };

        // a: held, b: moved then returned, c: moved then stranded, d: moved.
        let j = journal("frames");
        j.save_sealed(&Identity).unwrap();
        j.record_held(0, &Identity).unwrap();
        j.record_done(1, Method::Rename, &Identity).unwrap();
        j.record_returned(1, &Identity).unwrap();
        j.record_done(2, Method::Rename, &Identity).unwrap();
        j.record_stranded(2, &Identity).unwrap();
        j.record_done(3, Method::Rename, &Identity).unwrap();
        let back = Journal::load_sealed("test", "frames", &Identity).expect("reload");
        let states: Vec<_> = back.entries.iter().map(|e| e.state).collect();
        assert_eq!(
            states,
            [
                EntryState::Held,
                EntryState::Returned,
                EntryState::Stranded,
                EntryState::Moved
            ]
        );
        assert!(!back.entries[0].is_moved() && !back.entries[1].is_moved());
        assert!(back.entries[2].is_moved() && back.entries[3].is_moved());
        let summary = back.summary();
        assert_eq!(
            (summary.planned, summary.moved, summary.reversed),
            (0, 2, 0)
        );

        // Saving the whole journal keeps the states.
        back.save_sealed(&Identity).unwrap();
        let again = Journal::load_sealed("test", "frames", &Identity).expect("reload whole");
        let states: Vec<_> = again.entries.iter().map(|e| e.state).collect();
        assert_eq!(
            states,
            [
                EntryState::Held,
                EntryState::Returned,
                EntryState::Stranded,
                EntryState::Moved
            ]
        );

        // A held frame must take the place of the next done frame.
        let skipped = journal("held_out_of_order");
        skipped.save_sealed(&Identity).unwrap();
        skipped.record_held(2, &Identity).unwrap();
        assert!(Journal::load_sealed("test", "held_out_of_order", &Identity).is_err());

        // A return needs a move before it.
        let unmoved = journal("return_unmoved");
        unmoved.save_sealed(&Identity).unwrap();
        unmoved.record_returned(0, &Identity).unwrap();
        assert!(Journal::load_sealed("test", "return_unmoved", &Identity).is_err());

        // A stranded item is still reversible by undo's own frame.
        let undone = journal("stranded_then_undone");
        undone.save_sealed(&Identity).unwrap();
        undone.record_done(0, Method::Rename, &Identity).unwrap();
        undone.record_stranded(0, &Identity).unwrap();
        undone.record_undone(0, &Identity).unwrap();
        let back = Journal::load_sealed("test", "stranded_then_undone", &Identity).unwrap();
        assert_eq!(back.entries[0].state, EntryState::Reversed);

        let _ = fs::remove_dir_all(&dir);
    }

    /// Appended progress frames must carry the corrected method, not just the
    /// done bit. Otherwise undo's inode check picks the Rename path for a
    /// CopyUnlink entry after reload.
    #[test]
    fn record_done_preserves_copy_unlink_method_across_reload() {
        let _guard = STATE_DIR_LOCK.lock().unwrap();
        let dir =
            std::env::temp_dir().join(format!("sweep_progress_method_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("mkdir");
        unsafe { std::env::set_var("ETUDE_STATE_DIR", &dir) };

        struct Identity;
        impl Sealer for Identity {
            fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>, &'static str> {
                Ok(plaintext.to_vec())
            }
            fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, &'static str> {
                Ok(sealed.to_vec())
            }
        }

        let j = Journal {
            id: "method".into(),
            tool: "test".into(),
            root: PathBuf::from("/tmp/root"),
            entries: vec![Entry {
                from: PathBuf::from("/tmp/root/a"),
                to: PathBuf::from("/tmp/root/b"),
                method: Method::Rename, // as apply seeds it before move_one corrects
                size: 1,
                mtime_secs: 0,
                inode: 1,
                edge_hash: 0,
                state: EntryState::Planned,

                tag: None,
            }],
            created_directories: Vec::new(),
            validated_directories: Vec::new(),
            progress_tail_damaged: false,
        };
        j.save_sealed(&Identity).expect("base journal");
        j.record_done(0, Method::CopyUnlink, &Identity)
            .expect("record_done");

        let back = Journal::load_sealed("test", "method", &Identity).expect("reload");
        assert!(
            back.entries[0].is_moved(),
            "done bit lost on progress reload"
        );
        assert_eq!(
            back.entries[0].method,
            Method::CopyUnlink,
            "method correction lost on progress reload"
        );

        let _ = fs::remove_dir_all(&dir);
        unsafe { std::env::remove_var("ETUDE_STATE_DIR") };
    }

    /// A full `save_sealed` replaces the whole file (base + any appended
    /// progress). Stale progress must not resurrect a done bit the base
    /// explicitly cleared, e.g. after undo flips an entry back to not-done.
    #[test]
    fn save_sealed_discards_prior_progress_frames() {
        let _guard = STATE_DIR_LOCK.lock().unwrap();
        let dir =
            std::env::temp_dir().join(format!("sweep_progress_discard_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("mkdir");
        unsafe { std::env::set_var("ETUDE_STATE_DIR", &dir) };

        struct Identity;
        impl Sealer for Identity {
            fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>, &'static str> {
                Ok(plaintext.to_vec())
            }
            fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, &'static str> {
                Ok(sealed.to_vec())
            }
        }

        let mut j = Journal {
            id: "discard".into(),
            tool: "test".into(),
            root: PathBuf::from("/tmp/root"),
            entries: vec![Entry {
                from: PathBuf::from("/tmp/root/a"),
                to: PathBuf::from("/tmp/root/b"),
                method: Method::Rename,
                size: 1,
                mtime_secs: 0,
                inode: 1,
                edge_hash: 0,
                state: EntryState::Planned,

                tag: None,
            }],
            created_directories: Vec::new(),
            validated_directories: Vec::new(),
            progress_tail_damaged: false,
        };
        j.save_sealed(&Identity).expect("base");
        j.record_done(0, Method::Rename, &Identity)
            .expect("progress");

        // Simulate undo: entry is no longer done; caller persists via save_sealed.
        // Undo reverses, which is Reversed — not Planned. Planned would say
        // apply never moved it, which is a different situation entirely.
        j.entries[0].state = EntryState::Reversed;
        j.save_sealed(&Identity).expect("re-save after undo");

        let back = Journal::load_sealed("test", "discard", &Identity).expect("reload");
        assert!(
            !back.entries[0].is_moved(),
            "stale progress resurrected a cleared done bit"
        );

        let _ = fs::remove_dir_all(&dir);
        unsafe { std::env::remove_var("ETUDE_STATE_DIR") };
    }

    /// The defect in issue #3: a journal cut off mid-way through its last
    /// progress frame (exactly what a crash leaves behind, since the move
    /// for that entry already succeeded before the frame write started)
    /// must be refused outright, not half-loaded with the interrupted
    /// entry silently downgraded to "not done". A silent downgrade is worse
    /// than a refusal: `undo` would see `done: false`, skip the file, and
    /// leave it stranded at its destination without a word.
    #[test]
    fn a_torn_tail_keeps_the_complete_frames_and_says_it_was_torn() {
        let _guard = STATE_DIR_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("sweep_progress_trunc_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("mkdir");
        unsafe { std::env::set_var("ETUDE_STATE_DIR", &dir) };

        struct Identity;
        impl Sealer for Identity {
            fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>, &'static str> {
                Ok(plaintext.to_vec())
            }
            fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, &'static str> {
                Ok(sealed.to_vec())
            }
        }

        let j = Journal {
            id: "trunc".into(),
            tool: "test".into(),
            root: PathBuf::from("/tmp/root"),
            entries: vec![
                Entry {
                    from: PathBuf::from("/tmp/root/a"),
                    to: PathBuf::from("/tmp/root/g/a"),
                    method: Method::Rename,
                    size: 1,
                    mtime_secs: 0,
                    inode: 1,
                    edge_hash: 0,
                    state: EntryState::Planned,

                    tag: None,
                },
                Entry {
                    from: PathBuf::from("/tmp/root/b"),
                    to: PathBuf::from("/tmp/root/g/b"),
                    method: Method::Rename,
                    size: 1,
                    mtime_secs: 0,
                    inode: 2,
                    edge_hash: 0,
                    state: EntryState::Planned,

                    tag: None,
                },
            ],
            created_directories: Vec::new(),
            validated_directories: Vec::new(),
            progress_tail_damaged: false,
        };
        j.save_sealed(&Identity).expect("base journal");
        // Both moves genuinely completed on disk. record_done is only ever
        // called after move_one returns Ok (see apply's ordering rule).
        // Both progress frames were fully appended and fsynced here.
        j.record_done(0, Method::Rename, &Identity)
            .expect("record entry 0 done");
        j.record_done(1, Method::Rename, &Identity)
            .expect("record entry 1 done");

        // Simulate a crash that cut the file mid-write of the *last* frame:
        // chop a few trailing bytes off, landing inside entry 1's frame
        // rather than on a frame boundary. Entry 0's frame is untouched and
        // fully intact.
        let full = fs::read(j.path()).expect("read journal");
        assert!(
            full.len() > 4,
            "test setup: journal too short to truncate meaningfully"
        );
        let truncated = &full[..full.len() - 3];
        fs::write(j.path(), truncated).expect("write truncated journal");

        // The property: no move that completed on disk may end up invisible
        // to undo. There are two ways to honour it, and the code changed
        // from the first to the second.
        //
        // Refusing the whole journal honours it loudly and expensively: no
        // entry is downgraded, but nothing is recovered either, and every
        // file the complete frames describe stays at its destination. A CI
        // artifact showed 143 complete frames voided by a 4-byte tail.
        //
        // Replaying the complete frames and stopping at the tear honours it
        // too, and this is why: apply moves a file and only then appends its
        // frame, so a partial frame means that entry's move happened and was
        // not recorded. That entry is left Planned -- and it is necessarily
        // the FIRST Planned entry, since every earlier one has a complete
        // frame. undo's successor-entry recovery covers exactly the first
        // Planned entry. So the file is not skipped; it is recovered by the
        // path built for it. `apply_undo.rs` proves that end to end against
        // real files, because this test has no filesystem to check.
        //
        // What must never happen is a silent downgrade, so the loss of the
        // tail has to be visible on the journal itself.
        let loaded = Journal::load_sealed("test", "trunc", &Identity)
            .expect("a torn tail must not void the whole journal");

        assert!(
            loaded.entries[0].is_moved(),
            "entry 0's frame was complete and sealed. Dropping it strands a file \
             that undo could have restored, which is what voiding the journal did"
        );
        assert_eq!(
            loaded.entries[1].state,
            EntryState::Planned,
            "entry 1's frame was cut mid-write, so nothing durable says it moved"
        );
        assert!(
            loaded.progress_tail_damaged,
            "a journal that lost its tail must say so. Loading it as though it \
             were whole is the silent downgrade this test exists to prevent"
        );

        let _ = fs::remove_dir_all(&dir);
        unsafe { std::env::remove_var("ETUDE_STATE_DIR") };
    }

    /// A clean end of file (the file ends exactly on a frame boundary, as it
    /// does after a normal save with no progress yet, or after a crash that
    /// landed cleanly on a frame boundary before a later move ever started)
    /// is not damage and must still load normally. Only a torn frame is
    /// refused.
    #[test]
    fn clean_end_of_progress_frames_is_not_treated_as_damage() {
        let _guard = STATE_DIR_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("sweep_progress_clean_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("mkdir");
        unsafe { std::env::set_var("ETUDE_STATE_DIR", &dir) };

        struct Identity;
        impl Sealer for Identity {
            fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>, &'static str> {
                Ok(plaintext.to_vec())
            }
            fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, &'static str> {
                Ok(sealed.to_vec())
            }
        }

        let j = Journal {
            id: "clean".into(),
            tool: "test".into(),
            root: PathBuf::from("/tmp/root"),
            entries: vec![
                Entry {
                    from: PathBuf::from("/tmp/root/a"),
                    to: PathBuf::from("/tmp/root/g/a"),
                    method: Method::Rename,
                    size: 1,
                    mtime_secs: 0,
                    inode: 1,
                    edge_hash: 0,
                    state: EntryState::Planned,

                    tag: None,
                },
                Entry {
                    from: PathBuf::from("/tmp/root/b"),
                    to: PathBuf::from("/tmp/root/g/b"),
                    method: Method::Rename,
                    size: 1,
                    mtime_secs: 0,
                    inode: 2,
                    edge_hash: 0,
                    state: EntryState::Planned,

                    tag: None,
                },
            ],
            created_directories: Vec::new(),
            validated_directories: Vec::new(),
            progress_tail_damaged: false,
        };
        j.save_sealed(&Identity).expect("base journal");
        j.record_done(0, Method::Rename, &Identity)
            .expect("record entry 0 done");
        // Entry 1 was never even attempted. A crash landed cleanly between
        // the two moves. No bytes for it exist at all, so this is not a
        // torn frame.

        let back = Journal::load_sealed("test", "clean", &Identity).expect("clean load");
        assert!(
            back.entries[0].is_moved(),
            "entry 0's completed move was lost"
        );
        assert!(
            !back.entries[1].is_moved(),
            "entry 1 was reported done despite never being recorded"
        );

        let _ = fs::remove_dir_all(&dir);
        unsafe { std::env::remove_var("ETUDE_STATE_DIR") };
    }

    /// Journals written before length-framing must still load after upgrade.
    /// Otherwise in-flight undo breaks until TTL prunes them.
    #[test]
    fn load_sealed_reads_legacy_unframed_journals() {
        let _guard = STATE_DIR_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("sweep_legacy_journal_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("mkdir");
        unsafe { std::env::set_var("ETUDE_STATE_DIR", &dir) };

        struct Identity;
        impl Sealer for Identity {
            fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>, &'static str> {
                Ok(plaintext.to_vec())
            }
            fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, &'static str> {
                Ok(sealed.to_vec())
            }
        }

        let j = Journal {
            id: "legacy".into(),
            tool: "test".into(),
            root: PathBuf::from("/tmp/root"),
            entries: vec![
                Entry {
                    from: PathBuf::from("/tmp/root/a"),
                    to: PathBuf::from("/tmp/root/b"),
                    method: Method::Rename,
                    size: 12,
                    mtime_secs: 99,
                    inode: 7,
                    edge_hash: 1234,
                    state: EntryState::Moved,

                    tag: None,
                },
                Entry {
                    from: PathBuf::from("/tmp/root/c"),
                    to: PathBuf::from("/tmp/root/d"),
                    method: Method::CopyUnlink,
                    size: 3,
                    mtime_secs: 1,
                    inode: 2,
                    edge_hash: 5,
                    state: EntryState::Planned,

                    tag: None,
                },
            ],
            created_directories: Vec::new(),
            validated_directories: Vec::new(),
            progress_tail_damaged: false,
        };
        // Pre-framing write_bytes: sealed blob only, no length prefix.
        let sealed = Identity.seal(j.encode().as_bytes()).expect("seal legacy");
        fs::write(j.path(), sealed).expect("write legacy journal");

        let back = Journal::load_sealed("test", "legacy", &Identity).expect("load legacy");
        assert_eq!(back.entries.len(), 2);
        assert_eq!(back.entries[0].from, j.entries[0].from);
        assert_eq!(back.entries[0].to, j.entries[0].to);
        assert!(back.entries[0].is_moved(), "done entry lost on legacy load");
        assert_eq!(back.entries[1].method, Method::CopyUnlink);
        assert!(
            !back.entries[1].is_moved(),
            "not-done entry flipped on legacy load"
        );

        let _ = fs::remove_dir_all(&dir);
        unsafe { std::env::remove_var("ETUDE_STATE_DIR") };
    }

    /// Issue #23's contract: the un-overridden default is the macOS
    /// convention, not the old XDG path. Testing HOME directly rather than
    /// spawning the real binary -- an earlier attempt at this hung a live
    /// shell on a real keychain prompt, because overriding HOME while a real
    /// binary still calls /usr/bin/security is a system-level operation, not
    /// a contained one. state_dir() only reads an env var; nothing here
    /// touches the keychain.
    ///
    /// Restores whatever HOME was before it, rather than blindly removing
    /// it. A review pointed out that env vars are process-global and cargo
    /// runs tests on multiple threads in one process; STATE_DIR_LOCK
    /// serializes these tests against each other, but not against some
    /// unrelated test elsewhere in the crate reading HOME at the exact
    /// moment one of these had removed it. Restoring what was actually
    /// there closes that gap instead of assuming HOME is unset to begin
    /// with, which on a real machine it never is.
    struct RestoreEnvVar {
        name: &'static str,
        prior: Option<std::ffi::OsString>,
    }
    impl RestoreEnvVar {
        fn set(name: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let prior = std::env::var_os(name);
            unsafe { std::env::set_var(name, value) };
            Self { name, prior }
        }
    }
    impl Drop for RestoreEnvVar {
        fn drop(&mut self) {
            match self.prior.take() {
                Some(v) => unsafe { std::env::set_var(self.name, v) },
                None => unsafe { std::env::remove_var(self.name) },
            }
        }
    }

    #[test]
    fn the_default_state_dir_is_application_support_not_xdg() {
        let _guard = STATE_DIR_LOCK.lock().unwrap();
        unsafe { std::env::remove_var("ETUDE_STATE_DIR") };
        let fake_home = std::env::temp_dir().join(format!("etudes_home_{}", std::process::id()));
        let _home = RestoreEnvVar::set("HOME", &fake_home);

        let got = state_dir();

        assert_eq!(
            got,
            fake_home.join("Library/Application Support/etudes"),
            "the un-overridden default must be the macOS convention, got {got:?}"
        );
    }

    /// XDG_STATE_HOME is gone as a fallback, not merely deprioritised. If a
    /// test or a user's shell profile still sets it out of habit, it must be
    /// silently ignored rather than silently honoured.
    #[test]
    fn xdg_state_home_is_no_longer_read() {
        let _guard = STATE_DIR_LOCK.lock().unwrap();
        unsafe { std::env::remove_var("ETUDE_STATE_DIR") };
        let fake_home =
            std::env::temp_dir().join(format!("etudes_home_xdg_{}", std::process::id()));
        let _home = RestoreEnvVar::set("HOME", &fake_home);
        let _xdg = RestoreEnvVar::set("XDG_STATE_HOME", "/somewhere/that/must/be/ignored");

        let got = state_dir();

        assert_eq!(
            got,
            fake_home.join("Library/Application Support/etudes"),
            "XDG_STATE_HOME must not affect the default any more, got {got:?}"
        );
    }

    /// ETUDE_STATE_DIR still wins over everything -- the override this test
    /// suite itself depends on throughout, unrelated to which OS convention
    /// the un-overridden default follows.
    #[test]
    fn etude_state_dir_still_overrides_the_default() {
        let _guard = STATE_DIR_LOCK.lock().unwrap();
        let explicit = std::env::temp_dir().join(format!("etudes_explicit_{}", std::process::id()));
        let _override = RestoreEnvVar::set("ETUDE_STATE_DIR", &explicit);

        let got = state_dir();

        assert_eq!(got, explicit, "an explicit override must still win");
    }

    /// The migration itself: an old journal at the legacy path must still be
    /// reachable after the default moves, not silently orphaned. One
    /// fs::rename of the whole directory, checked by reading the journal
    /// back from its NEW location afterward -- not just checking that a
    /// path no longer exists, which would pass even if the migration lost
    /// the file rather than moving it.
    #[test]
    fn a_legacy_journal_is_migrated_forward_and_still_readable() {
        let _guard = STATE_DIR_LOCK.lock().unwrap();
        unsafe { std::env::remove_var("ETUDE_STATE_DIR") };
        let fake_home = std::env::temp_dir().join(format!("etudes_migrate_{}", std::process::id()));
        let _ = fs::remove_dir_all(&fake_home);
        let legacy = fake_home.join(".local/state/etudes");
        fs::create_dir_all(&legacy).expect("mkdir legacy");
        fs::write(legacy.join("sweep-old-id.journal"), b"legacy content").expect("write");
        let _home = RestoreEnvVar::set("HOME", &fake_home);

        migrate_legacy_state_dir();

        let new_path = fake_home
            .join("Library/Application Support/etudes")
            .join("sweep-old-id.journal");
        let content = fs::read(&new_path);
        let _ = fs::remove_dir_all(&fake_home);

        assert_eq!(
            content.expect("migrated journal must be readable at the new path"),
            b"legacy content",
            "the migrated file's content must be intact, not just present"
        );
    }

    /// A caller with an explicit ETUDE_STATE_DIR is not migrated. They have
    /// their own idea of where state lives; moving a directory they never
    /// asked about would be a surprise in the other direction.
    #[test]
    fn migration_does_not_run_when_etude_state_dir_is_set() {
        let _guard = STATE_DIR_LOCK.lock().unwrap();
        let fake_home =
            std::env::temp_dir().join(format!("etudes_nomigrate_{}", std::process::id()));
        let _ = fs::remove_dir_all(&fake_home);
        let legacy = fake_home.join(".local/state/etudes");
        fs::create_dir_all(&legacy).expect("mkdir legacy");
        fs::write(legacy.join("sweep-old-id.journal"), b"legacy content").expect("write");
        let _home = RestoreEnvVar::set("HOME", &fake_home);
        let explicit = std::env::temp_dir().join(format!("etudes_override_{}", std::process::id()));
        let _override = RestoreEnvVar::set("ETUDE_STATE_DIR", &explicit);

        migrate_legacy_state_dir();

        let legacy_still_there = legacy.join("sweep-old-id.journal").exists();
        let _ = fs::remove_dir_all(&fake_home);

        assert!(
            legacy_still_there,
            "an explicit ETUDE_STATE_DIR must leave the legacy directory untouched"
        );
    }
}
