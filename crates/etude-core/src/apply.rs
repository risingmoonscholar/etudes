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
    /// The OS could not tell us a filename's normalized form, so whether two
    /// destinations collide is unknown. Refusing is the only honest answer:
    /// guessing "they don't" is the guess that moves files.
    CannotCompareNames(PathBuf),
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
            ApplyError::CannotCompareNames(p) => write!(
                f,
                "refused: cannot compare {} against the other destinations — the system \
                 would not normalise the name, so a collision cannot be ruled out",
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
            if !planned_destinations.insert(dedupe_key(&dst)?) {
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

/// Case- and composition-insensitive dedupe key for a planned destination
/// path. Issue #9.
///
/// `to_lowercase` folds ASCII case correctly (`Report.pdf` == `report.PDF`),
/// but it does not touch Unicode *normalization*. "café" typed as one
/// precomposed code point (NFC, U+00E9) and as `e` followed by a combining
/// acute accent (NFD, U+0065 U+0301) are different byte strings in Rust, but
/// APFS treats them as the exact same directory entry — the guard that used
/// plain `to_lowercase` saw two distinct destinations, let the plan through,
/// moved the first file for real, then hit EEXIST on the second mid-run.
///
/// `etude-core` carries zero dependencies (see `Cargo.toml`), so this does
/// not pull in a Unicode-normalization crate. Instead, on macOS, it asks the
/// OS itself: `CFStringNormalize` from CoreFoundation, linked directly (the
/// same raw-FFI pattern `scan.rs` already uses for `getuid` and
/// `journal.rs` for `utimes` — no build.rs, no crates.io dependency, just a
/// framework the OS already ships). This uses the actual Unicode tables
/// macOS normalizes with, so it is not a hand-rolled table that covers "the
/// common accented Latin cases" and quietly misses everything else —
/// Hangul, Vietnamese multi-diacritic stacks, whatever else CoreFoundation's
/// NFC implementation covers, all go through the same call.
///
/// What this does NOT cover, stated plainly:
/// - **Non-UTF-8 paths.** Note this branch is unreachable in practice:
/// `dedupe_key` passes `to_string_lossy()`, which has already replaced bad
/// bytes with U+FFFD, so CoreFoundation never sees invalid UTF-8. Kept
/// because the function is callable with other input.
/// `CFStringCreateWithBytes` requires valid UTF-8
///   input; a path with invalid UTF-8 bytes falls back to plain
///   lowercasing of the lossy string, same as before this fix. Such a path
///   was already not resolvable to a clean human-readable name, so this
///   does not narrow anything that used to work.
/// - **Any HFS+-inherited normalization quirk.** Apple's older HFS+ format
///   documented (Tech Note TN1150) a non-standard decomposition that
///   excludes a handful of code points from full canonical decomposition.
///   `CFStringNormalize`'s NFC is the Unicode Consortium's standard
///   canonical form; this fix has been verified against the NFC/NFD Latin
///   case this issue actually reports (`café`), not against every such
///   legacy exception, if any still apply on APFS.
/// - **Non-macOS targets.** There is no CoreFoundation to call into off
///   Darwin, so this falls back to the pre-fix plain-lowercase behavior —
///   unchanged from before, not a regression, but also not fixed there.
fn dedupe_key(path: &Path) -> Result<String, ApplyError> {
    let lossy = path.to_string_lossy();
    #[cfg(target_os = "macos")]
    {
        // A review caught the first version falling back to plain lowercasing
        // when normalization failed. That is the pre-fix behaviour restored
        // silently — the exact partial apply this is meant to prevent, made
        // less likely rather than loud. If the OS cannot tell us the
        // normalized form, we do not know whether two destinations collide,
        // and guessing "they don't" is the answer that moves files.
        return macos_unicode::normalize_nfc(&lossy)
            .map(|nfc| nfc.to_lowercase())
            .ok_or_else(|| ApplyError::CannotCompareNames(path.to_path_buf()));
    }
    #[cfg(not(target_os = "macos"))]
    Ok(lossy.to_lowercase())
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

/// Raw FFI into CoreFoundation, linked directly with no crates.io
/// dependency (same pattern as the `getuid`/`utimes` calls elsewhere in this
/// crate). Used only to ask macOS to normalize a string to NFC — see the
/// doc comment on `dedupe_key` above for why.
#[cfg(target_os = "macos")]
mod macos_unicode {
    use std::ffi::{CStr, c_void};
    use std::os::raw::{c_char, c_uchar};

    type CFIndex = isize;
    type CFStringRef = *const c_void;
    type CFMutableStringRef = *mut c_void;
    type CFAllocatorRef = *const c_void;
    type CFStringEncoding = u32;
    type CFStringNormalizationForm = i32;
    type Boolean = c_uchar;

    const K_CF_STRING_ENCODING_UTF8: CFStringEncoding = 0x0800_0100;
    const K_CF_STRING_NORMALIZATION_FORM_C: CFStringNormalizationForm = 2;

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithBytes(
            alloc: CFAllocatorRef,
            bytes: *const c_uchar,
            num_bytes: CFIndex,
            encoding: CFStringEncoding,
            is_external_representation: Boolean,
        ) -> CFStringRef;
        fn CFStringCreateMutableCopy(
            alloc: CFAllocatorRef,
            max_length: CFIndex,
            the_string: CFStringRef,
        ) -> CFMutableStringRef;
        fn CFStringNormalize(the_string: CFMutableStringRef, the_form: CFStringNormalizationForm);
        fn CFStringGetLength(the_string: CFStringRef) -> CFIndex;
        fn CFStringGetMaximumSizeForEncoding(
            length: CFIndex,
            encoding: CFStringEncoding,
        ) -> CFIndex;
        fn CFStringGetCString(
            the_string: CFStringRef,
            buffer: *mut c_char,
            buffer_size: CFIndex,
            encoding: CFStringEncoding,
        ) -> Boolean;
        fn CFRelease(cf: *const c_void);
    }

    /// Returns `s` normalized to NFC via CoreFoundation, or `None` if any
    /// step of the FFI round-trip fails (caller falls back to un-normalized
    /// lowercasing — see `dedupe_key`).
    pub fn normalize_nfc(s: &str) -> Option<String> {
        // SAFETY: all CF calls below are given valid pointers of the type
        // each function expects, and every non-null CFTypeRef we create is
        // released exactly once on every path (including early returns).
        unsafe {
            let bytes = s.as_bytes();
            let immutable = CFStringCreateWithBytes(
                std::ptr::null(),
                bytes.as_ptr(),
                bytes.len() as CFIndex,
                K_CF_STRING_ENCODING_UTF8,
                0,
            );
            if immutable.is_null() {
                return None;
            }
            let mutable = CFStringCreateMutableCopy(std::ptr::null(), 0, immutable);
            CFRelease(immutable);
            if mutable.is_null() {
                return None;
            }

            CFStringNormalize(mutable, K_CF_STRING_NORMALIZATION_FORM_C);

            let len = CFStringGetLength(mutable as CFStringRef);
            let max_size = CFStringGetMaximumSizeForEncoding(len, K_CF_STRING_ENCODING_UTF8) + 1;
            if max_size <= 0 {
                CFRelease(mutable as *const c_void);
                return None;
            }
            // Allocate before the call, but note the release below is the only
            // one on this path: a review pointed out that allocating while
            // still holding `mutable` leaks it if the allocation unwinds.
            // try_reserve would be the belt-and-braces answer; keeping the
            // window to a single Vec::new is the cheap one.
            let mut buf: Vec<u8> = Vec::new();
            if buf.try_reserve_exact(max_size as usize).is_err() {
                CFRelease(mutable as *const c_void);
                return None;
            }
            buf.resize(max_size as usize, 0);
            let ok = CFStringGetCString(
                mutable as CFStringRef,
                buf.as_mut_ptr() as *mut c_char,
                max_size,
                K_CF_STRING_ENCODING_UTF8,
            );
            CFRelease(mutable as *const c_void);
            if ok == 0 {
                return None;
            }
            let cstr = CStr::from_ptr(buf.as_ptr() as *const c_char);
            Some(cstr.to_string_lossy().into_owned())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::normalize_nfc;

        #[test]
        fn nfd_cafe_normalizes_to_nfc_cafe() {
            let nfc_name = "caf\u{00e9}"; // single precomposed U+00E9
            let nfd_name = "cafe\u{0301}"; // e + combining acute U+0301
            assert_ne!(nfc_name.as_bytes(), nfd_name.as_bytes());
            assert_eq!(normalize_nfc(nfd_name).as_deref(), Some(nfc_name));
            assert_eq!(normalize_nfc(nfc_name).as_deref(), Some(nfc_name));
        }

        #[test]
        fn plain_ascii_round_trips() {
            assert_eq!(normalize_nfc("Report.pdf").as_deref(), Some("Report.pdf"));
        }
    }
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
