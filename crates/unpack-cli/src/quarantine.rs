//! Preserve the captured archive quarantine before publishing any extracted path.
//! Sources: Apple getxattr(2) and fsetxattr(2); values remain opaque and private.
use std::fs::File;
use std::io;
use std::path::Path;

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    const NAME: &[u8] = b"com.apple.quarantine\0";
    const ENOATTR: i32 = 93;
    const O_NOFOLLOW: i32 = 0x100;
    unsafe extern "C" {
        fn fgetxattr(
            fd: i32,
            name: *const core::ffi::c_char,
            value: *mut core::ffi::c_void,
            size: usize,
            position: u32,
            options: i32,
        ) -> isize;
        fn fsetxattr(
            fd: i32,
            name: *const core::ffi::c_char,
            value: *const core::ffi::c_void,
            size: usize,
            position: u32,
            options: i32,
        ) -> i32;
    }
    pub fn read(file: &File) -> io::Result<Option<Vec<u8>>> {
        // SAFETY: the descriptor and NUL-terminated name stay valid through the call.
        let size = unsafe {
            fgetxattr(
                file.as_raw_fd(),
                NAME.as_ptr().cast(),
                std::ptr::null_mut(),
                0,
                0,
                0,
            )
        };
        if size < 0 {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(ENOATTR) {
                Ok(None)
            } else {
                Err(error)
            };
        }
        let mut bytes = vec![0; size as usize];
        // SAFETY: the buffer holds size writable bytes and both descriptor and name are valid.
        let got = unsafe {
            fgetxattr(
                file.as_raw_fd(),
                NAME.as_ptr().cast(),
                bytes.as_mut_ptr().cast(),
                bytes.len(),
                0,
                0,
            )
        };
        if got < 0 {
            return Err(io::Error::last_os_error());
        }
        bytes.truncate(got as usize);
        Ok(Some(bytes))
    }
    pub fn open(path: &Path) -> io::Result<File> {
        File::options()
            .read(true)
            .custom_flags(O_NOFOLLOW)
            .open(path)
    }
    pub fn apply(path: &Path, expected: &[u8]) -> io::Result<()> {
        let file = open(path)?;
        // SAFETY: the borrowed byte buffer, descriptor and NUL-terminated name are valid.
        let rc = unsafe {
            fsetxattr(
                file.as_raw_fd(),
                NAME.as_ptr().cast(),
                expected.as_ptr().cast(),
                expected.len(),
                0,
                0,
            )
        };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        verify(&file, expected)
    }
    pub fn verify(file: &File, expected: &[u8]) -> io::Result<()> {
        let actual = etude_core::scan::observe_read("staging_quarantine_metadata", read(file))?;
        if actual.as_deref() != Some(expected) {
            return Err(io::Error::other(
                "quarantine readback did not match the captured archive state",
            ));
        }
        etude_core::scan::verify_read("staging_quarantine_metadata");
        Ok(())
    }
}

pub fn open_archive(path: &Path) -> io::Result<File> {
    #[cfg(target_os = "macos")]
    {
        platform::open(path)
    }
    #[cfg(not(target_os = "macos"))]
    {
        File::open(path)
    }
}

pub fn capture(file: &File) -> io::Result<Option<Vec<u8>>> {
    #[cfg(target_os = "macos")]
    {
        etude_core::scan::observe_read("archive_quarantine_metadata", platform::read(file))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = file;
        Ok(None)
    }
}

pub fn preserve(root: &Path, mark: Option<&[u8]>) -> io::Result<usize> {
    let Some(mark) = mark else {
        return Ok(0);
    };
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (root, mark);
        Err(io::Error::other("quarantine storage requires macOS"))
    }
    #[cfg(target_os = "macos")]
    {
        fn collect(path: &Path, paths: &mut Vec<std::path::PathBuf>) -> io::Result<()> {
            let metadata = etude_core::scan::observe_read(
                "staging_tree_metadata",
                std::fs::symlink_metadata(path),
            )?;
            if metadata.file_type().is_symlink() || (!metadata.is_file() && !metadata.is_dir()) {
                return Err(io::Error::other(
                    "quarantine target is not a regular file or directory",
                ));
            }
            if metadata.is_dir() {
                let entries = etude_core::scan::observe_read(
                    "staging_tree_metadata",
                    std::fs::read_dir(path),
                )?;
                for entry in entries {
                    collect(&entry?.path(), paths)?;
                }
            }
            paths.push(path.to_path_buf());
            Ok(())
        }
        // Collect before setting attributes: exFAT creates AppleDouble companions during the writes.
        let mut paths = Vec::new();
        collect(root, &mut paths)?;
        for path in &paths {
            platform::apply(path, mark)?;
        }
        Ok(paths.len())
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    #[test]
    fn unsupported_or_failed_storage_never_counts_as_preserved() {
        assert!(platform::apply(Path::new("/dev/null"), b"synthetic").is_err());
        assert!(preserve(Path::new("/dev/null"), Some(b"synthetic")).is_err());
        assert!(preserve(Path::new("/dev/null"), None).is_ok());
    }
    #[test]
    fn mismatched_readback_is_not_verification() {
        let path =
            std::env::temp_dir().join(format!("etudes-quarantine-readback-{}", std::process::id()));
        std::fs::write(&path, b"synthetic").unwrap();
        platform::apply(&path, b"0083;00000000;etudes-expected").unwrap();
        let file = open_archive(&path).unwrap();
        assert!(platform::verify(&file, b"0083;00000000;etudes-different").is_err());
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn quarantine_walk_refuses_symlinks_without_touching_the_target() {
        let root =
            std::env::temp_dir().join(format!("etudes-quarantine-link-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        std::os::unix::fs::symlink("/dev/null", root.join("link")).unwrap();
        assert!(preserve(&root, Some(b"synthetic")).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(all(test, target_os = "macos"))]
pub fn platform_set_for_test(path: &Path, mark: &[u8]) -> io::Result<()> {
    platform::apply(path, mark)
}
