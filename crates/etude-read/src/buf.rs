//! A buffer for file contents that is locked against swap where possible and
//! erased when dropped, including the spare capacity past its length.
//!
//! Memory handling has honest limits, and they are stated rather than implied.
//! The `mlock` guarantee is best-effort and usually fails above ~64 KiB on
//! macOS; [`LockedBuf::locked`] reports which case applies.
//!
//! One path is NOT covered. [`LockedBuf::read_capped`] moves the allocation
//! out of the struct while filling it, so the locked pointer is the one
//! written to. If the reader panics mid-read, that local allocation is freed
//! directly and the erase in `Drop` runs against the empty replacement. Bytes
//! a panicking reader had already written are not erased. So the claim here is
//! "erased when dropped", not "always erased" -- the difference is a panic,
//! and it is not yet closed.

use std::io::{self, Read};
use std::sync::atomic::{Ordering, compiler_fence};

/// Hard ceiling on how much of any file is read.
pub const MAX_READ: usize = 1024 * 1024;

/// Bytes examined when deciding whether a file is really text.
pub const SNIFF: usize = 8192;

#[cfg(test)]
const WITNESS_WAITING: u8 = 0;
#[cfg(test)]
const WITNESS_ERASED: u8 = 1;
#[cfg(test)]
const WITNESS_NOT_ERASED: u8 = 2;

pub struct LockedBuf {
    data: Vec<u8>,
    locked: bool,
    #[cfg(test)]
    drop_witness: Option<std::sync::Arc<std::sync::atomic::AtomicU8>>,
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
            #[cfg(test)]
            drop_witness: None,
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
        // The whole allocation, not the logical length. mlock and munlock
        // already treat capacity as one region; erasing only len left bytes
        // above it untouched, and a safe Read may write into the spare
        // capacity and then report a shorter count, stranding real content
        // exactly there.
        let cap = self.data.capacity();
        let ptr = self.data.as_mut_ptr();
        for i in 0..cap {
            // SAFETY: i < cap, so the offset is inside the allocation. The
            // memory past len is allocated and owned; writing zeroes to it is
            // sound and is the point.
            unsafe { std::ptr::write_volatile(ptr.add(i), 0u8) };
        }
        compiler_fence(Ordering::SeqCst);

        #[cfg(test)]
        if let Some(witness) = &self.drop_witness {
            // This happens while `self.data` is still allocated. The witness
            // can therefore observe the zeroes without reading freed memory.
            let state = if self.data.iter().all(|byte| *byte == 0) {
                WITNESS_ERASED
            } else {
                WITNESS_NOT_ERASED
            };
            witness.store(state, Ordering::SeqCst);
        }

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
    use std::sync::Arc;

    impl LockedBuf {
        fn witness_drop_with(&mut self, witness: Arc<std::sync::atomic::AtomicU8>) {
            self.drop_witness = Some(witness);
        }
    }

    #[test]
    fn reads_are_capped_so_a_huge_file_cannot_exhaust_memory() {
        let big = vec![b'a'; MAX_READ * 3];
        let buf = LockedBuf::read_capped(&mut big.as_slice()).expect("read");
        assert_eq!(buf.len(), MAX_READ, "read cap was not enforced");
    }

    #[test]
    fn dropping_the_buffer_erases_contents_with_a_live_witness() {
        // The previous test inspected the allocation after Drop. That is
        // undefined behaviour and segfaults on Linux when the 1 MiB Vec is
        // backed by mmap and freeing it unmaps the pages.
        //
        // Instead, the test-only witness is called from erase_and_unlock while
        // the Vec still owns its allocation. It records whether the bytes are
        // zero, then this scope invokes the real Drop implementation.
        // Controlled fault: removing `self.erase_and_unlock()` from Drop leaves
        // this witness waiting, so the assertion below fails.
        let secret = b"123-45-6789 SOCIAL SECURITY";
        let witness = Arc::new(std::sync::atomic::AtomicU8::new(WITNESS_WAITING));
        {
            let mut buf = LockedBuf::read_capped(&mut secret.as_slice()).expect("read");
            assert_eq!(buf.bytes(), secret, "test setup: contents did not land");
            buf.witness_drop_with(Arc::clone(&witness));
        }

        assert!(
            witness.load(Ordering::SeqCst) == WITNESS_ERASED,
            "dropping the buffer did not erase its contents before release"
        );
    }

    #[test]
    fn lock_status_is_reported_rather_than_assumed() {
        // Must not panic or refuse when mlock fails, which is the common case
        // on macOS above RLIMIT_MEMLOCK.
        let buf = LockedBuf::read_capped(&mut b"hello".as_slice()).expect("read");
        let _ = buf.locked(); // either value is valid; the point is it is knowable
    }

    /// Bytes can live above `len` and below `capacity`, and nothing else
    /// scrubs them: `Vec` does not, and an erase loop bounded by `len` never
    /// reaches them. A safe `Read` puts them there by writing into the slice
    /// it is handed and then reporting a smaller count.
    ///
    /// This constructs that state DIRECTLY rather than trying to provoke it
    /// through `read_to_end`. Whether `read_to_end` hands a reader the Vec's
    /// own spare capacity is an implementation detail that differs by
    /// platform -- an earlier version of this test relied on it, passed here,
    /// and failed its own precondition on CI because no bytes were stranded
    /// there at all. The property under test is that the erase covers the
    /// whole allocation; the route the bytes took to get there is not.
    ///
    /// Controlled fault: bound the erase loop by `len` instead of `capacity`
    /// and this goes red.
    #[test]
    fn bytes_above_the_length_are_erased_too() {
        let mut data: Vec<u8> = Vec::with_capacity(64);
        data.push(0x01);

        // Write a recognisable pattern into the spare capacity, then leave
        // len at 1. SAFETY: the Vec owns `capacity` bytes; writing to
        // allocated-but-uninitialised memory it owns is sound, and len is
        // left untouched so the bytes are genuinely above it.
        let cap = data.capacity();
        unsafe {
            let ptr = data.as_mut_ptr();
            for i in 1..cap {
                std::ptr::write_volatile(ptr.add(i), 0xAB);
            }
        }
        assert_eq!(data.len(), 1, "the bytes are above the reported length");

        let mut buf = LockedBuf {
            data,
            locked: false,
            drop_witness: None,
        };
        let ptr = buf.data.as_ptr();

        // SAFETY: the Vec owns this allocation and is alive for the assertion.
        assert!(
            unsafe { std::slice::from_raw_parts(ptr, cap) }[1..].contains(&0xAB),
            "precondition: the bytes really are sitting above len"
        );

        buf.erase_and_unlock();

        // SAFETY: same allocation, still owned, still alive. Nothing is read
        // after free -- the buffer outlives every assertion here.
        assert!(
            unsafe { std::slice::from_raw_parts(ptr, cap) }
                .iter()
                .all(|&b| b != 0xAB),
            "bytes above the reported length survived erase_and_unlock"
        );
    }
}
