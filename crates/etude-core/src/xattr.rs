//! Extended attributes: the one binding to Darwin's `getxattr`, `setxattr`
//! and `removexattr`, and the one place their error numbers are read.
//!
//! Every call acts on the directory entry itself and never follows a symlink
//! (`XATTR_NOFOLLOW`). An absent attribute is not an error: `presence` says
//! `false`, `read` and `read_into` say `None`, and `remove` succeeds. Every
//! other failure is returned as the operating system reported it, so a caller
//! can still tell one cause from another.
//!
//! A volume that cannot hold attributes at all answers `ENOTSUP`. That is an
//! error here, because what it means depends on the question:
//! "has this file a tag?" is answered "no" (see `is_unsupported`), while
//! "what are this file's tags?" has no answer and must stay an error.
//!
//! Elsewhere than macOS every call fails with `io::ErrorKind::Unsupported`
//! and touches nothing. A caller that must not even try checks `SUPPORTED`.

use std::ffi::CStr;
use std::fmt;
use std::io;
use std::path::Path;

/// Whether this build can read and write extended attributes.
pub const SUPPORTED: bool = cfg!(target_os = "macos");

/// Whether the attribute `name` exists on the entry at `path`.
///
/// Asks for the value's length through a null, zero-length buffer, so no
/// part of the value is ever copied into this process.
pub fn presence(path: &Path, name: &CStr) -> io::Result<bool> {
    platform::length(path, name).map(|length| length.is_some())
}

/// Read the attribute into `buffer` with exactly one call. Returns the
/// number of bytes read, or `None` when the attribute is absent. A value
/// longer than `buffer` is an error (`ERANGE`), never a truncated read.
pub fn read_into(path: &Path, name: &CStr, buffer: &mut [u8]) -> io::Result<Option<usize>> {
    platform::read_into(path, name, buffer)
}

/// Read the whole attribute, or `None` when it is absent.
///
/// A value longer than `max` bytes is refused before it is copied
/// (`Refusal::TooLarge`). A value that keeps growing between the length
/// query and the read is tried three times, then refused
/// (`Refusal::KeptChanging`).
pub fn read(path: &Path, name: &CStr, max: usize) -> io::Result<Option<Vec<u8>>> {
    for _ in 0..3 {
        let Some(length) = platform::length(path, name)? else {
            return Ok(None);
        };
        if length > max {
            return Err(io::Error::other(Refusal::TooLarge));
        }
        let mut buffer = vec![0u8; length];
        match platform::read_into(path, name, &mut buffer) {
            Ok(Some(got)) => {
                buffer.truncate(got);
                return Ok(Some(buffer));
            }
            Ok(None) => return Ok(None),
            // The value grew after its length was read: ask again.
            Err(error) if platform::is_too_small(&error) => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::other(Refusal::KeptChanging))
}

/// Replace the attribute with exactly these bytes, creating it if needed.
pub fn write(path: &Path, name: &CStr, value: &[u8]) -> io::Result<()> {
    platform::write(path, name, value)
}

/// Remove the attribute. An attribute that is already absent is removed.
pub fn remove(path: &Path, name: &CStr) -> io::Result<()> {
    platform::remove(path, name)
}

/// Whether `error` means the volume cannot hold extended attributes
/// (`ENOTSUP`), or, elsewhere than macOS, that this build has no support.
pub fn is_unsupported(error: &io::Error) -> bool {
    platform::is_unsupported(error)
}

/// Why `read` declined a value that exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The value is longer than the caller's limit.
    TooLarge,
    /// The value kept changing size while it was read.
    KeptChanging,
}

impl Refusal {
    /// The refusal carried by `error`, if `read` produced it.
    pub fn of(error: &io::Error) -> Option<Refusal> {
        error.get_ref()?.downcast_ref::<Refusal>().copied()
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Refusal::TooLarge => "attribute is too large to handle",
            Refusal::KeptChanging => "attribute kept changing while it was read",
        })
    }
}

impl std::error::Error for Refusal {}

#[cfg(target_os = "macos")]
mod platform {
    use std::ffi::{CStr, CString};
    use std::io;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    /// Darwin `XATTR_NOFOLLOW`: act on the directory entry, never a link target.
    const XATTR_NOFOLLOW: i32 = 0x0001;
    /// Darwin `ENOATTR`: the attribute is absent.
    const ENOATTR: i32 = 93;
    /// Darwin `ENOTSUP`: the volume cannot hold extended attributes.
    const ENOTSUP: i32 = 45;
    /// Darwin `ERANGE`: the value is longer than the buffer.
    const ERANGE: i32 = 34;

