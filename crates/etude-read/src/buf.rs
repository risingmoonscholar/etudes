//! A buffer for file contents that is locked against swap where possible and
//! always erased when dropped.
//!
//! Memory handling has honest limits: the `mlock` guarantee is best-effort and
//! usually fails above ~64 KiB on macOS. [`LockedBuf::locked`] reports which
//! case applies so nothing overclaims.

use std::io::{self, Read};
use std::sync::atomic::{Ordering, compiler_fence};

/// Hard ceiling on how much of any file is read.
pub const MAX_READ: usize = 1024 * 1024;

/// Bytes examined when deciding whether a file is really text.
pub const SNIFF: usize = 8192;

pub struct LockedBuf {
    data: Vec<u8>,
    locked: bool,
}

impl LockedBuf {
    /// Read at most `MAX_READ` bytes from `r`, attempting to lock the pages.
    ///
    /// The buffer is allocated at full capacity up front so it is never
    /// reallocated mid-read: a realloc would copy the contents to a fresh
    /// allocation and leave the old one un-zeroed and unlocked.
    pub fn read_capped(r: &mut dyn Read) -> io::Result<Self> {
        let mut data = Vec::with_capacity(MAX_READ);
        let mut buf = Self {
            data,
            locked: false,
        };
        buf.lock_pages();

        // Re-borrow after locking so the pointer we locked is the one we fill.
        data = std::mem::take(&mut buf.data);
        let mut limited = r.take(MAX_READ as u64);
        let res = limited.read_to_end(&mut data);
        buf.data = data;
        res?;
        Ok(buf)
    }

    /// Whether the pages are actually locked against swap on this machine.
    ///
    /// False is normal: `RLIMIT_MEMLOCK` is small on macOS. Callers must report
    /// this rather than assume the strong guarantee.
    pub fn locked(&self) -> bool {
        self.locked
    }

    pub fn bytes(&self) -> &[u8] {
        &self.data
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    fn lock_pages(&mut self) {
        #[cfg(unix)]
        {
            let cap = self.data.capacity();
            if cap == 0 {
                return;
            }
            // SAFETY: the pointer and length describe this Vec's own allocation.
            let rc = unsafe { mlock(self.data.as_ptr() as *const core::ffi::c_void, cap) };
            self.locked = rc == 0;
        }
    }
}

impl LockedBuf {
    /// Zero the contents and release the lock.
    ///
    /// This is a method rather than the body of `drop` so a test can call it on
    /// a buffer that is still alive. The test it replaced proved the erase by
    /// reading the allocation *after* the drop, which is undefined behaviour
    /// and segfaulted on Linux: `read_capped` reserves `MAX_READ` (1 MiB),
    /// glibc serves anything past its 128 KiB mmap threshold with `mmap`, and
    /// freeing an `mmap`ed block `munmap`s it. So the pages were gone. macOS
    /// keeps the region mapped, so the same read quietly succeeded there and
    /// the suite was green on the machine it was written on.
    ///
    /// Idempotent: calling it and then letting `drop` run again is harmless.
    fn erase_and_unlock(&mut self) {
        // Overwrite through a volatile write so the compiler cannot elide it as
        // a dead store to memory that is about to be freed.
        let len = self.data.len();
        let ptr = self.data.as_mut_ptr();
        for i in 0..len {
            // SAFETY: i < len, so the offset is inside the allocation.
            unsafe { std::ptr::write_volatile(ptr.add(i), 0u8) };
        }
        compiler_fence(Ordering::SeqCst);

        #[cfg(unix)]
        if self.locked {
            let cap = self.data.capacity();
            // SAFETY: same region that was locked in lock_pages.
            unsafe { munlock(self.data.as_ptr() as *const core::ffi::c_void, cap) };
            // Do not unlock twice if this runs again from `drop`.
            self.locked = false;
        }
    }
}

impl Drop for LockedBuf {
    fn drop(&mut self) {
        self.erase_and_unlock();
    }
}

#[cfg(unix)]
unsafe extern "C" {
    fn mlock(addr: *const core::ffi::c_void, len: usize) -> i32;
    fn munlock(addr: *const core::ffi::c_void, len: usize) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_are_capped_so_a_huge_file_cannot_exhaust_memory() {
        let big = vec![b'a'; MAX_READ * 3];
        let buf = LockedBuf::read_capped(&mut big.as_slice()).expect("read");
        assert_eq!(buf.len(), MAX_READ, "read cap was not enforced");
    }

    #[test]
    fn contents_are_erased_before_the_buffer_is_released() {
        // The previous version of this test read the allocation after the drop
        // to prove the erase. That is undefined behaviour, and on Linux it is
        // fatal: the 1 MiB reservation is mmap'd, freeing it munmaps it, and
        // the read segfaults. It passed on macOS for three months because that
        // allocator keeps the region mapped.
        //
        // The property is the same and the observation is legal: run the erase
        // the way `drop` runs it, on a buffer that is still alive, then look.
        let secret = b"123-45-6789 SOCIAL SECURITY";
        let mut buf = LockedBuf::read_capped(&mut secret.as_slice()).expect("read");
        assert_eq!(buf.bytes(), secret, "test setup: contents did not land");

        buf.erase_and_unlock();

        assert!(buf.bytes().iter().all(|b| *b == 0), "buffer was not erased");
        assert!(
            !buf.locked(),
            "the lock was not released, so a second release could double-unlock"
        );
    }

    #[test]
    fn lock_status_is_reported_rather_than_assumed() {
        // Must not panic or refuse when mlock fails, which is the common case
        // on macOS above RLIMIT_MEMLOCK.
        let buf = LockedBuf::read_capped(&mut b"hello".as_slice()).expect("read");
        let _ = buf.locked(); // either value is valid; the point is it is knowable
    }
}
