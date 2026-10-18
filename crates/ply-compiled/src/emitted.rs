//! What the runtime holds of a value whose layout is emitted code's own, and what it may do with
//! one. A [`Block`] is a block emitted code made, of which the runtime reads the count at offset 0
//! and gives the memory back by size; a [`Handed`] is a word compiled code and the runtime pass each
//! other, which the runtime moves and never reads; a [`Carried`] is such a word kept beside its
//! type's [`PlyType`], counted only through it. None of them gives its word up, and every walker of
//! the runtime's own objects takes a [`Word`], so a walk of a value emitted code lays out cannot be
//! written.

use crate::heap::{self, HEADER, Heap, IMMORTAL, KIND_REFUSED, Obj, SHARED, Word};
use crate::native::{Answered, Taken};
use crate::rt::Ctx;

/// A block emitted code made, or a runtime object it held the last count of, as a native takes it.
#[repr(transparent)]
pub struct Block(Word);

impl Block {
    fn obj(&self) -> *mut Obj {
        self.0 as *mut Obj
    }

    /// A fresh block of `bytes`, its header among them, from `heap`: counted once, of
    /// [`KIND_REFUSED`], and holding past its header whatever the block last held.
    pub fn alloc(heap: &mut Heap, bytes: Word) -> Block {
        Block(heap.raw_alloc(KIND_REFUSED, 0, 0, 0, payload(bytes)) as Word)
    }

    /// [`Block::alloc`] with an immortal count: no holder counts it and nothing frees it.
    pub fn alloc_immortal(heap: &mut Heap, bytes: Word) -> Block {
        let o = heap.raw_alloc(KIND_REFUSED, 0, 0, 0, payload(bytes));
        unsafe { (*o).rc = IMMORTAL };
        Block(o as Word)
    }

    /// Gives the block back to the heap that owns it, `bytes` long with its header, as it was
    /// allocated. A debug build panics where its block says it was allocated at another size.
    ///
    /// # Safety
    /// The block is live, and its last holder was the caller; `heap` is the running entry's.
    pub unsafe fn free(self, heap: *mut Heap, bytes: Word) {
        unsafe { heap::free_sized(self.obj(), heap, payload(bytes)) }
    }

    /// One holder fewer of a shared block: whether that was the last.
    ///
    /// # Safety
    /// The block is live and shared, and the caller holds a count of it.
    pub unsafe fn last_of_shared(self) -> bool {
        let o = self.obj();
        debug_assert!(
            matches!(unsafe { heap::count_of(o) }, rc if rc > SHARED && rc != IMMORTAL),
            "{self:?} was let go of as shared, and its count is not"
        );
        unsafe { heap::last_of_shared(o) }
    }

    /// Gives back a runtime handle whose last holder was the caller. A continuation's stop is dropped
    /// at the next drain of the entry's dead continuations.
    ///
    /// # Safety
    /// The block is a live handle the runtime made: a bridge, a hold or a continuation; `ctx` is
    /// the running entry's.
    pub unsafe fn free_handle(self, ctx: *mut Ctx) {
        unsafe {
            crate::detached::died(ctx, self.obj());
            heap::free_handle(self.obj(), &raw mut (*ctx).heap);
        }
    }

    /// One holder more, counted as emitted code's `ply_dup` counts one: an immortal block keeps its
    /// count, and a shared one is counted atomically.
    pub fn dup(&self) {
        let o = self.obj();
        let rc = unsafe { heap::count_of(o) };
        if rc < SHARED {
            unsafe { (*o).rc = rc + 1 };
        } else if rc != IMMORTAL {
            heap::inc_shared(self.0);
        }
    }

    /// One holder fewer, counted as emitted code's `ply_last` counts one: whether the caller held
    /// the last, the block then the caller's to release.
    ///
    /// # Safety
    /// The block is live, and the caller holds a count of it.
    pub unsafe fn last(&self) -> bool {
        let o = self.obj();
        match unsafe { heap::count_of(o) } {
            IMMORTAL => false,
            rc if rc >= SHARED => unsafe { heap::last_of_shared(o) },
            rc if rc > 1 => {
                unsafe { (*o).rc = rc - 1 };
                false
            }
            rc => {
                debug_assert_eq!(rc, 1, "{self:?} was let go of after it died");
                true
            }
        }
    }

    /// Releases a function value whose last holder was the caller, through the release entry of
    /// the descriptor its first word names.
    ///
    /// # Safety
    /// The block is a closure emitted code laid out; `ctx` is the running entry's.
    pub unsafe fn release_function(self, ctx: *mut Ctx) {
        unsafe {
            let desc = &**(self.0 as *const *const PlyFnDesc).byte_add(HEADER);
            (desc.release)(ctx, Handed(self.0));
        }
    }
}

