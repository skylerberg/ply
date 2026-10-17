//! Memory straight from the operating system: mapped, given back, and let go of while it stays
//! mapped.

use std::ptr;

unsafe extern "C" {
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, offset: i64) -> *mut u8;
    fn munmap(addr: *mut u8, len: usize) -> i32;
    fn mprotect(addr: *mut u8, len: usize, prot: i32) -> i32;
    fn madvise(addr: *mut u8, len: usize, advice: i32) -> i32;
}

const PROT_NONE: i32 = 0;
const PROT_READ: i32 = 1;
const PROT_WRITE: i32 = 2;
const MAP_PRIVATE: i32 = 0x02;
#[cfg(target_os = "macos")]
const MAP_ANONYMOUS: i32 = 0x1000;
#[cfg(target_os = "linux")]
const MAP_ANONYMOUS: i32 = 0x20;
/// The advice after which the memory reads as zeros or as it was, and is no longer the process's.
#[cfg(target_os = "macos")]
const MADV_LET_GO: i32 = 5;
#[cfg(target_os = "linux")]
const MADV_LET_GO: i32 = 4;

/// `len` bytes of zeros, readable and writable and touched by nothing yet; null when the system
/// maps no more.
pub unsafe fn map(len: usize) -> *mut u8 {
    let p = unsafe {
        mmap(
            ptr::null_mut(),
            len,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE | MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    if p as isize == -1 { ptr::null_mut() } else { p }
}

/// [`map`] of `len` bytes at a multiple of `len`, a power of two.
pub unsafe fn map_aligned(len: usize) -> *mut u8 {
    unsafe {
        let p = map(2 * len);
        if p.is_null() {
            return p;
        }
        let lead = p.addr().wrapping_neg() & (len - 1);
        if lead > 0 {
            munmap(p, lead);
        }
        let base = p.add(lead);
        if lead < len {
            munmap(base.add(len), len - lead);
        }
        base
    }
}

/// Gives back `len` bytes [`map`] answered at `addr`.
pub unsafe fn unmap(addr: *mut u8, len: usize) {
    unsafe { munmap(addr, len) };
}

/// Makes every touch of the `len` bytes at `addr` a fault.
pub unsafe fn forbid(addr: *mut u8, len: usize) {
    unsafe { mprotect(addr, len, PROT_NONE) };
}

/// Lets the system take back the `len` bytes at `addr`, a multiple of its page size, which stay
/// mapped: what they held is gone, and a touch maps them afresh.
pub unsafe fn let_go(addr: *mut u8, len: usize) {
    unsafe { madvise(addr, len, MADV_LET_GO) };
}
