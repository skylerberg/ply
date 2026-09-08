//! Stacks the runtime owns, and the switch between them.
//!
//! A suspended computation in the compiled tier is a C stack: a task body runs on one of these,
//! and the scheduler's decision of who runs next is [`switch`]. Nothing emitted knows which stack
//! it is on. The switch saves the callee-saved registers on the stack being left, stores its
//! stack pointer where the caller asked, and resumes the other stack from its saved pointer; a
//! stack that has never run is laid out by [`Stack::prepare`] so that the first switch into it
//! returns into a trampoline that calls the entry.

use std::ptr;

/// Reserved per stack. Pages are mapped on first touch, so the reservation costs address space
/// until a frame reaches it, and the guard page below turns an overrun into a fault rather than a
/// write into whatever was mapped there.
pub const STACK_SIZE: usize = 8 * 1024 * 1024;

const GUARD: usize = 16 * 1024;

pub struct Stack {
    base: *mut u8,
    size: usize,
}

impl Stack {
    pub fn new() -> Stack {
        let size = STACK_SIZE + GUARD;
        let base = unsafe { mmap_anonymous(size) };
        assert!(!base.is_null(), "a task stack could not be reserved");
        unsafe { mprotect_none(base, GUARD) };
        Stack { base, size }
    }

    /// The lowest address a compiled frame may begin at, in the same terms as the thread's own
    /// floor: the guard, then the margin the runtime's Rust frames need under a compiled one.
    pub fn floor(&self) -> usize {
        self.base as usize + GUARD + crate::rt::STACK_MARGIN
    }

    pub fn top(&self) -> usize {
        self.base as usize + self.size
    }

    pub fn holds(&self, address: usize) -> bool {
        (self.base as usize..self.top()).contains(&address)
    }

    /// Lays out the frame the first [`switch`] into this stack will pop: the callee-saved
    /// registers, with the return address set to a trampoline that calls `entry(arg)`. `entry`
    /// must switch away for the last time itself; there is nothing below it to return into.
    pub fn prepare(&self, entry: extern "C" fn(usize), arg: usize) -> usize {
        let top = self.top() & !15;
        unsafe { lay_out(top, entry as usize, arg) }
    }

    /// The bytes live on this stack when `sp` is its stack pointer: everything from `sp` to the
    /// top, which is what a snapshot copies and what a restore writes back in place.
    pub fn live(&self, sp: usize) -> &[u8] {
        debug_assert!(self.holds(sp));
        unsafe { std::slice::from_raw_parts(sp as *const u8, self.top() - sp) }
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

    /// Bytes the saved frame takes: x19..x28, x29, x30 and d8..d15, rounded to the 16-byte
    /// alignment the stack pointer must keep.
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

    /// Where the first switch into a fresh stack lands: the entry is in x20 and its argument in
    /// x19, exactly where [`lay_out`] put them.
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

    /// rbp, rbx, r12..r15 and the return address, laid out so the trampoline is entered with the
    /// stack pointer eight past a sixteen-byte boundary, as after a call.
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

    /// Where the first switch into a fresh stack lands, with the stack pointer eight past a
    /// sixteen-byte boundary as after a call; the `and` puts it on the boundary so that the call
    /// below enters `entry` as the ABI requires.
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

/// Leaves the current stack, storing its stack pointer in `from`, and continues the stack whose
/// pointer is `to`. Returns when something switches back to `*from`.
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct Pong {
        main: usize,
        task: usize,
        log: Vec<usize>,
    }

    thread_local! {
        static PONG: RefCell<Option<Pong>> = const { RefCell::new(None) };
    }

    fn with<R>(f: impl FnOnce(&mut Pong) -> R) -> R {
        PONG.with(|p| f(p.borrow_mut().as_mut().unwrap()))
    }

    /// Switches back to the test's stack, storing this stack's pointer where the test reads it.
    fn yield_to_main() {
        let (task, main) = with(|p| (&mut p.task as *mut usize, p.main));
        unsafe { switch(&mut *task, main) };
    }

    fn run_task() {
        let (main, task) = with(|p| (&mut p.main as *mut usize, p.task));
        unsafe { switch(&mut *main, task) };
    }

    /// The address of a sixteen-byte-aligned local, which is aligned only if the frame was
    /// entered as the ABI requires; a trampoline that enters off by a word is caught here.
    #[inline(never)]
    fn aligned_local() -> usize {
        #[repr(align(16))]
        struct Aligned([u8; 16]);
        let a = Aligned([0; 16]);
        std::hint::black_box(&a.0) as *const [u8; 16] as usize
    }

    extern "C" fn count_to(n: usize) {
        assert_eq!(
            aligned_local() % 16,
            0,
            "the task was entered off the ABI's alignment"
        );
        for i in 1..=n {
            let local = i * 10;
            with(|p| p.log.push(local));
            yield_to_main();
        }
        with(|p| p.log.push(0));
        yield_to_main();
        unreachable!("a finished task was resumed");
    }

    #[test]
    fn a_task_runs_on_its_own_stack_and_yields_back_in_order() {
        let stack = Stack::new();
        let sp = stack.prepare(count_to, 3);
        PONG.with(|p| {
            *p.borrow_mut() = Some(Pong {
                main: 0,
                task: sp,
                log: Vec::new(),
            })
        });
        let mut seen = Vec::new();
        for _ in 0..4 {
            run_task();
            seen.push(with(|p| *p.log.last().unwrap()));
            assert!(stack.holds(with(|p| p.task)));
        }
        assert_eq!(seen, [10, 20, 30, 0]);
        PONG.with(|p| *p.borrow_mut() = None);
    }

    #[test]
    fn a_snapshot_restored_in_place_resumes_the_same_frame_again() {
        let stack = Stack::new();
        let sp = stack.prepare(count_to, 2);
        PONG.with(|p| {
            *p.borrow_mut() = Some(Pong {
                main: 0,
                task: sp,
                log: Vec::new(),
            })
        });
        let run = run_task;
        run();
        assert_eq!(with(|p| p.log.clone()), [10]);
        let captured = with(|p| p.task);
        let snapshot = stack.live(captured).to_vec();
        run();
        assert_eq!(with(|p| p.log.clone()), [10, 20]);
        unsafe { stack.restore(captured, &snapshot) };
        with(|p| p.task = captured);
        run();
        assert_eq!(
            with(|p| p.log.clone()),
            [10, 20, 20],
            "the second resumption of the captured frame counts from where it was captured"
        );
        run();
        assert_eq!(with(|p| p.log.clone()), [10, 20, 20, 0]);
        PONG.with(|p| *p.borrow_mut() = None);
    }
}
