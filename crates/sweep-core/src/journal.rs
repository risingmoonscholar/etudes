//! The undo journal.
//!
//! # Encryption
//!
//! The journal is sealed by a [`Sealer`] supplied by the caller. `sweep-core`
//! deliberately does not know how — keeping the engine dependency-free is what
//! makes the no-network claim cheap to check, so the cipher lives in
//! `sweep-keep` and is injected here.
//!
//! There is no plaintext fallback. If sealing is unavailable the journal is not
//! written and the caller is told, because silently degrading to plaintext is
//! exactly the failure a privacy tool must not have.
//!
//! # Ordering
//!
//! The journal is written *before* the first move and each entry is marked
//! complete only after its move succeeds. A crash therefore leaves a journal
//! describing exactly what happened, never more (docs/CRITIQUE.md § 9).

use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// How a file got to its destination. Undo reverses each differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// `rename(2)` within one device. Reversed by renaming back.
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
    pub root: PathBuf,
    pub entries: Vec<Entry>,
}

/// Supplied by the caller so `sweep-core` need not depend on a cipher.
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
/// Drive, Dropbox and OneDrive (docs/THREAT-MODEL.md § T4).
pub fn state_dir() -> PathBuf {
    if let Ok(x) = std::env::var("SWEEP_STATE_DIR") {
        return PathBuf::from(x);
    }
    if let Ok(x) = std::env::var("XDG_STATE_HOME") {
        return PathBuf::from(x).join("sweep");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".local/state/sweep")
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
        state_dir().join(format!("{}.journal", self.id))
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

    /// Persist, sealed. Written before the first move and rewritten after each
    /// one, so the on-disk state never claims more than actually happened.
    pub fn save_sealed(&self, sealer: &dyn Sealer) -> Result<(), JournalError> {
        let bytes = sealer
            .seal(self.encode().as_bytes())
            .map_err(|m| JournalError::Seal(m))?;
        self.write_bytes(&bytes)
    }

    /// Load and unseal.
    pub fn load_sealed(id: &str, sealer: &dyn Sealer) -> Result<Journal, JournalError> {
        let p = state_dir().join(format!("{id}.journal"));
        let raw = fs::read(&p).map_err(|_| JournalError::NotFound)?;
        let plain = sealer.open(&raw).map_err(|m| JournalError::Seal(m))?;
        let text = String::from_utf8(plain).map_err(|_| JournalError::Malformed("not utf-8"))?;
        let mut j = Journal::decode(&text)?;
        j.id = id.to_string();
        Ok(j)
    }

    /// Most recent sealed journal by modification time.
    pub fn latest_sealed(sealer: &dyn Sealer) -> Result<Journal, JournalError> {
        let id = latest_id()?;
        Journal::load_sealed(&id, sealer)
    }

    fn write_bytes(&self, bytes: &[u8]) -> Result<(), JournalError> {
        let dir = state_dir();
        fs::create_dir_all(&dir).map_err(JournalError::Io)?;
        // 0700, not 0600: a directory needs the execute bit to be entered, so
        // 0600 makes it impossible to create the journal inside it.
        restrict_dir(&dir);
        let p = self.path();
        // Write to a temp file in the same directory, then rename: a crash
        // mid-write cannot leave a half-parsed journal.
        let tmp = p.with_extension("journal.tmp");
        let mut f = fs::File::create(&tmp).map_err(JournalError::Io)?;
        restrict(&tmp);
        f.write_all(bytes).map_err(JournalError::Io)?;
        f.sync_all().map_err(JournalError::Io)?;
        drop(f);
        fs::rename(&tmp, &p).map_err(JournalError::Io)?;
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
/// long as the user remembers what they ran (docs/THREAT-MODEL.md § A4).
pub const TTL_DAYS: u64 = 30;

/// Delete journals older than [`TTL_DAYS`]. Returns how many were removed.
///
/// Called at the start of every run that touches the state directory. Failure
/// is not fatal: a journal that cannot be removed is a housekeeping problem,
/// not a reason to refuse the user's actual request.
pub fn prune_expired() -> usize {
    let cutoff = match SystemTime::now().checked_sub(std::time::Duration::from_secs(TTL_DAYS * 86_400)) {
        Some(c) => c,
        None => return 0,
    };
    let Ok(rd) = fs::read_dir(state_dir()) else { return 0 };
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

/// Newest journal id in the state directory, by modification time.
pub fn latest_id() -> Result<String, JournalError> {
    let dir = state_dir();
    let mut best: Option<(SystemTime, String)> = None;
    for e in fs::read_dir(&dir).map_err(|_| JournalError::NotFound)? {
        let Ok(e) = e else { continue };
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(id) = name.strip_suffix(".journal") else { continue };
        let Ok(md) = e.metadata() else { continue };
        let Ok(t) = md.modified() else { continue };
        if best.as_ref().is_none_or(|(bt, _)| t > *bt) {
            best = Some((t, id.to_string()));
        }
    }
    best.map(|(_, id)| id).ok_or(JournalError::NotFound)
}

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
#[cfg(not(unix))]
fn restrict(_p: &Path) {}
#[cfg(not(unix))]
fn restrict_dir(_p: &Path) {}

/// Facts about a file, captured at apply time and re-checked at undo time.
pub fn fingerprint(p: &Path) -> io::Result<(u64, i64, u64, u64)> {
    let md = fs::metadata(p)?;
    let size = md.len();
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
    Ok((size, mtime, inode, edge_hash(p, size)?))
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

    #[test]
    fn expired_journals_are_pruned_and_fresh_ones_are_kept() {
        let dir = std::env::temp_dir().join(format!("sweep_ttl_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("mkdir");
        unsafe { std::env::set_var("SWEEP_STATE_DIR", &dir) };

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
        unsafe { std::env::remove_var("SWEEP_STATE_DIR") };
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

    #[test]
    fn round_trips_names_containing_tabs_and_newlines() {
        // The fixture tree contains a tab in a filename on purpose.
        let j = Journal {
            id: "t".into(),
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
        assert_eq!(back.entries[0].done, true);
    }

}
