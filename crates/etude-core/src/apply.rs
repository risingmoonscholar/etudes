//! Apply and undo.
//!
//! Ordering rule for `apply()`: the journal is written before the first move,
//! and each entry is marked done only after its move succeeds. So the journal
//! never claims a move that did not happen.
//!
//! A crash between a move succeeding and its progress frame reaching disk
//! leaves the entry reading as not done. `undo` recovers that one entry — and
//! only that one — because apply moves in order, so the successor of the last
//! recorded done is the single place a crash can hide. On macOS the move
//! itself is one atomic syscall, so the window only exists at all on the
//! link+unlink fallback path and in journals written before the change.
//!
//! `undo()` persists its own progress per entry and self-heals an entry whose
//! move landed but whose record did not. See its doc comment for exactly what
//! that promises and what it does not.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::journal::{Entry, EntryState, Journal, Method, Sealer, fingerprint};
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
            // The whole error, not e.kind(). kind() renders a Rust-internal
            // category -- "uncategorized error", "entity not found" -- while
            // the error itself renders what the OS actually said: "Operation
            // not permitted (os error 1)". The category names nothing a user
            // can search for and suggests nothing; the OS text does both.
            //
            // Found for real: apply on an exFAT volume reported "io error:
            // uncategorized error" during issue #21, which is why that issue
            // took a syscall-level reproduction to diagnose rather than
            // reading the message. Noted there as a secondary finding and
            // fixed here.
            //
            // On paths: an io::Error raised BY THE OS carries the errno and
            // its text, never the path operated on, so rendering it in full
            // does not weaken the "errors never contain paths unless
            // --explain" claim. Checked directly against a filename designed
            // to be obvious if it appeared.
            //
            // That is a fact about OS-raised errors, not about the type. A
            // review corrected an earlier version of this comment that said
            // io::Error can never hold a path: io::Error::other(msg) takes
            // arbitrary text and would happily carry one. Nothing in this
            // crate does that today -- every custom payload here is a fixed
            // string ("path contains a NUL byte", "cross-device copy size
            // mismatch") -- but the guarantee lives in what we construct,
            // not in what the type forbids. Anyone adding an
            // io::Error::other with a formatted path is the one who breaks
            // it, and an_io_error_names_the_os_reason_without_naming_the_path
            // is what should catch them.
            ApplyError::Io(e) => write!(f, "io error: {e}"),
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
                "refused: cannot compare {} against the other destinations. The system \
                 would not normalise the name. A collision cannot be ruled out",
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
        // A journal being written now has no tail to have lost.
        progress_tail_damaged: false,
    };

    // Build the full entry list first, so the journal describes the whole
    // intended operation before any of it happens.
    // Ask the destination filesystem once, not once per file.
    let folds = folds_case(&plan.root);
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
            if !planned_destinations.insert(dedupe_key(&dst, folds)?) {
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
                state: EntryState::Planned,
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
        j.entries[i].state = EntryState::Moved;
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

/// Whether `dir` treats two names differing only in case as the same entry.
///
/// Asked, not assumed. The previous version lowercased every destination
/// unconditionally. That is right on APFS and wrong on ext4. On ext4,
/// `Report.pdf` and `report.pdf` are two genuinely different files. Folding
/// them made `apply` refuse a legal plan on the default filesystem of
/// every Linux machine. The macOS suite could not see it: APFS collapses those
/// two names into one directory entry, so the fixture only ever had one file.
///
/// Same mistake as trusting `$HOME` to say where home is. A property of the
/// machine you happen to be on is not a property of the world, and the fix is
/// to ask rather than to guess better.
///
/// When it cannot be determined (an unwritable directory, a probe that will
/// not create), the answer is "folds". That refuses more than necessary rather
/// than missing a real collision, and refusing is the direction this tool is
/// allowed to be wrong in.
fn folds_case(dir: &Path) -> bool {
    use std::process;
    let stem = format!(".etudes-case-probe-{}", process::id());
    let lower = dir.join(&stem);
    let upper = dir.join(stem.to_uppercase());

    // A probe that cannot be created tells us nothing, so assume folding.
    if fs::write(&lower, b"").is_err() {
        return true;
    }
    let folds = upper.exists();
    let _ = fs::remove_file(&lower);
    folds
}

/// Case- and composition-insensitive dedupe key for a planned destination
/// path. Issue #9.
///
/// `to_lowercase` folds ASCII case correctly (`Report.pdf` == `report.PDF`),
/// but it does not touch Unicode *normalization*. "café" typed as one
/// precomposed code point (NFC, U+00E9) and as `e` followed by a combining
/// acute accent (NFD, U+0065 U+0301) are different byte strings in Rust, but
/// APFS treats them as the exact same directory entry. The guard that used
/// plain `to_lowercase` saw two distinct destinations. It let the plan through,
/// moved the first file for real, then hit EEXIST on the second mid-run.
///
/// `etude-core` carries zero dependencies (see `Cargo.toml`), so this does
/// not pull in a Unicode-normalization crate. Instead, on macOS, it asks the
/// OS itself: `CFStringNormalize` from CoreFoundation, linked directly (the
/// same raw-FFI pattern `scan.rs` already uses for `getuid` and
/// `journal.rs` for `utimes`: no build.rs, no crates.io dependency, just a
/// framework the OS already ships). This uses the actual Unicode tables
/// macOS normalizes with. It is not a hand-rolled table that covers "the
/// common accented Latin cases" and quietly misses everything else. Hangul,
/// Vietnamese multi-diacritic stacks, and anything else CoreFoundation's NFC
/// implementation covers all go through the same call.
///
/// What this does NOT cover, stated plainly:
/// - **Non-UTF-8 paths.** Unreachable in practice, and the earlier version of
///   this note claimed otherwise. `dedupe_key` passes `to_string_lossy()`,
///   which has already replaced bad bytes with U+FFFD, so CoreFoundation never
///   sees invalid UTF-8. The branch stays because the function is callable
///   with other input. `CFStringCreateWithBytes` requires valid UTF-8
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
///   Darwin. This falls back to the pre-fix plain-lowercase behavior. That
///   behavior is unchanged from before. It is not a regression, but also
///   not fixed there.
fn dedupe_key(path: &Path, folds_case: bool) -> Result<String, ApplyError> {
    let lossy = path.to_string_lossy();
    let cased = |s: String| if folds_case { s.to_lowercase() } else { s };
    #[cfg(target_os = "macos")]
    {
        // A review caught the first version falling back to plain lowercasing
        // when normalization failed. That is the pre-fix behaviour restored
        // silently. That is the exact partial apply this is meant to prevent,
        // made less likely rather than loud. If the OS cannot tell us the
        // normalized form, we do not know whether two destinations collide,
        // and guessing "they don't" is the answer that moves files.
        macos_unicode::normalize_nfc(&lossy)
            .map(cased)
            .ok_or_else(|| ApplyError::CannotCompareNames(path.to_path_buf()))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(cased(lossy.into_owned()))
    }
}

/// link → unlink within one device; copy → verify → unlink across devices.
/// One atomic, non-clobbering rename on macOS.
///
/// `renamex_np` with `RENAME_EXCL` is the whole point of issue #5's fix: the
/// previous strategy was `link` then `unlink`, two syscalls, and a signal
/// between them left one file reachable by both names. Undo then had to guess
/// whose hard link the extra name was — a question with no safe answer. One
/// syscall means there is no in-between state to interpret, and `RENAME_EXCL`
/// refuses an existing destination at the filesystem level, backing up the
/// plan-level collision check with one that cannot be raced.
///
/// A future tidy-up replacing this with `fs::rename` would silently clobber on
/// collision and reopen nothing else — `fs::rename` overwrites. The test
/// `a_plain_rename_never_replaces_this` in apply_undo.rs fails on that tidy-up.
#[cfg(target_os = "macos")]
fn rename_excl(from: &Path, to: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    unsafe extern "C" {
        fn renamex_np(
            from: *const core::ffi::c_char,
            to: *const core::ffi::c_char,
            flags: u32,
        ) -> i32;
    }
    const RENAME_EXCL: u32 = 0x0000_0004;
    let f = CString::new(from.as_os_str().as_bytes())
        .map_err(|_| io::Error::other("path contains a NUL byte"))?;
    let t = CString::new(to.as_os_str().as_bytes())
        .map_err(|_| io::Error::other("path contains a NUL byte"))?;
    // SAFETY: both pointers are valid NUL-terminated strings for the call.
    let rc = unsafe { renamex_np(f.as_ptr(), t.as_ptr(), RENAME_EXCL) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Test-only window onto `move_one`, so the no-clobber promise its comment
/// makes can be asserted from an integration test without making the real
/// function public API.
#[doc(hidden)]
pub fn move_one_for_tests(from: &Path, to: &Path) -> io::Result<Method> {
    move_one(from, to)
}

/// A minimal, explicit cross-device copy on macOS: data and POSIX stat
/// (mode, mtime) only. Deliberately not `fs::copy`.
///
/// `fs::copy`'s own documentation states its macOS mapping to
/// `fcopyfile`/`fclonefileat` is an implementation detail that "may change
/// in the future" (doc.rust-lang.org/std/fs/fn.copy.html) -- not a
/// contract. In practice it requests `COPYFILE_ALL`, which also copies the
/// SOURCE's own pre-existing extended attributes (Finder tags, comments,
/// any prior quarantine flag). sweep has no reason to propagate those.
///
/// This does not, and per issue #20 cannot, avoid every side effect on a
/// destination that cannot store extended attributes (exFAT, FAT32):
/// verified directly against a real exFAT volume that macOS's own
/// quarantine/provenance subsystem tags files written by a non-Apple-signed
/// process with `com.apple.provenance` independent of which copyfile()
/// flags are requested, and that the OS represents that tag as a `._`
/// AppleDouble sidecar file on a filesystem that cannot hold it natively.
/// No flag combination suppressed it in that test, including a raw
/// read-then-write with no `copyfile()` call at all. What this DOES
/// guarantee, and what undo depends on: mtime survives the copy, so the
/// fingerprint recorded at plan time still matches at undo time.
///
/// Source for the flags: `man copyfile`, and the exact bit values, taken
/// from the real SDK header rather than assumed --
/// `.../MacOSX.sdk/usr/include/copyfile.h`:
/// `COPYFILE_STAT (1<<1)`, `COPYFILE_DATA (1<<3)`.
// Module-level, not local to copy_data_and_stat, so a unit test can bind to
// the actual production value instead of redeclaring its own copy that
// could silently drift from what the real call site requests. A review
// caught an earlier version of the test doing exactly that -- asserting
// against local constants of the same name, which would stay green even if
// the real call site changed.
// ACL and XATTR are unused outside the test that proves they are NOT
// requested -- that absence is the point, so #[allow(dead_code)] rather
// than deleting them, which would silently drop the thing being asserted.
#[cfg(target_os = "macos")]
#[allow(dead_code)]
const COPYFILE_ACL: u32 = 1 << 0;
#[cfg(target_os = "macos")]
const COPYFILE_STAT: u32 = 1 << 1;
#[cfg(target_os = "macos")]
#[allow(dead_code)]
const COPYFILE_XATTR: u32 = 1 << 2;
#[cfg(target_os = "macos")]
const COPYFILE_DATA: u32 = 1 << 3;
/// What copy_data_and_stat actually requests. Read by the function AND by
/// the test that checks it excludes XATTR/ACL, so the two cannot drift.
#[cfg(target_os = "macos")]
const COPY_DATA_AND_STAT_FLAGS: u32 = COPYFILE_STAT | COPYFILE_DATA;

#[cfg(target_os = "macos")]
fn copy_data_and_stat(from: &Path, to: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    unsafe extern "C" {
        fn copyfile(
            from: *const core::ffi::c_char,
            to: *const core::ffi::c_char,
            state: *mut core::ffi::c_void,
            flags: u32,
        ) -> i32;
    }
    let f = CString::new(from.as_os_str().as_bytes())
        .map_err(|_| io::Error::other("path contains a NUL byte"))?;
    let t = CString::new(to.as_os_str().as_bytes())
        .map_err(|_| io::Error::other("path contains a NUL byte"))?;
    // SAFETY: both pointers are valid NUL-terminated strings for the call,
    // and a null state pointer is documented as valid -- "less control...
    // but [it] will work normally".
    let rc = unsafe {
        copyfile(
            f.as_ptr(),
            t.as_ptr(),
            std::ptr::null_mut(),
            COPY_DATA_AND_STAT_FLAGS,
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn move_one(from: &Path, to: &Path) -> io::Result<Method> {
    // One syscall, no crash window, refuses to clobber. See rename_excl.
    #[cfg(target_os = "macos")]
    {
        match rename_excl(from, to) {
            Ok(()) => return Ok(Method::Rename),
            // EXDEV: cross-device, fall through to the copy path below.
            Err(e) if e.raw_os_error() == Some(18) => {}
            // ENOTSUP: a filesystem without renamex_np. Fall back to
            // link+unlink, which keeps the old crash window on that volume
            // only; undo's successor-entry recovery covers it.
            Err(e) if e.raw_os_error() == Some(45) => {
                return move_one_link_unlink(from, to);
            }
            Err(e) => return Err(e),
        }
    }
    #[cfg(not(target_os = "macos"))]
    if fs::symlink_metadata(from)?.file_type().is_file() {
        return move_one_link_unlink(from, to);
    }

    match fs::rename(from, to) {
        Ok(()) => Ok(Method::Rename),
        Err(e) if e.raw_os_error() == Some(18) => {
            // EXDEV: cross-device. rename(2) cannot do this, so copy, verify
            // the copy landed intact, and only then unlink the source.
            //
            // Explicit data+stat copy on macOS, not fs::copy: see
            // copy_data_and_stat's doc comment (issue #20) for the sourced
            // reason and what it does and does not guarantee.
            #[cfg(target_os = "macos")]
            copy_data_and_stat(from, to)?;
            #[cfg(not(target_os = "macos"))]
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

/// The pre-#5 strategy, kept only as the fallback for filesystems without
/// `renamex_np` and for non-macOS builds. Two syscalls, so a crash between
/// them leaves one file under two names; undo's successor-entry recovery is
/// what makes that survivable.
fn move_one_link_unlink(from: &Path, to: &Path) -> io::Result<Method> {
    if fs::symlink_metadata(from)?.file_type().is_file() {
        match fs::hard_link(from, to) {
            Ok(()) => {
                fs::remove_file(from)?;
                return Ok(Method::Rename);
            }
            Err(e) if e.raw_os_error() == Some(18) => {}
            // ENOTSUP (45): the filesystem has no hard-link concept at all.
            // Verified directly against a real exFAT volume -- exFAT
            // returns exactly this, not EXDEV, and this branch was missing
            // it while move_one's OTHER call site already handled it
            // (issue #21). Without this, apply failed outright rather than
            // falling back to a plain rename on any exFAT or FAT32
            // destination -- not cosmetic junk, a hard failure.
            Err(e) if e.raw_os_error() == Some(45) => {}
            Err(e) => return Err(e),
        }
    }
    match fs::rename(from, to) {
        Ok(()) => Ok(Method::Rename),
        Err(e) => Err(e),
    }
}

#[derive(Debug, Default)]
#[must_use = "check `error`: undo() no longer returns a Result, so a failed \
              partial restore is silent unless this field is inspected"]
pub struct UndoReport {
    pub restored: usize,
    /// Entries a previous, killed undo had already put back. Counted rather
    /// than re-walked: issue #7 was that these looked identical to entries
    /// never moved, so every resumed run reported the same work forever.
    pub already_reversed: usize,
    /// Entries a killed undo had moved home without recording, found by the
    /// origin matching the journal's fingerprint. Distinct from `healed`,
    /// which is one file that was reachable by two names — these are files
    /// already where they belong, with only the record missing.
    pub reconciled: Vec<PathBuf>,
    /// Entries skipped because the file changed after apply. Never forced.
    pub skipped_changed: Vec<PathBuf>,
    /// Entries whose destination no longer exists.
    pub skipped_missing: Vec<PathBuf>,
    /// Half-moves collapsed: a crash between the link and the unlink left one
    /// file reachable by two names, and undo removed the extra name. Reported
    /// separately from `restored` because nothing was moved, something was
    /// tidied, and a user deserves to know the difference. See `reconciled`
    /// for the other shape, where the file was already home.
    pub healed: Vec<PathBuf>,
    /// Set when a move failed and undo stopped early. `restored` and the
    /// `skipped_*` lists above still describe everything that happened
    /// *before* the failure. They are never discarded just because the walk
    /// did not finish. Each reversal is already on disk by the time this is
    /// set, appended as it happened, so a caller that gives up here still
    /// leaves a journal that agrees with the filesystem.
    pub error: Option<ApplyError>,
    /// Set when the journal lost more than its final in-flight record, and
    /// undo therefore did nothing at all.
    ///
    /// A crash between a move and its record loses exactly one record: apply
    /// moves then appends, so only the entry it was working on can be missing.
    /// Undo's successor-entry recovery covers precisely that entry.
    ///
    /// A journal missing MORE than that was not left by a crash mid-write. Its
    /// remaining entries read Planned while their files sit at their
    /// destinations, and recovery reaches only the first of them -- so
    /// proceeding would restore a few, strand the rest, and exit 0. That is a
    /// silent half-restore, which is worse than the refusal it would replace.
    ///
    /// Holds how many entries look moved-but-unrecorded, because the number is
    /// the evidence: one is a crash, several is a damaged file.
    pub unrecorded_moves: usize,
}

/// Whether two paths are the same file on disk, by device AND inode.
///
/// Inode alone is not identity: numbers are only unique within a filesystem,
/// so two files on different volumes can share one. `scan` already pairs them
/// and a review caught this not doing so.
#[cfg(unix)]
fn same_file(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (fs::symlink_metadata(a), fs::symlink_metadata(b)) {
        (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
        _ => false,
    }
}

#[cfg(not(unix))]
fn same_file(_a: &Path, _b: &Path) -> bool {
    false
}

/// Reverse a journal, verifying each file before touching it.
///
/// A file that changed since apply is **reported and skipped**, never
/// overwritten.
///
/// Always returns a report, even when a move fails partway through: this
/// never discards what it already did.
///
/// Progress is persisted per entry via `record_undone`, mirroring what
/// `apply()` does with `record_done`, so a killed run resumes where it
/// stopped instead of walking everything again. That is why the sealer is
/// required rather than optional: an undo that cannot write its progress is
/// the exact defect issue #7 filed, and passing `None` used to reintroduce it
/// silently — including in this crate's own crash-safety example.
///
/// Saving the whole journal afterwards is still worthwhile (it rewrites the
/// base frame and drops the accumulated progress records), but it is no
/// longer what makes the on-disk state correct.
/// Entries that say "not moved" while the file sits at the destination.
///
/// The journal cannot answer this and the filesystem can. A file truncated on
/// a frame boundary is byte-identical to one that stopped there, so a journal
/// missing every record looks exactly like an apply that never moved anything
/// -- and the difference is visible only on disk.
///
/// Exactly one is the signature of a crash between a move and its record:
/// apply moves then appends, so only the entry in flight can be missing.
/// Several means records were lost.
///
/// Public because the CLIs must ask BEFORE deciding there is nothing to
/// reverse. Without it, a journal cut back to its base frame reports "already
/// restored" while every file is still at its destination -- which is not a
/// half-restore but a plain untruth, and it exits 1 as though all were well.
pub fn unrecorded_moves(j: &Journal) -> usize {
    j.entries
        .iter()
        .filter(|e| e.state == EntryState::Planned && e.to.exists() && !e.from.exists())
        .count()
}

pub fn undo(j: &mut Journal, sealer: &dyn Sealer) -> UndoReport {
    let mut r = UndoReport::default();

    // Apply moves entries in order and seals a done record after each, so
    // after a crash exactly one entry can be mid-flight: the successor of the
    // last recorded done. That position is the authorship proof the first
    // version of this recovery lacked. Sweep wrote down that it was about to
    // create precisely this link at precisely this path, and the plan-time
    // collision check had verified the destination was empty. Inference from
    // inode equality alone could not distinguish sweep's interrupted link
    // from a hard link the user made; position can.
    let first_planned = j
        .entries
        .iter()
        .position(|e| e.state == EntryState::Planned);

    // How many entries say "not moved" while the file is sitting at the
    // destination anyway? Exactly one is the signature of a crash between a
    // move and its record, and recovery below handles it. More than one means
    // the journal lost records it should have had, and the filesystem is the
    // only thing that can say so -- the bytes cannot, since a file truncated
    // on a frame boundary is indistinguishable from one that stopped there.
    //
    // Refuse before touching anything. Restoring the reachable ones and
    // leaving the rest is the half-restore this check exists to prevent; it
    // was measured at 3 restored and 17 stranded with exit 0.
    let unrecorded = unrecorded_moves(j);
    if unrecorded > 1 {
        r.unrecorded_moves = unrecorded;
        return r;
    }

    // Reverse order, so nested destinations empty before their parents.
    for i in (0..j.entries.len()).rev() {
        let e = j.entries[i].clone();
        if !e.is_moved() {
            // Only the successor entry gets recovery. Everything past it never
            // started, and no inference is applied there — which is what keeps
            // a user's own hard links out of reach.
            if e.state == EntryState::Reversed {
                // Undo already put this one back, on an earlier run that was
                // killed before it finished. Nothing to do, and crucially no
                // recovery inference either: the file is home and the journal
                // says so.
                r.already_reversed += 1;
                continue;
            }
            if Some(i) != first_planned {
                continue;
            }
            if e.to.exists() && e.from.exists() && same_file(&e.to, &e.from) {
                // The interrupted link+unlink of the fallback path: one file,
                // two names, and the journal says sweep made the second.
                match fs::remove_file(&e.to) {
                    Ok(()) => r.healed.push(e.to.clone()),
                    // Do not swallow it. Undo reporting a tidy it did not
                    // manage is the shape this whole fix exists to remove.
                    Err(err) => {
                        r.error = Some(ApplyError::Io(err));
                        break;
                    }
                }
            } else if e.to.exists() && !e.from.exists() {
                // Issue #14's shape: the move landed and the crash beat the
                // done record. The fingerprint must still match what apply
                // recorded, or this is not the file sweep moved.
                if let Ok((size, mtime, _ino, hash)) = fingerprint(&e.to)
                    && size == e.size
                    && mtime == e.mtime_secs
                    && hash == e.edge_hash
                {
                    if let Some(parent) = e.from.parent()
                        && let Err(err) = fs::create_dir_all(parent)
                    {
                        r.error = Some(ApplyError::Io(err));
                        break;
                    }
                    match move_one(&e.to, &e.from) {
                        Ok(_) => r.restored += 1,
                        Err(err) => {
                            r.error = Some(ApplyError::Io(err));
                            break;
                        }
                    }
                }
            }
            // Only `from` exists: the move never started. Nothing to do.
            continue;
        }
        if !e.to.exists() {
            // The destination is gone and the entry still says Moved. That is
            // consistent with a killed undo having moved the file home in the
            // gap between the move and its record — but consistent is not
            // proof, and a review caught the first version of this treating it
            // as proof.
            //
            // Healing on absence alone can abandon a live file: if anything at
            // all sits at the origin, the entry gets written off and every
            // later run says nothing is left to undo while the file is still
            // at its destination. So the origin has to BE the file the journal
            // describes, checked the way the restore path checks a destination.
            //
            // Inode is deliberately not compared. A cross-device undo copies
            // rather than renames, so it legitimately changes; size, mtime and
            // the edge hash survive either path.
            let is_the_file = e.from.exists()
                && match fingerprint(&e.from) {
                    Ok((size, mtime, _inode, hash)) => {
                        size == e.size && mtime == e.mtime_secs && hash == e.edge_hash
                    }
                    Err(_) => false,
                };
            if is_the_file {
                j.entries[i].state = EntryState::Reversed;
                if let Err(err) = j.record_undone(i, sealer) {
                    r.error = Some(ApplyError::Journal(err));
                    break;
                }
                r.reconciled.push(e.from.clone());
                continue;
            }
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
            // Something is at the origin. A review talked me out of treating a
            // same-inode pair here as a half-move to collapse: at this point
            // the entry says the move COMPLETED, so both names existing is far
            // more likely a hard link the user made themselves than a crash,
            // and deleting one would remove a name they meant to have.
            //
            // The crash case leaves `done` false and is handled above. This
            // stays a refusal.
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
                j.entries[i].state = EntryState::Reversed;
                r.restored += 1;
                // Persist it now, not at the end. Issue #7: a kill here used
                // to leave the journal claiming every entry was still at its
                // destination, so the next run walked them all again and the
                // count never converged.
                if let Err(err) = j.record_undone(i, sealer) {
                    r.error = Some(ApplyError::Journal(err));
                    break;
                }
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
// unlikely. Not impossible: pid reuse after wraparound with a repeated or
// backward-stepping clock reading remains a theoretical residual risk.
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
/// crate). Used only to ask macOS to normalize a string to NFC. See the
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
    /// lowercasing, see `dedupe_key`).
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
    /// Two claims at once, because the fix for one could break the other.
    ///
    /// An io error must render what the OS said, not Rust's internal
    /// category: "uncategorized error" is what apply reported on an exFAT
    /// volume during issue #21, and it names nothing a user can act on or
    /// search for.
    ///
    /// And it must still not carry the path. `sweep --quiet` promising it
    /// never prints a filename, and the readme promising errors carry no
    /// paths without --explain, both depend on that -- so the test that
    /// proves the message got MORE informative also proves it did not get
    /// more revealing.
    #[test]
    fn an_io_error_names_the_os_reason_without_naming_the_path() {
        let secret = std::path::Path::new("/nonexistent/TAX_RETURN_2024_SSN.pdf");
        let io_err = fs::metadata(secret).expect_err("this path must not exist");
        let rendered = format!("{}", ApplyError::Io(io_err));

        assert!(
            rendered.contains("os error"),
            "the OS's own reason must survive into the message, got: {rendered}"
        );
        assert!(
            !rendered.contains("TAX_RETURN"),
            "an error must never carry the path it failed on, got: {rendered}"
        );
    }

    /// The narrower, deterministic half of issue #20's claim: that
    /// copy_data_and_stat does not request COPYFILE_XATTR (1<<2) or
    /// COPYFILE_ACL (1<<0). A review asked for this, correctly noting that
    /// the OS-level AppleDouble behaviour itself is not something a unit
    /// test can pin down -- it depends on macOS's own provenance-tracking
    /// state for the calling process, verified only against a real exFAT
    /// volume in the stress scenarios. What a unit test CAN pin down, and
    /// should, is that this crate is not the one asking for xattrs or ACLs
    /// to be copied. If a later edit adds COPYFILE_XATTR back to "fix"
    /// something, this is what should turn red first.
    #[cfg(target_os = "macos")]
    #[test]
    fn copy_data_and_stat_does_not_request_xattr_or_acl() {
        // Binds to COPY_DATA_AND_STAT_FLAGS, the actual value the real
        // function passes to copyfile() -- not a redeclared local copy. A
        // review caught the first version of this test asserting against
        // its own constants of the same name, which could not have gone red
        // if the real call site changed. If a later edit adds
        // COPYFILE_XATTR back to COPY_DATA_AND_STAT_FLAGS to "fix"
        // something, this is what turns red first.
        assert_eq!(
            COPY_DATA_AND_STAT_FLAGS & COPYFILE_XATTR,
            0,
            "must not request COPYFILE_XATTR"
        );
        assert_eq!(
            COPY_DATA_AND_STAT_FLAGS & COPYFILE_ACL,
            0,
            "must not request COPYFILE_ACL"
        );
        assert_ne!(
            COPY_DATA_AND_STAT_FLAGS & COPYFILE_STAT,
            0,
            "must request COPYFILE_STAT (mtime)"
        );
        assert_ne!(
            COPY_DATA_AND_STAT_FLAGS & COPYFILE_DATA,
            0,
            "must request COPYFILE_DATA"
        );
    }

    /// Issue #20's contract: data and mtime survive, and nothing else is
    /// asked for. Same-device is enough to prove data, mtime and the
    /// no-clobber-required shape; the exFAT sidecar behaviour itself needs a
    /// real foreign filesystem and lives in
    /// stress/scenarios/60-cross-device-copy-and-mtime.sh (and the
    /// same-device fallback in
    /// stress/scenarios/60-exfat-same-device-rename-fallback.sh), which
    /// this module cannot build (macOS's hdiutil is a process, not a
    /// syscall this crate should be spawning from a unit test).
    #[cfg(target_os = "macos")]
    #[test]
    fn copy_data_and_stat_preserves_data_and_mtime() {
        let dir = std::env::temp_dir().join(format!("etudes_copyflags_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("mkdir");
        let from = dir.join("src.txt");
        let to = dir.join("dst.txt");
        fs::write(&from, b"hello").expect("write");

        // Backdate the source so "the destination just happened to get a
        // fresh timestamp" cannot pass this test by accident.
        let past = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(946_684_800);
        let f = fs::File::open(&from).expect("open");
        f.set_modified(past).expect("set mtime");
        drop(f);

        copy_data_and_stat(&from, &to).expect("copy");

        assert_eq!(
            fs::read(&to).expect("read dst"),
            b"hello",
            "data must survive"
        );
        let dst_mtime = fs::metadata(&to)
            .expect("stat dst")
            .modified()
            .expect("mtime");
        assert_eq!(
            dst_mtime, past,
            "mtime must survive: undo's fingerprint check compares the \
             mtime recorded at plan time against the destination at undo \
             time, and a copy that resets it would make every cross-device \
             move look changed and never restore"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_probe_agrees_with_the_filesystem_it_is_asked_about() {
        // The first version of this test asserted the probe returns true,
        // because "the default macOS volume folds case". That is a property of
        // the machine it was written on. It failed on the ubuntu job. That is
        // precisely the bug this probe exists to fix, committed into the
        // test for that fix. Worth leaving the note.
        //
        // The property is not "folds" or "does not fold". It is that the probe
        // agrees with the filesystem in front of it, on whatever machine that
        // is.
        let dir = std::env::temp_dir().join(format!("etudes_case_probe_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("mkdir");

        // Observe the truth independently, with different names than the probe
        // uses, so this is a second opinion rather than the same call twice.
        fs::write(dir.join("witness-lower"), b"").expect("write");
        let actually_folds = dir.join("WITNESS-LOWER").exists();
        fs::remove_file(dir.join("witness-lower")).expect("cleanup");

        assert_eq!(
            folds_case(&dir),
            actually_folds,
            "the probe disagreed with the filesystem it was asked about"
        );

        // And it must not leave its probe behind.
        let leftovers: Vec<_> = fs::read_dir(&dir).unwrap().flatten().collect();
        assert!(
            leftovers.is_empty(),
            "the probe left files behind: {:?}",
            leftovers.iter().map(|e| e.file_name()).collect::<Vec<_>>()
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unwritable_directory_is_treated_as_folding() {
        // Cannot ask, so assume the answer that refuses more rather than the
        // one that might miss a real collision.
        assert!(
            folds_case(std::path::Path::new("/nonexistent-etudes-probe-dir")),
            "an unaskable directory must fall back to folding"
        );
    }
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
            skipped_system: 0,
            skipped_unreadable: 0,
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
