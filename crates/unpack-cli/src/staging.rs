//! Remove a folder only when this tool can prove it made it, for this exact
//! destination, and that nothing is still writing to it.
use etude_cli_support::lock::ExclusiveLock;
use etude_cli_support::{plain_io_error, quoted_path, shown};
use std::fs::{File, Metadata};
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

const MARKER: &str = "unpack staging lock v1";
// Source: the platform's sys/fcntl.h. O_NONBLOCK avoids waiting on a FIFO
// substituted for the lock, and O_NOFOLLOW refuses a substituted symlink.
#[cfg(target_os = "macos")]
const OPEN_FLAGS: i32 = 0x100 | 0x4;
#[cfg(not(target_os = "macos"))]
const OPEN_FLAGS: i32 = 0x20000 | 0x800;
// flock(2): a volume without advisory locks answers ENOTSUP, EOPNOTSUPP or
// ENOLCK. Numbers from the platform's sys/errno.h.
#[cfg(target_os = "macos")]
const NO_LOCKS_HERE: &[i32] = &[45, 102, 77];
#[cfg(not(target_os = "macos"))]
const NO_LOCKS_HERE: &[i32] = &[95, 37];

pub struct Staging {
    pub path: PathBuf,
    /// None on a volume without advisory locks. Such a folder has no lock file,
    /// so a later run can never prove it abandoned and leaves it in place.
    pub lock: Option<ExclusiveLock>,
    /// The folder itself, held open. Its parent can be moved while unpack
    /// works, and the folder goes with it; this is how it is found again.
    folder: File,
}

/// Where the private folder is now.
pub enum Whereabouts {
    At(PathBuf),
    /// Not at its path, and its new place could not be read or did not prove
    /// to be this folder. It may be gone, or anywhere.
    Unknown,
}

pub fn lock_path(stage: &Path) -> Option<PathBuf> {
    let name = stage.file_name()?.as_bytes();
    let stem = name.strip_suffix(b".partial")?;
    let id = stem.get(stem.len().checked_sub(32)?..)?;
    Some(stage.join(format!(".unpack-{}.lock", std::str::from_utf8(id).ok()?)))
}

/// What the lock file must say. It names the folder it was made for by device
/// and inode, so a copy of a leftover, such as one restored from a backup or
/// brought over in a copied folder, proves nothing: it is someone's copy now.
fn marker_for(directory: &Metadata) -> String {
    format!("{MARKER}\n{}:{}\n", directory.dev(), directory.ino())
}

impl Staging {
    pub fn reserve(dest: &Path, entries: &[String]) -> io::Result<Self> {
        Self::reserve_with(dest, entries, ExclusiveLock::take)
    }

    fn reserve_with(
        dest: &Path,
        entries: &[String],
        take: impl Fn(File) -> io::Result<ExclusiveLock>,
    ) -> io::Result<Self> {
        for _ in 0..128 {
            let path = super::staging_destination(dest)?;
            let lock_path = lock_path(&path).expect("new staging has a nonce");
            // The archive cannot replace the lock, including when its wrapper
            // is flattened. Choose another nonce rather than reserve a name
            // that an ordinary archive member could legitimately need.
            if entries.iter().any(|entry| {
                entry
                    .split('/')
                    .any(|part| part.as_bytes() == lock_path.file_name().unwrap().as_bytes())
            }) {
                std::fs::remove_dir(&path)?;
                continue;
            }
            let initialized = (|| -> io::Result<(Option<ExclusiveLock>, File)> {
                let folder = File::open(&path)?;
                let held = folder.metadata()?;
                if !held.is_dir() || !same(&held, &path.symlink_metadata()?) {
                    return Err(io::Error::other("the private folder changed"));
                }
                let file = File::options()
                    .read(true)
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .custom_flags(OPEN_FLAGS)
                    .open(&lock_path)?;
                let lock = match take(file) {
                    Ok(lock) => lock,
                    Err(error)
                        if error
                            .raw_os_error()
                            .is_some_and(|code| NO_LOCKS_HERE.contains(&code)) =>
                    {
                        // Extraction worked here before locks existed and must
                        // keep working. Only later cleanup is given up.
                        std::fs::remove_file(&lock_path)?;
                        return Ok((None, folder));
                    }
                    Err(error) => return Err(error),
                };
                // An empty or incomplete file is never evidence of death.
                // Publish readiness only while already holding the lock.
                let marker = marker_for(&path.symlink_metadata()?);
                lock.file().write_all(marker.as_bytes())?;
                Ok((Some(lock), folder))
            })();
            return match initialized {
                Ok((lock, folder)) => Ok(Self { path, lock, folder }),
                Err(error) => {
                    if let cleanup @ (super::Cleanup::Remains { .. }
                    | super::Cleanup::Unverified { .. }) = super::cleanup_destination(&path)
                    {
                        eprintln!("unpack: {}", cleanup.message());
                    }
                    Err(error)
                }
            };
        }
        Err(io::Error::other(
            "could not choose a private folder name absent from the archive",
        ))
    }