    // Signatures from the macOS SDK's sys/xattr.h.
    unsafe extern "C" {
        fn getxattr(
            path: *const std::ffi::c_char,
            name: *const std::ffi::c_char,
            value: *mut std::ffi::c_void,
            size: usize,
            position: u32,
            options: i32,
        ) -> isize;
        fn setxattr(
            path: *const std::ffi::c_char,
            name: *const std::ffi::c_char,
            value: *const std::ffi::c_void,
            size: usize,
            position: u32,
            options: i32,
        ) -> i32;
        fn removexattr(
            path: *const std::ffi::c_char,
            name: *const std::ffi::c_char,
            options: i32,
        ) -> i32;
    }

    /// The one Darwin error mapping: what a failed call's `errno` means.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) enum Failure {
        /// `ENOATTR`: the entry has no such attribute.
        Absent,
        /// `ENOTSUP`: the volume cannot hold attributes.
        Unsupported,
        /// `ERANGE`: the buffer is shorter than the value.
        TooSmall,
        /// Anything else: returned to the caller as it is.
        Other,
    }

    pub(super) fn failure(error: &io::Error) -> Failure {
        match error.raw_os_error() {
            Some(ENOATTR) => Failure::Absent,
            Some(ENOTSUP) => Failure::Unsupported,
            Some(ERANGE) => Failure::TooSmall,
            _ => Failure::Other,
        }
    }

    pub fn is_unsupported(error: &io::Error) -> bool {
        failure(error) == Failure::Unsupported
    }

    pub fn is_too_small(error: &io::Error) -> bool {
        failure(error) == Failure::TooSmall
    }

    fn c_path(path: &Path) -> io::Result<CString> {
        CString::new(path.as_os_str().as_bytes())
            .map_err(|_| io::Error::other("path contains a NUL byte"))
    }

    /// The last call's error, or `Ok` when it only says "absent".
    fn absent_or_error() -> io::Result<()> {
        let error = io::Error::last_os_error();
        match failure(&error) {
            Failure::Absent => Ok(()),
            _ => Err(error),
        }
    }

    pub fn length(path: &Path, name: &CStr) -> io::Result<Option<usize>> {
        let path = c_path(path)?;
        // SAFETY: both strings are NUL-terminated; the value buffer is null
        // with length zero, so getxattr reports the length and cannot write
        // Rust memory.
        let result = unsafe {
            getxattr(
                path.as_ptr(),
                name.as_ptr(),
                std::ptr::null_mut(),
                0,
                0,
                XATTR_NOFOLLOW,
            )
        };
        if result >= 0 {
            Ok(Some(result as usize))
        } else {
            absent_or_error().map(|()| None)
        }
    }

    pub fn read_into(path: &Path, name: &CStr, buffer: &mut [u8]) -> io::Result<Option<usize>> {
        let path = c_path(path)?;
        // SAFETY: valid C strings and a buffer that owns `buffer.len()`
        // writable bytes.
        let result = unsafe {
            getxattr(
                path.as_ptr(),
                name.as_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                0,
                XATTR_NOFOLLOW,
            )
        };
        if result >= 0 {
            Ok(Some(result as usize))
        } else {
            absent_or_error().map(|()| None)
        }
    }

    pub fn write(path: &Path, name: &CStr, value: &[u8]) -> io::Result<()> {
        let path = c_path(path)?;
        // SAFETY: valid C strings and a slice that outlives the call.
        let result = unsafe {
            setxattr(
                path.as_ptr(),
                name.as_ptr(),
                value.as_ptr().cast(),
                value.len(),
                0,
                XATTR_NOFOLLOW,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    pub fn remove(path: &Path, name: &CStr) -> io::Result<()> {
        let path = c_path(path)?;
        // SAFETY: valid NUL-terminated strings.
        let result = unsafe { removexattr(path.as_ptr(), name.as_ptr(), XATTR_NOFOLLOW) };
        if result == 0 {
            Ok(())
        } else {
            absent_or_error()
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use std::ffi::CStr;
    use std::io;
    use std::path::Path;

    fn unsupported() -> io::Error {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "extended attributes are a macOS feature",
        )
    }

    pub fn is_unsupported(error: &io::Error) -> bool {
        error.kind() == io::ErrorKind::Unsupported
    }

    pub fn is_too_small(_: &io::Error) -> bool {
        false
    }

    pub fn length(_: &Path, _: &CStr) -> io::Result<Option<usize>> {
        Err(unsupported())
    }

    pub fn read_into(_: &Path, _: &CStr, _: &mut [u8]) -> io::Result<Option<usize>> {
        Err(unsupported())
    }

    pub fn write(_: &Path, _: &CStr, _: &[u8]) -> io::Result<()> {
        Err(unsupported())
    }

    pub fn remove(_: &Path, _: &CStr) -> io::Result<()> {
        Err(unsupported())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    const NAME: &CStr = c"org.etudes.test:xattr";

    /// A private folder under the temp folder, removed on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(label: &str) -> Scratch {
            let dir = std::env::temp_dir().join(format!(
                "etude-xattr-{label}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }

        fn file(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            fs::write(&path, b"contents").unwrap();
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn presence_follows_write_and_remove() {
        let scratch = Scratch::new("presence");
        let file = scratch.file("a.txt");
        assert!(
            !presence(&file, NAME).unwrap(),
            "a new file has no attribute"
        );
        write(&file, NAME, b"value").unwrap();
        assert!(presence(&file, NAME).unwrap());
        // An empty value still exists.
        write(&file, NAME, b"").unwrap();
        assert!(presence(&file, NAME).unwrap());
        remove(&file, NAME).unwrap();
        assert!(!presence(&file, NAME).unwrap());
        remove(&file, NAME).expect("removing an absent attribute succeeds");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn read_returns_the_value_up_to_the_limit_and_refuses_beyond_it() {
        let scratch = Scratch::new("read");
        let file = scratch.file("a.txt");
        assert_eq!(read(&file, NAME, 16).unwrap(), None);
        write(&file, NAME, b"0123456789").unwrap();
        assert_eq!(
            read(&file, NAME, 10).unwrap().as_deref(),
            Some(&b"0123456789"[..])
        );
        let error = read(&file, NAME, 9).unwrap_err();
        assert_eq!(Refusal::of(&error), Some(Refusal::TooLarge));
        assert_eq!(error.raw_os_error(), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn read_into_makes_one_exact_read() {
        let scratch = Scratch::new("read-into");
        let file = scratch.file("a.txt");
        let mut buffer = [0u8; 4];
        assert_eq!(read_into(&file, NAME, &mut buffer).unwrap(), None);
        write(&file, NAME, b"abc").unwrap();
        assert_eq!(read_into(&file, NAME, &mut buffer).unwrap(), Some(3));
        assert_eq!(&buffer[..3], b"abc");
        write(&file, NAME, b"abcde").unwrap();
        let error = read_into(&file, NAME, &mut buffer).unwrap_err();
        assert_eq!(platform::failure(&error), platform::Failure::TooSmall);
    }

    /// Every call acts on the link itself: a tagged target does not make
    /// its link look tagged.
    #[cfg(target_os = "macos")]
    #[test]
    fn calls_never_follow_a_symlink() {
        let scratch = Scratch::new("nofollow");
        let target = scratch.file("target.txt");
        let link = scratch.0.join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        write(&target, NAME, b"on the target").unwrap();
        assert!(!presence(&link, NAME).unwrap());
        assert_eq!(read(&link, NAME, 64).unwrap(), None);
        remove(&link, NAME).unwrap();
        assert!(presence(&target, NAME).unwrap(), "the target keeps it");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn darwin_errors_map_to_one_meaning_each() {
        use platform::{Failure, failure};
        let errno = io::Error::from_raw_os_error;
        assert_eq!(failure(&errno(93)), Failure::Absent);
        assert_eq!(failure(&errno(45)), Failure::Unsupported);
        assert_eq!(failure(&errno(34)), Failure::TooSmall);
        assert_eq!(failure(&errno(1)), Failure::Other);
        assert_eq!(failure(&io::Error::other("no errno")), Failure::Other);
        assert!(is_unsupported(&errno(45)));
        assert!(!is_unsupported(&errno(93)));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_path_with_a_nul_byte_is_an_error_not_absence() {
        use std::os::unix::ffi::OsStrExt;
        let path = Path::new(std::ffi::OsStr::from_bytes(b"a\0b"));
        assert!(presence(path, NAME).is_err());
        assert!(read(path, NAME, 8).is_err());
        assert!(remove(path, NAME).is_err());
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn every_call_is_unsupported_elsewhere() {
        let scratch = Scratch::new("stub");
        let file = scratch.file("a.txt");
        for error in [
            presence(&file, NAME).unwrap_err(),
            read(&file, NAME, 8).unwrap_err(),
            read_into(&file, NAME, &mut [0u8; 8]).unwrap_err(),
            write(&file, NAME, b"x").unwrap_err(),
            remove(&file, NAME).unwrap_err(),
        ] {
            assert!(is_unsupported(&error));
        }
    }

    #[test]
    fn a_refusal_is_recognised_only_in_its_own_error() {
        let error = io::Error::other(Refusal::KeptChanging);
        assert_eq!(Refusal::of(&error), Some(Refusal::KeptChanging));
        assert_eq!(Refusal::of(&io::Error::other("other")), None);
        assert_eq!(Refusal::of(&io::Error::from_raw_os_error(34)), None);
    }
}
