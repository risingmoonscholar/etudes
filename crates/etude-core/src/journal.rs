//! The undo journal.
//!
//! # Encryption
//!
//! The journal is sealed by a [`Sealer`] supplied by the caller. `etude-core`
//! deliberately does not know how — keeping the engine dependency-free is what
//! makes the no-network claim cheap to check, so the cipher lives in
//! `etude-keep` and is injected here.
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
    /// link → unlink within one device. Reversed the same way.
    Rename,
    /// copy → verify → unlink across devices. Reversed the same way.
    CopyUnlink,
}

impl Method {
    fn tag(self) -> &'static str {
        match self {
            Method::Rename => "rename",
            Method::CopyUnlink => "copy",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        match s {
            "rename" => Some(Method::Rename),
            "copy" => Some(Method::CopyUnlink),
            _ => None,
        }
    }
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
    /// integrity — it defeats accident, not an adversary.
    pub edge_hash: u64,
    /// False until the move has actually succeeded on disk.
    pub done: bool,
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
            JournalError::Io(e) => write!(f, "journal io: {}", e.kind()),
            JournalError::Malformed(w) => write!(f, "journal malformed: {w}"),
            JournalError::NotFound => write!(f, "no journal found"),
            JournalError::Seal(m) => write!(f, "journal seal: {m}"),
        }
    }
}