    pub fn remove_lock(&self) -> io::Result<()> {
        let Some(lock) = &self.lock else {
            return Ok(());
        };
        let path = lock_path(&self.path).expect("reserved staging has a nonce");
        if !same(&lock.file().metadata()?, &path.symlink_metadata()?) {
            return Err(io::Error::other("the private lock file changed"));
        }
        std::fs::remove_file(path)
    }

    /// Where the folder is now: at its path, or wherever it was moved to,
    /// such as with its parent folder. A place counts only when the name
    /// there is this same folder, by device and inode.
    pub fn whereabouts(&self) -> Whereabouts {
        let Ok(held) = self.folder.metadata() else {
            return Whereabouts::Unknown;
        };
        match self.path.symlink_metadata() {
            Ok(md) if same(&held, &md) => return Whereabouts::At(self.path.clone()),
            // Something else has this name now. It is not ours to remove.
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            // The name cannot be checked, so the folder may still be there.
            // Removal will fail the same way and say so for this path.
            Err(_) => return Whereabouts::At(self.path.clone()),
        }
        match current_path(&self.folder) {
            Some(now) if now.symlink_metadata().is_ok_and(|md| same(&held, &md)) => {
                Whereabouts::At(now)
            }
            // A removed folder can still report its last path, so a missing
            // name there does not prove it is gone.
            _ => Whereabouts::Unknown,
        }
    }
}

/// The path the system has for an open folder now, following renames of the
/// folder or of any folder above it.
#[cfg(target_os = "macos")]
fn current_path(folder: &File) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    unsafe extern "C" {
        fn fcntl(fd: i32, cmd: i32, ...) -> i32;
    }
    // F_GETPATH is 50 in the platform's sys/fcntl.h, and it fills a buffer
    // of MAXPATHLEN (1024, sys/param.h) bytes.
    let mut buffer = vec![0_u8; 1024];
    // SAFETY: the descriptor is open for the life of `folder`, and the buffer
    // holds MAXPATHLEN bytes, as F_GETPATH requires.
    if unsafe { fcntl(folder.as_raw_fd(), 50, buffer.as_mut_ptr()) } == -1 {
        return None;
    }
    let end = buffer.iter().position(|&byte| byte == 0)?;
    buffer.truncate(end);
    Some(PathBuf::from(std::ffi::OsString::from_vec(buffer)))
}

#[cfg(not(target_os = "macos"))]
fn current_path(folder: &File) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/self/fd/{}", folder.as_raw_fd())).ok()
}

fn same(left: &Metadata, right: &Metadata) -> bool {
    left.dev() == right.dev() && left.ino() == right.ino()
}

fn owned_directory(path: &Path) -> io::Result<Metadata> {
    let md = etude_core::scan::observe_read(
        "staging_recovery_metadata_and_lock_marker",
        path.symlink_metadata(),
    )?;
    unsafe extern "C" {
        fn geteuid() -> u32;
    }
    // SAFETY: geteuid has no arguments or side effects.
    if !md.is_dir() || md.uid() != unsafe { geteuid() } {
        return Err(io::Error::other("not a real directory owned by this user"));
    }
    Ok(md)
}

