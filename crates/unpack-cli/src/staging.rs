//! Reclaim only initialized, unlocked staging for this exact destination.
use etude_cli_support::lock::ExclusiveLock;
use std::fs::{File, Metadata};
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

const READY: &[u8] = b"unpack staging lock v1\n";
// Source: the platform's sys/fcntl.h. O_NONBLOCK avoids waiting on a FIFO
// substituted for the lock, and O_NOFOLLOW refuses a substituted symlink.
#[cfg(target_os = "macos")]
const OPEN_FLAGS: i32 = 0x100 | 0x4;
#[cfg(not(target_os = "macos"))]
const OPEN_FLAGS: i32 = 0x20000 | 0x800;

pub struct Staging {
    pub path: PathBuf,
    pub lock: ExclusiveLock,
}

pub fn lock_path(stage: &Path) -> Option<PathBuf> {
    let name = stage.file_name()?.as_bytes();
    let stem = name.strip_suffix(b".partial")?;
    let id = stem.get(stem.len().checked_sub(32)?..)?;
    Some(stage.join(format!(".unpack-{}.lock", std::str::from_utf8(id).ok()?)))
}

impl Staging {
    pub fn reserve(dest: &Path, entries: &[String]) -> io::Result<Self> {
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
            let initialized = (|| -> io::Result<ExclusiveLock> {
                let file = File::options()
                    .read(true)
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .custom_flags(OPEN_FLAGS)
                    .open(lock_path)?;
                let lock = ExclusiveLock::take(file)?;
                // An empty or incomplete file is never evidence of death.
                // Publish readiness only while already holding the lock.
                lock.file().write_all(READY)?;
                Ok(lock)
            })();
            return match initialized {
                Ok(lock) => Ok(Self { path, lock }),
                Err(error) => {
                    if let cleanup @ (super::Cleanup::Remains { .. } | super::Cleanup::Unverified { .. }) =
                        super::cleanup_destination(&path)
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
        let path = lock_path(&self.path).expect("reserved staging has a nonce");
        if !same(&self.lock.file().metadata()?, &path.symlink_metadata()?) {
            return Err(io::Error::other("staging lock identity changed"));
        }
        std::fs::remove_file(path)
    }
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

fn reclaim(path: &Path) -> io::Result<()> {
    let directory = owned_directory(path)?;
    let lock_path = lock_path(path).ok_or_else(|| io::Error::other("invalid staging name"))?;
    let file = File::options()
        .read(true)
        .custom_flags(OPEN_FLAGS)
        .open(&lock_path)?;
    let md = file.metadata()?;
    if !md.is_file()
        || md.uid() != directory.uid()
        || md.nlink() != 1
        || md.len() != READY.len() as u64
    {
        return Err(io::Error::other(
            "lock is not an initialized private regular file",
        ));
    }
    let lock = ExclusiveLock::take(file)?;
    let mut contents = Vec::new();
    etude_core::scan::observe_read(
        "staging_recovery_metadata_and_lock_marker",
        lock.file()
            .take(READY.len() as u64 + 1)
            .read_to_end(&mut contents),
    )?;
    if contents != READY {
        return Err(io::Error::other("lock initialization was not verified"));
    }
    // Another cleaner may have opened the same lock before we did, removed
    // the tree and released it. Never use that now-unlinked descriptor as
    // permission to remove whatever subsequently occupies the pathname.
    if !same(&directory, &owned_directory(path)?)
        || !same(&md, &lock_path.symlink_metadata()?)
        || lock.file().metadata()?.nlink() != 1
    {
        return Err(io::Error::other("staging or lock identity changed"));
    }
    std::fs::remove_dir_all(path)?;
    match path.symlink_metadata() {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
        Ok(_) => Err(io::Error::other("path still exists after cleanup")),
    }
}

pub fn recover(dest: &Path) {
    let parent = dest
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut prefix = vec![b'.'];
    prefix.extend_from_slice(dest.file_name().unwrap_or_default().as_bytes());
    prefix.extend_from_slice(b".unpack-");
    let entries = match etude_core::scan::observe_read(
        "staging_recovery_metadata_and_lock_marker",
        std::fs::read_dir(parent),
    ) {
        Ok(entries) => entries,
        Err(error) => {
            eprintln!(
                "unpack: could not inspect staging in {} ({error}); continuing extraction",
                parent.display()
            );
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                eprintln!(
                    "unpack: could not inspect a staging entry in {} ({error}); continuing extraction",
                    parent.display()
                );
                continue;
            }
        };
        let name = entry.file_name();
        let Some(tail) = name.as_bytes().strip_prefix(prefix.as_slice()) else {
            continue;
        };
        let exact = tail.strip_suffix(b".partial").is_some_and(|id| {
            id.len() == 32
                && id
                    .iter()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
        });
        let path = entry.path();
        let result = if exact {
            reclaim(&path)
        } else {
            Err(io::Error::other("name is not the exact staging pattern"))
        };
        match result {
            Ok(()) => eprintln!(
                "unpack: removed abandoned staging {}",
                path.to_string_lossy().escape_debug()
            ),
            Err(error) => eprintln!(
                "unpack: left staging candidate {} ({error}); continuing extraction",
                path.to_string_lossy().escape_debug()
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_initialized_and_unlocked_staging_can_be_reclaimed() {
        let dest = std::env::temp_dir().join("unpack-lock-initialization");
        let path = super::super::staging_destination(&dest).unwrap();
        let lock_path = lock_path(&path).unwrap();
        let file = File::options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&lock_path)
            .unwrap();
        // A concurrent initializer can have created the file but not yet
        // taken its lock. An empty lock file must never authorize removal.
        assert!(reclaim(&path).is_err());
        assert!(path.is_dir());
        let lock = ExclusiveLock::take(file).unwrap();
        lock.file().write_all(READY).unwrap();
        assert!(reclaim(&path).is_err());
        assert!(path.is_dir());
        drop(lock);
        reclaim(&path).unwrap();
        assert!(!path.exists());
    }
}
