//! Memory straight from the operating system: mapped or reserved, made writable, given back, and
//! let go of while it stays mapped.

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
/// macOS keeps no commit charge, so a reservation needs no flag to escape one.
#[cfg(target_os = "macos")]
const MAP_NORESERVE: i32 = 0;
#[cfg(target_os = "linux")]
const MAP_NORESERVE: i32 = 0x4000;
/// The advice after which the memory reads as zeros or as it was, and is no longer the process's.
#[cfg(target_os = "macos")]
const MADV_LET_GO: i32 = 5;
#[cfg(target_os = "linux")]
const MADV_LET_GO: i32 = 4;
#[cfg(target_os = "linux")]
const MADV_NOHUGEPAGE: i32 = 15;

fn mapped(p: *mut u8) -> *mut u8 {
    if p as isize == -1 { ptr::null_mut() } else { p }
}

/// `len` bytes of zeros, readable and writable and touched by nothing yet; null when the system
/// maps no more.
pub unsafe fn map(len: usize) -> *mut u8 {
    mapped(unsafe {
        mmap(
            ptr::null_mut(),
            len,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE | MAP_ANONYMOUS,
            -1,
            0,
        )
    })
}

/// `len` bytes of address space every touch of which faults until [`commit`] makes a part of it
/// writable, charged for nothing until then; null when the system maps no more.
pub unsafe fn reserve(len: usize) -> *mut u8 {
    let p = mapped(unsafe {
        mmap(
            ptr::null_mut(),
            len,
            PROT_NONE,
            MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE,
            -1,
            0,
        )
    });
    // A huge page would make the first touch of a stack's top cost 2 MB resident.
    #[cfg(target_os = "linux")]
    if !p.is_null() {
        unsafe { madvise(p, len, MADV_NOHUGEPAGE) };
    }
    p
}

/// Makes the `len` bytes at `addr`, within what [`reserve`] returned and a multiple of the page
/// size, readable and writable: zeros, backed as they are touched. `false` when the system will
/// commit no more.
pub unsafe fn commit(addr: *mut u8, len: usize) -> bool {
    unsafe { mprotect(addr, len, PROT_READ | PROT_WRITE) == 0 }
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

/// Gives back `len` bytes [`map`] or [`reserve`] returned at `addr`.
pub unsafe fn unmap(addr: *mut u8, len: usize) {
    unsafe { munmap(addr, len) };
}

/// Lets the system take back the `len` bytes at `addr`, a multiple of its page size, which stay
/// mapped: what they held is gone, and a touch maps them afresh.
pub unsafe fn let_go(addr: *mut u8, len: usize) {
    unsafe { madvise(addr, len, MADV_LET_GO) };
}