/// access(2) with W_OK: whether this user may add and remove names here.
fn writable(folder: &Path) -> bool {
    unsafe extern "C" {
        fn access(path: *const std::ffi::c_char, mode: i32) -> i32;
    }
    let Ok(path) = std::ffi::CString::new(folder.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: the pointer is a valid NUL-terminated string for this call. W_OK
    // is 2 in the platform's sys/unistd.h.
    unsafe { access(path.as_ptr(), 2) == 0 }
}

/// A step that removes this folder when it is run, or why none can be given.
/// `when` opens the sentence: it says in what case to run the step.
fn removal_step(path: &Path, when: &str) -> String {
    let parent = super::folder_of(path);
    let quoted = quoted_path(path);
    if writable(parent) {
        return format!("{when}: chmod -R u+rwx {quoted} && rm -rf {quoted}");
    }
    unsafe extern "C" {
        fn geteuid() -> u32;
    }
    // SAFETY: geteuid has no arguments or side effects.
    let owner = parent
        .metadata()
        .is_ok_and(|md| md.uid() == unsafe { geteuid() });
    if owner {
        format!(
            "{} is read-only, and removing it makes that folder writable. {when}: \
             chmod u+w {} && chmod -R u+rwx {quoted} && rm -rf {quoted}",
            shown(parent),
            quoted_path(parent)
        )
    } else {
        format!(
            "{} is read-only for you and is not yours, so its owner has to remove it.",
            shown(parent)
        )
    }
}

/// What became of one folder with an exact leftover name.
#[derive(Debug)]
enum Reclaim {
    Removed,
    /// Already gone: another run removed it first. Nothing to say about it.
    Gone,
    /// Its lock is held, so an extraction or its surviving writer is alive.
    InUse,
    /// Nothing proves this tool made it, or that its maker is gone.
    Unproved,
    /// It was proved abandoned, but removal did not finish.
    RemoveFailed(io::Error),
}

fn reclaim(path: &Path) -> Reclaim {
    // Missing is the only failure that does not leave the folder in place.
    fn unproved(error: io::Error) -> Reclaim {
        if error.kind() == io::ErrorKind::NotFound {
            Reclaim::Gone
        } else {
            Reclaim::Unproved
        }
    }
    let directory = match owned_directory(path) {
        Ok(directory) => directory,
        Err(error) => return unproved(error),
    };
    let Some(lock_path) = lock_path(path) else {
        return Reclaim::Unproved;
    };
    let marker = marker_for(&directory);
    let file = match File::options()
        .read(true)
        .custom_flags(OPEN_FLAGS)
        .open(&lock_path)
    {
        Ok(file) => file,
        // The folder is there and its lock is not: an older leftover, or one
        // whose removal stopped partway. Neither proves anything.
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Reclaim::Unproved,
        Err(error) => return unproved(error),
    };
    let md = match file.metadata() {
        Ok(md) => md,
        Err(error) => return unproved(error),
    };
    if !md.is_file()
        || md.uid() != directory.uid()
        || md.nlink() != 1
        || md.len() != marker.len() as u64
    {
        return Reclaim::Unproved;
    }
    let lock = match ExclusiveLock::take(file) {
        Ok(lock) => lock,
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Reclaim::InUse,
        Err(error) => return unproved(error),
    };
    let mut contents = Vec::new();
    if let Err(error) = etude_core::scan::observe_read(
        "staging_recovery_metadata_and_lock_marker",
        lock.file()
            .take(marker.len() as u64 + 1)
            .read_to_end(&mut contents),
    ) {
        return unproved(error);
    }
    if contents != marker.as_bytes() {
        return Reclaim::Unproved;
    }
    // Another cleaner may have opened the same lock before we did, removed
    // the tree and released it. Never use that now-unlinked descriptor as
    // permission to remove whatever subsequently occupies the pathname.
    match (owned_directory(path), lock_path.symlink_metadata()) {
        (Ok(now), Ok(lock_now))
            if same(&directory, &now)
                && same(&md, &lock_now)
                && lock.file().metadata().is_ok_and(|held| held.nlink() == 1) => {}
        (Err(error), _) | (_, Err(error)) if error.kind() == io::ErrorKind::NotFound => {
            return Reclaim::Gone;
        }
        _ => return Reclaim::Unproved,
    }
    // Without write access to the parent the folder's own name cannot go, so
    // emptying it would only destroy the proof and the contents.
    if !writable(super::folder_of(path)) {
        return Reclaim::RemoveFailed(io::Error::from(io::ErrorKind::PermissionDenied));
    }
    if let Err(error) = std::fs::remove_dir_all(path)
        && error.kind() != io::ErrorKind::NotFound
    {
        return Reclaim::RemoveFailed(error);
    }
    match path.symlink_metadata() {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Reclaim::Removed,
        Err(error) => Reclaim::RemoveFailed(error),
        Ok(_) => Reclaim::RemoveFailed(io::Error::other("the folder is still there")),
    }
}

/// True for the exact name `.{destination}.unpack-{32 hex}.partial`.
fn is_leftover_name(name: &[u8], destination: &[u8]) -> bool {
    name.strip_prefix(b".")
        .and_then(|rest| rest.strip_prefix(destination))
        .and_then(|rest| rest.strip_prefix(b".unpack-"))
        .and_then(|rest| rest.strip_suffix(b".partial"))
        .is_some_and(|id| {
            id.len() == 32
                && id
                    .iter()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
        })
}

/// Remove what earlier killed runs left for this destination and say so by path.
/// Anything else stays. Nothing here can stop the new extraction.
pub fn recover(dest: &Path) {
    let parent = super::folder_of(dest);
    let destination = dest.file_name().unwrap_or_default().as_bytes();
    let entries = match etude_core::scan::observe_read(
        "staging_recovery_metadata_and_lock_marker",
        std::fs::read_dir(parent),
    ) {
        Ok(entries) => entries,
        // The extraction's own check of this folder says it better.
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return;
        }
        Err(error) => {
            eprintln!(
                "unpack: could not look in {} for folders left by interrupted extractions: {}. \
                 Any there stay in place. Continuing.",
                shown(parent),
                plain_io_error(&error)
            );
            return;
        }
    };
    let (mut removed, mut left) = (0, 0);
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                eprintln!(
                    "unpack: could not finish looking in {} for folders left by interrupted \
                     extractions: {}. Any others stay in place. Continuing.",
                    shown(parent),
                    plain_io_error(&error)
                );
                break;
            }
        };
        let name = entry.file_name();
        // A name that only resembles ours is not ours, so it is not mentioned.
        if !is_leftover_name(name.as_bytes(), destination) {
            continue;
        }
        let path = entry.path();
        match reclaim(&path) {
            Reclaim::Removed => {
                removed += 1;
                eprintln!(
                    "unpack: removed {}, a folder left by an interrupted extraction.",
                    shown(&path)
                );
            }
            Reclaim::Gone => {}
            Reclaim::InUse => {
                left += 1;
                eprintln!(
                    "unpack: left {} in place: another extraction is still using it. Continuing.",
                    shown(&path)
                );
            }
            Reclaim::Unproved => {
                left += 1;
                eprintln!(
                    "unpack: left {} in place: its name looks like a folder from an interrupted \
                     extraction, but unpack cannot prove it made it. Continuing.\n{}",
                    shown(&path),
                    removal_step(
                        &path,
                        "If no other extraction is running and you do not need it"
                    )
                );
            }
            Reclaim::RemoveFailed(error) => {
                left += 1;
                eprintln!(
                    "unpack: could not finish removing {}, a folder from an interrupted \
                     extraction: {}. Continuing.\n{}",
                    shown(&path),
                    plain_io_error(&error),
                    removal_step(&path, "To delete it")
                );
            }
        }
    }
    if removed > 0 {
        etude_cli_support::envelope::effect("leftover_folders_removed", removed);
    }
    if left > 0 {
        etude_cli_support::envelope::effect("leftover_folders_kept", left);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_stage(label: &str) -> (PathBuf, PathBuf) {
        let root =
            std::env::temp_dir().join(format!("unpack-reclaim-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).unwrap();
        let path = super::super::staging_destination(&root.join("out")).unwrap();
        (root, path)
    }

    #[test]
    fn only_initialized_and_unlocked_staging_can_be_reclaimed() {
        let (root, path) = new_stage("initialized");
        let lock_path = lock_path(&path).unwrap();
        let file = File::options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&lock_path)
            .unwrap();
        // A concurrent initializer can have created the file but not yet
        // taken its lock. An empty lock file must never authorize removal.
        assert!(matches!(reclaim(&path), Reclaim::Unproved));
        assert!(path.is_dir());
        let lock = ExclusiveLock::take(file).unwrap();
        lock.file()
            .write_all(marker_for(&path.symlink_metadata().unwrap()).as_bytes())
            .unwrap();
        assert!(matches!(reclaim(&path), Reclaim::InUse));
        assert!(path.is_dir());
        drop(lock);
        // Another test in this process can be between fork and exec with a
        // copy of the lock's descriptor. That copy ends at exec, so wait for
        // it. Only `InUse` is retried, and only for two seconds.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let last = loop {
            let seen = reclaim(&path);
            if !matches!(seen, Reclaim::InUse) || std::time::Instant::now() >= deadline {
                break seen;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert!(matches!(last, Reclaim::Removed));
        assert!(!path.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_marker_for_another_folder_proves_nothing() {
        let (root, path) = new_stage("copied");
        std::fs::write(lock_path(&path).unwrap(), b"unpack staging lock v1\n1:2\n").unwrap();
        std::fs::write(path.join("keep"), b"keep").unwrap();
        assert!(matches!(reclaim(&path), Reclaim::Unproved));
        assert_eq!(std::fs::read(path.join("keep")).unwrap(), b"keep");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_folder_already_removed_by_another_run_is_not_reported_as_left() {
        let (root, path) = new_stage("gone");
        std::fs::remove_dir(&path).unwrap();
        assert!(matches!(reclaim(&path), Reclaim::Gone));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_name_that_only_resembles_ours_is_not_a_leftover() {
        let hex = "0123456789abcdef0123456789abcdef";
        let exact = format!(".out.unpack-{hex}.partial");
        assert!(is_leftover_name(exact.as_bytes(), b"out"));
        for other in [
            format!(".other.unpack-{hex}.partial"),
            format!(".out.unpack-{}.partial", hex.to_uppercase()),
            format!(".out.unpack-{hex}0.partial"),
            format!(".out.unpack-{hex}"),
            format!("out.unpack-{hex}.partial"),
            format!(".out.unpack-x.unpack-{hex}.partial"),
            ".out.unpack-notes.partial".to_owned(),
        ] {
            assert!(!is_leftover_name(other.as_bytes(), b"out"), "{other}");
        }
    }

    #[test]
    fn a_leftover_name_keeps_the_exact_bytes_of_its_destination() {
        use std::ffi::OsStr;
        let hex = "0123456789abcdef0123456789abcdef";
        let raw = Path::new(OsStr::from_bytes(b"dir/\xff"));
        let name = super::super::staging_name(raw, hex);
        assert!(is_leftover_name(name.as_bytes(), b"\xff"));
        // Two destinations that read the same once made lossy stay two.
        assert!(!is_leftover_name(name.as_bytes(), "\u{fffd}".as_bytes()));
        assert!(!is_leftover_name(name.as_bytes(), b"\xfe"));
    }

    #[test]
    fn a_volume_without_advisory_locks_still_extracts_and_keeps_no_lock_file() {
        let (root, stray) = new_stage("nolock");
        std::fs::remove_dir(&stray).unwrap();
        let dest = root.join("out");
        let staged = Staging::reserve_with(&dest, &[], |_| {
            Err(io::Error::from_raw_os_error(NO_LOCKS_HERE[0]))
        })
        .unwrap();
        assert!(staged.lock.is_none());
        assert!(!lock_path(&staged.path).unwrap().exists());
        staged.remove_lock().unwrap();
        // Later runs cannot prove it abandoned, so it stays.
        assert!(matches!(reclaim(&staged.path), Reclaim::Unproved));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn any_other_lock_failure_stops_the_extraction_and_leaves_nothing() {
        let (root, stray) = new_stage("lockfail");
        std::fs::remove_dir(&stray).unwrap();
        let dest = root.join("out");
        let error = Staging::reserve_with(&dest, &[], |_| Err(io::Error::from_raw_os_error(5)));
        assert!(error.is_err());
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
        std::fs::remove_dir_all(root).unwrap();
    }
}
