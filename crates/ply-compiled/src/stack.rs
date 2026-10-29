//! Stacks the runtime owns, and the [`switch`] between them: a suspended computation in the
//! C backend is a C stack, and nothing emitted knows which stack it is on. Each is one reservation
//! that never moves, made writable from its top down as the computation on it goes deeper, so every
//! frame keeps its address however deep it lies.

use crate::rt::STACK_MARGIN;
use crate::vm;
use ply_core::limit::{self, Reserved};
use std::cell::{Cell, RefCell};
use std::mem::ManuallyDrop;
use std::ptr::NonNull;

/// The address space each stack reserves, which bounds how deeply a computation on it nests.
pub const RESERVE: usize = 1 << 30;

/// Made writable at a time, from the top down; a stack starts with one.
pub const CHUNK: usize = 8 << 20;

/// Reserved below the stack and never made writable: a multiple of every page size (4, 16 and
/// 64 KB), so the chunks above it start on a page.
const GUARD: usize = 64 << 10;

/// The stacks a thread keeps that never grew past their first chunk, for its next bodies and tasks.
const POOLED: usize = 4;

thread_local! {
    static POOL: RefCell<Vec<Reservation>> = const { RefCell::new(Vec::new()) };
}

/// A stack of the runtime's. Dropping it gives it back: to this thread's pool while it never grew
/// past its first chunk, else to the system. Boxed, so the name the running stack goes by holds
/// wherever the `Stack` moves.
pub struct Stack(ManuallyDrop<Box<Reservation>>);

struct Reservation {
    base: usize,
    /// The lowest writable address.
    committed: Cell<usize>,
}

impl Reservation {
    fn new() -> Option<Reservation> {
        let base = unsafe { vm::reserve(GUARD + RESERVE) } as usize;
        if base == 0 {
            return None;
        }
        let r = Reservation {
            base,
            committed: Cell::new(base + GUARD + RESERVE),
        };
        r.extend().then_some(r)
    }

    fn top(&self) -> usize {
        self.base + GUARD + RESERVE
    }
}

impl Reserved for Reservation {
    fn writable(&self) -> (usize, usize) {
        (self.committed.get(), self.top())
    }

    fn extend(&self) -> bool {
        let next = self.committed.get() - CHUNK;
        if next < self.base + GUARD || !unsafe { vm::commit(next as *mut u8, CHUNK) } {
            return false;
        }
        self.committed.set(next);
        true
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        unsafe { vm::unmap(self.base as *mut u8, GUARD + RESERVE) };
    }
}

impl Stack {
    /// A stack of its own: one this thread gave back as it took it, or a new one with its first
    /// chunk writable. `None` when the system maps no more.
    pub fn take() -> Option<Stack> {
        let pooled = POOL.try_with(|p| p.borrow_mut().pop()).ok().flatten();
        let reservation = pooled.or_else(Reservation::new)?;
        Some(Stack(ManuallyDrop::new(Box::new(reservation))))
    }

    /// The lowest address a compiled frame may begin at: what is writable, less the Rust frames'
    /// margin.
    pub fn floor(&self) -> usize {
        self.0.committed.get() + STACK_MARGIN
    }

    /// Makes the next chunk below writable, lowering the floor by as much; `false` once the
    /// reservation is spent.
    pub fn extend(&self) -> bool {
        self.0.extend()
    }

    pub fn top(&self) -> usize {
        self.0.top()
    }

    pub fn holds(&self, address: usize) -> bool {
        (self.0.base..self.top()).contains(&address)
    }

    /// Names this stack as the one this thread runs on, which a computation's first frame on it
    /// does before anything else, and answers the floor compiled code honours there.
    ///
    /// # Safety
    /// The stack outlives its time as the running one.
    #[must_use]
    pub unsafe fn enter(&self) -> usize {
        unsafe { limit::set_running(Some(self.reserved())) };
        self.floor()
    }

    fn reserved(&self) -> NonNull<dyn Reserved> {
        let reservation: &(dyn Reserved + 'static) = &**self.0;
        NonNull::from(reservation)
    }

    /// Lays out the frame the first [`switch`] pops, returning into a trampoline that calls
    /// `entry(arg)`, which must never return: there is nothing below it.
    pub fn prepare(&self, entry: extern "C" fn(usize), arg: usize) -> usize {
        let top = self.top() & !15;
        unsafe { lay_out(top, entry as usize, arg) }
    }
}

impl Drop for Stack {
    fn drop(&mut self) {
        debug_assert!(
            limit::running().is_none_or(|r| r.cast::<()>() != self.reserved().cast::<()>()),
            "a stack was given back while this thread runs on it"
        );
        let reservation = *unsafe { ManuallyDrop::take(&mut self.0) };
        if reservation.committed.get() == reservation.top() - CHUNK {
            let _ = POOL.try_with(move |p| {
                let mut pool = p.borrow_mut();
                if pool.len() < POOLED {
                    pool.push(reservation);
                }
            });
        }
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
