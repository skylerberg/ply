use std::ptr;

unsafe extern "C" {
    fn mmap(address: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, offset: i64) -> *mut u8;
    fn munmap(address: *mut u8, len: usize) -> i32;
    fn mprotect(address: *mut u8, len: usize, prot: i32) -> i32;
    fn madvise(address: *mut u8, len: usize, advice: i32) -> i32;
}

const PROT_NONE: i32 = 0;
const PROT_READ: i32 = 1;
const PROT_WRITE: i32 = 2;
const MAP_PRIVATE: i32 = 0x02;
#[cfg(target_os = "macos")]
const MAP_ANONYMOUS: i32 = 0x1000;
#[cfg(target_os = "linux")]
const MAP_ANONYMOUS: i32 = 0x20;
#[cfg(target_os = "macos")]
const MAP_NORESERVE: i32 = 0;
#[cfg(target_os = "linux")]
const MAP_NORESERVE: i32 = 0x4000;
#[cfg(target_os = "macos")]
const MADV_FREE: i32 = 5;
#[cfg(target_os = "linux")]
const MADV_FREE: i32 = 4;
#[cfg(target_os = "linux")]
const MADV_NOHUGEPAGE: i32 = 15;

fn mapped(p: *mut u8) -> *mut u8 {
    if p as isize == -1 { ptr::null_mut() } else { p }
}

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
    #[cfg(target_os = "linux")]
    if !p.is_null() {
        unsafe { madvise(p, len, MADV_NOHUGEPAGE) };
    }
    p
}

pub unsafe fn commit(address: *mut u8, len: usize) -> bool {
    unsafe { mprotect(address, len, PROT_READ | PROT_WRITE) == 0 }
}

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

pub unsafe fn unmap(address: *mut u8, len: usize) {
    unsafe { munmap(address, len) };
}

pub unsafe fn let_go(address: *mut u8, len: usize) {
    unsafe { madvise(address, len, MADV_FREE) };
}
