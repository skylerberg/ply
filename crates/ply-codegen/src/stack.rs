//! Stacks the runtime owns, and the [`switch`] between them: a suspended computation in the
//! compiled tier is a C stack, and nothing emitted knows which stack it is on.

use std::ptr;

/// Reserved per stack, mapped on first touch; a guard page below turns an overrun into a fault.
pub const STACK_SIZE: usize = 8 * 1024 * 1024;

const GUARD: usize = 16 * 1024;

pub struct Stack {
    base: *mut u8,
    size: usize,
}

impl Stack {
    pub fn new() -> Stack {
        Stack::reserve().expect("a task stack could not be reserved")
    }

    /// A stack of its own, or nothing when the platform would map no more.
    pub fn reserve() -> Option<Stack> {
        let size = STACK_SIZE + GUARD;
        let base = unsafe { mmap_anonymous(size) };
        if base.is_null() {
            return None;
        }
        unsafe { mprotect_none(base, GUARD) };
        Some(Stack { base, size })
    }

    /// The lowest address a compiled frame may begin at: the guard, then the Rust frames' margin.
    pub fn floor(&self) -> usize {
        self.base as usize + GUARD + crate::rt::STACK_MARGIN
    }

    pub fn top(&self) -> usize {
        self.base as usize + self.size
    }

    pub fn holds(&self, address: usize) -> bool {
        (self.base as usize..self.top()).contains(&address)
    }

    /// Lays out the frame the first [`switch`] pops, returning into a trampoline that calls
    /// `entry(arg)`, which must never return: there is nothing below it.
    pub fn prepare(&self, entry: extern "C" fn(usize), arg: usize) -> usize {
        let top = self.top() & !15;
        unsafe { lay_out(top, entry as usize, arg) }
    }

    /// The bytes live on this stack when `sp` is its stack pointer: from `sp` to the top. Nothing
    /// when `sp` is not this stack's, as it is for a computation that grew onto another.
    pub fn live(&self, sp: usize) -> Option<&[u8]> {
        if !self.holds(sp) {
            return None;
        }
        Some(unsafe { std::slice::from_raw_parts(sp as *const u8, self.top() - sp) })
    }

    pub unsafe fn restore(&self, sp: usize, bytes: &[u8]) {
        debug_assert_eq!(bytes.len(), self.top() - sp);
        unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), sp as *mut u8, bytes.len()) }
    }
}

impl Drop for Stack {
    fn drop(&mut self) {
        unsafe { munmap(self.base, self.size) };
    }
}

impl Default for Stack {
    fn default() -> Stack {
        Stack::new()
    }
}

#[cfg(target_arch = "aarch64")]
mod arch {
    use std::arch::naked_asm;

    /// x19..x28, x29, x30 and d8..d15, rounded to 16-byte stack alignment.
    pub const FRAME: usize = 176;

    #[unsafe(naked)]
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn ply_stack_switch(from: *mut usize, to: usize) {
        naked_asm!(
            "sub sp, sp, #176",
            "stp x19, x20, [sp, #0]",
            "stp x21, x22, [sp, #16]",
            "stp x23, x24, [sp, #32]",
            "stp x25, x26, [sp, #48]",
            "stp x27, x28, [sp, #64]",
            "stp x29, x30, [sp, #80]",
            "stp d8, d9, [sp, #96]",
            "stp d10, d11, [sp, #112]",
            "stp d12, d13, [sp, #128]",
            "stp d14, d15, [sp, #144]",
            "mov x2, sp",
            "str x2, [x0]",
            "mov sp, x1",
            "ldp x19, x20, [sp, #0]",
            "ldp x21, x22, [sp, #16]",
            "ldp x23, x24, [sp, #32]",
            "ldp x25, x26, [sp, #48]",
            "ldp x27, x28, [sp, #64]",
            "ldp x29, x30, [sp, #80]",
            "ldp d8, d9, [sp, #96]",
            "ldp d10, d11, [sp, #112]",
            "ldp d12, d13, [sp, #128]",
            "ldp d14, d15, [sp, #144]",
            "add sp, sp, #176",
            "ret",
        )
    }

    /// Where the first switch into a fresh stack lands: entry in x20, argument in x19.
    #[unsafe(naked)]
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn ply_stack_trampoline() {
        naked_asm!("mov x0, x19", "blr x20", "brk #0")
    }

    pub unsafe fn lay_out(top: usize, entry: usize, arg: usize) -> usize {
        let sp = top - FRAME;
        let frame = sp as *mut usize;
        unsafe {
            std::ptr::write_bytes(frame, 0, FRAME / 8);
            frame.write(arg);
            frame.add(1).write(entry);
            frame
                .add(11)
                .write(ply_stack_trampoline as unsafe extern "C" fn() as usize);
        }
        sp
    }
}

#[cfg(target_arch = "x86_64")]
mod arch {
    use std::arch::naked_asm;

    /// rbp, rbx, r12..r15 and the return address; the trampoline is entered as after a call.
    pub const FRAME: usize = 64;

    #[unsafe(naked)]
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn ply_stack_switch(from: *mut usize, to: usize) {
        naked_asm!(
            "push rbp",
            "push rbx",
            "push r12",
            "push r13",
            "push r14",
            "push r15",
            "mov [rdi], rsp",
            "mov rsp, rsi",
            "pop r15",
            "pop r14",
            "pop r13",
            "pop r12",
            "pop rbx",
            "pop rbp",
            "ret",
        )
    }

    /// Where the first switch into a fresh stack lands; the `and` realigns for the ABI.
    #[unsafe(naked)]
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn ply_stack_trampoline() {
        naked_asm!("and rsp, -16", "mov rdi, rbx", "call r12", "ud2")
    }

    pub unsafe fn lay_out(top: usize, entry: usize, arg: usize) -> usize {
        let sp = top - FRAME;
        let frame = sp as *mut usize;
        unsafe {
            std::ptr::write_bytes(frame, 0, FRAME / 8);
            frame.add(3).write(entry);
            frame.add(4).write(arg);
            frame
                .add(6)
                .write(ply_stack_trampoline as unsafe extern "C" fn() as usize);
        }
        sp
    }
}

/// Saves the current stack pointer in `from` and continues `to`; returns when switched back.
///
/// # Safety
/// `to` must be a pointer a previous `switch` stored or [`Stack::prepare`] returned, for a stack
/// that is still mapped, and nothing may be running on it.
pub unsafe fn switch(from: &mut usize, to: usize) {
    unsafe { arch::ply_stack_switch(from, to) }
}

unsafe fn lay_out(top: usize, entry: usize, arg: usize) -> usize {
    unsafe { arch::lay_out(top, entry, arg) }
}

unsafe extern "C" {
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, offset: i64) -> *mut u8;
    fn munmap(addr: *mut u8, len: usize) -> i32;
    fn mprotect(addr: *mut u8, len: usize, prot: i32) -> i32;
}

const PROT_NONE: i32 = 0;
const PROT_READ: i32 = 1;
const PROT_WRITE: i32 = 2;
const MAP_PRIVATE: i32 = 0x02;
#[cfg(target_os = "macos")]
const MAP_ANONYMOUS: i32 = 0x1000;
#[cfg(target_os = "linux")]
const MAP_ANONYMOUS: i32 = 0x20;

unsafe fn mmap_anonymous(len: usize) -> *mut u8 {
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

unsafe fn mprotect_none(addr: *mut u8, len: usize) {
    unsafe { mprotect(addr, len, PROT_NONE) };
}