impl std::fmt::Debug for Block {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the block at {:#x}", self.0)
    }
}

/// What a block of `bytes` holds past its header.
fn payload(bytes: Word) -> usize {
    usize::try_from(bytes)
        .ok()
        .and_then(|b| b.checked_sub(HEADER))
        .unwrap_or_else(|| panic!("a block of {bytes} bytes is smaller than its header"))
}

/// A word compiled code hands the runtime, or the runtime hands it, as its type lays it out: moved,
/// never read.
#[repr(transparent)]
pub struct Handed(Word);

/// `type#T`: a type's descriptor, static data emitted code lays out, through which the runtime
/// counts and crosses a value of the type it holds as a [`Carried`]. Each entry takes the value's
/// word form; what `inc`, `dec` and `lower` answer is unread.
#[repr(C)]
pub struct PlyType {
    /// The low 64 bits of the digest of the type's spelling, which tells two types apart.
    pub identity: u64,
    /// The type's spelling, `spelled_len` bytes of UTF-8, for a diagnostic.
    pub spelled: *const u8,
    pub spelled_len: u32,
    /// One holder more of a value of the type.
    pub inc: unsafe extern "C" fn(*mut Ctx, Word) -> Word,
    /// One holder fewer, the last releasing the value as its type does.
    pub dec: unsafe extern "C" fn(*mut Ctx, Word) -> Word,
    /// Appends the value's encoding to the context's lowering, borrowing it.
    pub lower: unsafe extern "C" fn(*mut Ctx, Word) -> Word,
    /// Reads a value of the type from the context's lifting, answering its word form.
    pub lift: unsafe extern "C" fn(*mut Ctx) -> Word,
}

// SAFETY: a descriptor is read-only data, which nothing writes once its unit is loaded.
unsafe impl Sync for PlyType {}

/// A value compiled code handed the runtime with its type: held, and counted only through its
/// [`PlyType`]. Nothing but the type reads its word.
#[repr(C)]
#[must_use = "a carried value is released through its type or handed back"]
pub struct Carried {
    word: Word,
    ty: &'static PlyType,
}

impl Carried {
    /// `w`, a value of the type `ty` describes, holding the count compiled code handed with it.
    ///
    /// # Safety
    /// `ty` is a descriptor emitted code laid out, alive as long as its unit, and `w` a word form of
    /// its type.
    pub unsafe fn new(w: Handed, ty: *const PlyType) -> Carried {
        Carried {
            word: w.0,
            ty: unsafe { &*ty },
        }
    }

    pub fn ty(&self) -> &'static PlyType {
        self.ty
    }

    /// The value held once more, counted through its type.
    pub fn dup(&self, ctx: *mut Ctx) -> Carried {
        unsafe { (self.ty.inc)(ctx, self.word) };
        Carried {
            word: self.word,
            ty: self.ty,
        }
    }

    /// Lets the value go through its type, whose last holder releases it.
    pub fn release(self, ctx: *mut Ctx) {
        unsafe { (self.ty.dec)(ctx, self.word) };
    }

    /// The value handed to compiled code, which takes this count of it.
    pub fn handed(self) -> Handed {
        Handed(self.word)
    }
}

/// A drop root, as emitted code writes one: the release of a value whose last holder let it go,
/// given the word that holder had.
pub type DropRoot = unsafe extern "C" fn(*mut Ctx, Handed) -> Word;

/// A function value's descriptor, static data emitted code lays out per lambda, which the first word
/// past a closure block's header names. The runtime calls through `uniform` and releases through
/// `release`; it reads none of the rest.
#[repr(C)]
pub struct PlyFnDesc {
    /// Calls the function with the block and every argument as its word form.
    pub uniform: unsafe extern "C" fn(*mut Ctx, Word, *const Word) -> Word,
    /// The entry at the function type's own convention, which only emitted code calls.
    pub typed: *const (),
    pub convention: Word,
    /// Releases a block whose last holder let it go: its captures, then its memory.
    pub release: DropRoot,
    pub share: *const (),
    pub keep: *const (),
}

// SAFETY: a descriptor is read-only data, which nothing writes once its unit is loaded.
unsafe impl Sync for PlyFnDesc {}

// SAFETY: each is passed as one machine word, as an `int64_t` is.
unsafe impl Taken for Block {}
unsafe impl Taken for Handed {}
unsafe impl Taken for DropRoot {}

// SAFETY: each is returned as one machine word, as an `int64_t` is.
unsafe impl Answered for Block {
    const ANSWERS: bool = true;
    const RAISES: bool = false;
    fn of(w: Word) -> Block {
        Block(w)
    }
}

unsafe impl Answered for Handed {
    const ANSWERS: bool = true;
    const RAISES: bool = false;
    fn of(w: Word) -> Handed {
        Handed(w)
    }
}
