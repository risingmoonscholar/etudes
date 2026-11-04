//! Nonblocking advisory ownership of an open file, including a child writer.
use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::Command;

pub struct ExclusiveLock(File);

impl ExclusiveLock {
    pub fn take(file: File) -> io::Result<Self> {
        unsafe extern "C" {
            fn flock(fd: i32, operation: i32) -> i32;
        }
        // flock(2): LOCK_EX | LOCK_NB. The file stays open for this guard's
        // lifetime. Drop closes it; explicitly unlocking would also unlock
        // the inherited reference held by a surviving child.
        if unsafe { flock(file.as_raw_fd(), 2 | 4) } == 0 {
            Ok(Self(file))
        } else {
            Err(io::Error::last_os_error())
        }
    }

    pub fn file(&self) -> &File {
        &self.0
    }

    /// Keep the lock until both the supervisor and its writer have exited.
    /// The guard must remain alive until the command has spawned.
    pub fn inherit(&self, command: &mut Command) {
        let fd = self.0.as_raw_fd();
        // close(2), fcntl(2): F_SETFD = 2, zero clears close-on-exec.
        // SAFETY: the pre-exec closure uses only an integer and the
        // async-signal-safe fcntl call. The parent keeps close-on-exec set.
        unsafe {
            command.pre_exec(move || {
                unsafe extern "C" {
                    fn fcntl(fd: i32, cmd: i32, ...) -> i32;
                }
                if fcntl(fd, 2, 0) == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
}