/// Where journals live. Chosen to sit outside the default sync roots of iCloud
/// Drive, Dropbox and OneDrive.
pub fn state_dir() -> PathBuf {
    if let Ok(x) = std::env::var("ETUDE_STATE_DIR") {
        return PathBuf::from(x);
    }
    if let Ok(x) = std::env::var("XDG_STATE_HOME") {
        return PathBuf::from(x).join("etudes");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".local/state/etudes")
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

impl Journal {
    pub fn path(&self) -> PathBuf {
        state_dir().join(format!("{}-{}.journal", self.tool, self.id))
    }

    /// Serialise. Line-oriented so a truncated write loses at most one entry.
    pub fn encode(&self) -> String {
        let mut s = String::new();
        s.push_str("sweep-journal 1\n");
        s.push_str(&format!("root\t{}\n", esc(&self.root)));
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
                if e.done { 1 } else { 0 }
            ));
        }
        s
    }

    pub fn decode(text: &str) -> Result<Journal, JournalError> {
        let mut j = Journal::default();
        for line in text.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            match f.first().copied() {
                Some("root") => {
                    j.root = PathBuf::from(unesc(f.get(1).copied().unwrap_or_default()))
                }
                Some("mv") => {
                    if f.len() < 9 {
                        return Err(JournalError::Malformed("short mv record"));
                    }
                    j.entries.push(Entry {
                        from: PathBuf::from(unesc(f[1])),
                        to: PathBuf::from(unesc(f[2])),
                        method: Method::parse(f[3])
                            .ok_or(JournalError::Malformed("unknown method"))?,
                        size: f[4].parse().map_err(|_| JournalError::Malformed("size"))?,
                        mtime_secs: f[5].parse().map_err(|_| JournalError::Malformed("mtime"))?,
                        inode: f[6].parse().map_err(|_| JournalError::Malformed("inode"))?,
                        edge_hash: f[7].parse().map_err(|_| JournalError::Malformed("hash"))?,
                        done: f[8] == "1",
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
        // on the file is enough — that's why this is O(1).
        f.sync_all().map_err(JournalError::Io)?;
        Ok(())
    }

    /// Load and unseal. `id` is the bare id; `tool` selects the namespace.
    ///
    /// On-disk layout is length-framed: a required base frame (sealed encode
    /// of all entries), then zero or more sealed progress frames appended by
    /// [`Self::record_done`]. Trailing progress is best-effort.
    ///
    /// Journals written before length-framing — a single unframed sealed blob —
    /// are still accepted. Framed parse is tried first; any failure falls back
    /// to opening the whole file as one sealed blob (the pre-framing format).
    pub fn load_sealed(tool: &str, id: &str, sealer: &dyn Sealer) -> Result<Journal, JournalError> {
        let p = state_dir().join(format!("{tool}-{id}.journal"));
        let raw = fs::read(&p).map_err(|_| JournalError::NotFound)?;

        // Current format: 4-byte LE length + sealed base (+ optional progress).
        if raw.len() >= 4 {
            let base_len = u32::from_le_bytes(raw[0..4].try_into().unwrap()) as usize;
            if 4 + base_len <= raw.len()
                && let Ok(plain) = sealer.open(&raw[4..4 + base_len])
                && let Ok(text) = String::from_utf8(plain)
                && let Ok(mut j) = Journal::decode(&text)
            {
                j.id = id.to_string();
                j.tool = tool.to_string();
                j.apply_progress(&raw[4 + base_len..], sealer);
                return Ok(j);
            }
        }

        // Legacy: whole file is one unframed sealed blob (no progress frames).
        let plain = sealer.open(&raw).map_err(JournalError::Seal)?;
        let text = String::from_utf8(plain).map_err(|_| JournalError::Malformed("not utf-8"))?;
        let mut j = Journal::decode(&text)?;
        j.id = id.to_string();
        j.tool = tool.to_string();
        Ok(j)
    }

    /// Replay length-framed progress records that follow the base frame.
    /// Truncated, unsealable, or out-of-order frames stop replay rather than
    /// failing the load — we keep only the longest verified in-order
    /// done-prefix.
    fn apply_progress(&mut self, raw: &[u8], sealer: &dyn Sealer) {
        let mut offset = 0usize;
        let mut expected = 0usize;
        while offset + 4 <= raw.len() {
            let len = u32::from_le_bytes(raw[offset..offset + 4].try_into().unwrap()) as usize;
            offset += 4;
            if offset + len > raw.len() {
                break; // truncated trailing record — don't trust it
            }
            let sealed = &raw[offset..offset + len];
            offset += len;
            let Ok(plain) = sealer.open(sealed) else {
                break;
            };
            let Ok(text) = std::str::from_utf8(&plain) else {
                break;
            };
            let text = text.trim_end_matches('\n');
            let f: Vec<&str> = text.split('\t').collect();
            if f.len() != 3 || f[0] != "done" {
                break;
            }
            let Ok(index) = f[1].parse::<usize>() else {
                break;
            };
            let Some(method) = Method::parse(f[2]) else {
                break;
            };
            if index != expected || index >= self.entries.len() {
                break;
            }
            self.entries[index].done = true;
            self.entries[index].method = method;
            expected += 1;
        }
    }

    /// Most recent sealed journal **written by `tool`**, by modification time.
    pub fn latest_sealed(tool: &str, sealer: &dyn Sealer) -> Result<Journal, JournalError> {
        let id = latest_id(tool)?;
        Journal::load_sealed(tool, &id, sealer)
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
        // atomically discards any previously-appended progress frames — there
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
    let Ok(rd) = fs::read_dir(state_dir()) else {
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
    let prefix = format!("{tool}-");
    let dir = state_dir();
    let mut best: Option<(SystemTime, String)> = None;
    for e in fs::read_dir(&dir).map_err(|_| JournalError::NotFound)? {
        let Ok(e) = e else { continue };
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(stem) = name.strip_suffix(".journal") else {
            continue;
        };
        let Some(id) = stem.strip_prefix(&prefix) else {
            continue;
        };
        let Ok(md) = e.metadata() else { continue };
        let Ok(t) = md.modified() else { continue };
        if best.as_ref().is_none_or(|(bt, _)| t > *bt) {
            best = Some((t, id.to_string()));
        }
    }
    best.map(|(_, id)| id).ok_or(JournalError::NotFound)
}

/// Every id for journals written by `tool`, newest-first by modification time.
/// `latest_id` only ever returns the single most recent one; pop needs to search
/// across all of them to find the journal for a specific folder, not just the
/// newest.
pub fn ids_by_recency(tool: &str) -> Result<Vec<String>, JournalError> {
    let prefix = format!("{tool}-");
    let dir = state_dir();
    let mut journals = Vec::new();
    for e in fs::read_dir(&dir).map_err(|_| JournalError::NotFound)? {
        let Ok(e) = e else { continue };
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(stem) = name.strip_suffix(".journal") else {
            continue;
        };
        let Some(id) = stem.strip_prefix(&prefix) else {
            continue;
        };
        let Ok(md) = e.metadata() else { continue };
        let Ok(t) = md.modified() else { continue };
        journals.push((t, id.to_string()));
    }
    if journals.is_empty() {
        return Err(JournalError::NotFound);
    }
    journals.sort_by(|(a, _), (b, _)| b.cmp(a));
    Ok(journals.into_iter().map(|(_, id)| id).collect())
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
    let d = fs::File::open(dir).map_err(JournalError::Io)?;
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
/// this must accept a directory. There is nothing to hash — opening a directory
/// fails — so the edge hash is 0 and identity rests on inode and mtime. Found by
/// stash, which moves everything including packages; sweep would have hit the
/// same panic the first time a `.app` landed in a group.
pub fn fingerprint(p: &Path) -> io::Result<(u64, i64, u64, u64)> {
    // symlink_metadata, not metadata: a symlink must be identified by the LINK,
    // never by its target. Following it makes the link's fingerprint change
    // whenever the target does — stash found this with a link pointing at the
    // folder being emptied, which reported the link as modified and refused to
    // restore it.
    let md = fs::symlink_metadata(p)?;
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
/// This detects accidental change — an edit, a replacement, a different file at
/// the same path. It is **not** an integrity check and must never be described
/// as one: an adversary can trivially preserve it.
pub fn edge_hash(p: &Path, size: u64) -> io::Result<u64> {
    const EDGE: u64 = 4096;
    let mut f = fs::File::open(p)?;
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let feed = |buf: &[u8], h: &mut u64| {
        for b in buf {
            *h ^= *b as u64;
            *h = h.wrapping_mul(0x100_0000_01b3);
        }
    };

    let head_len = size.min(EDGE) as usize;
    let mut head = vec![0u8; head_len];
    f.read_exact(&mut head)?;
    feed(&head, &mut h);

    if size > EDGE {
        let tail_len = EDGE.min(size - EDGE) as usize;
        f.seek(SeekFrom::End(-(tail_len as i64)))?;
        let mut tail = vec![0u8; tail_len];
        f.read_exact(&mut tail)?;
        feed(&tail, &mut h);
    }
    // Length is part of the identity, so truncation alone changes the hash.
    feed(&size.to_le_bytes(), &mut h);
    Ok(h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // ETUDE_STATE_DIR is process-global; serialize tests that touch it.
    static STATE_DIR_LOCK: Mutex<()> = Mutex::new(());

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

    /// Smoke test: proves write_bytes actually calls sync_dir after rename —
    /// not a full crash-consistency proof.
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
                done: true,
            }],
        };
        let back = Journal::decode(&j.encode()).expect("decode");
        assert_eq!(back.entries[0].from, j.entries[0].from);
        assert_eq!(back.entries[0].to, j.entries[0].to);
        assert!(back.entries[0].done);
    }

    /// Appended progress frames must carry the corrected method, not just the
    /// done bit — otherwise undo's inode check picks the Rename path for a
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
                done: false,
            }],
        };
        j.save_sealed(&Identity).expect("base journal");
        j.record_done(0, Method::CopyUnlink, &Identity)
            .expect("record_done");

        let back = Journal::load_sealed("test", "method", &Identity).expect("reload");
        assert!(back.entries[0].done, "done bit lost on progress reload");
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
    /// explicitly cleared — e.g. after undo flips an entry back to not-done.
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
                done: false,
            }],
        };
        j.save_sealed(&Identity).expect("base");
        j.record_done(0, Method::Rename, &Identity)
            .expect("progress");

        // Simulate undo: entry is no longer done; caller persists via save_sealed.
        j.entries[0].done = false;
        j.save_sealed(&Identity).expect("re-save after undo");

        let back = Journal::load_sealed("test", "discard", &Identity).expect("reload");
        assert!(
            !back.entries[0].done,
            "stale progress resurrected a cleared done bit"
        );

        let _ = fs::remove_dir_all(&dir);
        unsafe { std::env::remove_var("ETUDE_STATE_DIR") };
    }

    /// Journals written before length-framing must still load after upgrade —
    /// otherwise in-flight undo breaks until TTL prunes them.
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
                    done: true,
                },
                Entry {
                    from: PathBuf::from("/tmp/root/c"),
                    to: PathBuf::from("/tmp/root/d"),
                    method: Method::CopyUnlink,
                    size: 3,
                    mtime_secs: 1,
                    inode: 2,
                    edge_hash: 5,
                    done: false,
                },
            ],
        };
        // Pre-framing write_bytes: sealed blob only, no length prefix.
        let sealed = Identity.seal(j.encode().as_bytes()).expect("seal legacy");
        fs::write(j.path(), sealed).expect("write legacy journal");

        let back = Journal::load_sealed("test", "legacy", &Identity).expect("load legacy");
        assert_eq!(back.entries.len(), 2);
        assert_eq!(back.entries[0].from, j.entries[0].from);
        assert_eq!(back.entries[0].to, j.entries[0].to);
        assert!(back.entries[0].done, "done entry lost on legacy load");
        assert_eq!(back.entries[1].method, Method::CopyUnlink);
        assert!(
            !back.entries[1].done,
            "not-done entry flipped on legacy load"
        );

        let _ = fs::remove_dir_all(&dir);
        unsafe { std::env::remove_var("ETUDE_STATE_DIR") };
    }
}
