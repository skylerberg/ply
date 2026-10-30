//! What the runtime holds of a value whose layout is emitted code's own, and what it may do with
//! one. A [`Block`] is a block emitted code made, of which the runtime reads the count at offset 0
//! and gives the memory back by size; a [`Handed`] is a word compiled code and the runtime pass each
//! other, which the runtime moves and never reads; a [`Carried`] is such a word kept beside its
//! type's [`PlyType`], counted only through it; a function value's block is called and let go
//! through its lambda's [`PlyFnDesc`]. None of them gives its word up, and every walker of the
//! runtime's own objects takes a [`Word`], so a walk of a value emitted code lays out cannot be
//! written.

use crate::heap::{self, HEADER, Heap, IMMORTAL, Obj, SHARED, Word};
use crate::native::{Returned, Machine, Taken};
use crate::rt::Ctx;

/// A block emitted code made, or a runtime object it held the last count of, as a native takes it.
#[repr(transparent)]
pub struct Block(Word);

impl Block {
    fn obj(&self) -> *mut Obj {
        self.0 as *mut Obj
    }

    /// A fresh block of `bytes`, its header among them, from `heap`: counted once, and holding
    /// past its header whatever the block last held.
    pub fn alloc(heap: &mut Heap, bytes: Word) -> Block {
        Block(heap.raw_alloc(payload(bytes)) as Word)
    }

    /// [`Block::alloc`] with an immortal count: no holder counts it and nothing frees it.
    pub fn alloc_immortal(heap: &mut Heap, bytes: Word) -> Block {
        let o = heap.raw_alloc(payload(bytes));
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
            matches!(unsafe { heap::count_of(o) }, rc if rc > SHARED && rc < heap::FRAME),
            "{self:?} was let go of as shared, and its count is not"
        );
        unsafe { heap::last_of_shared(o) }
    }

    /// Gives back a runtime handle whose last holder was the caller.
    ///
    /// # Safety
    /// The block is a live runtime handle, a bridge, a native object or a scoped value; `ctx` is
    /// the running entry's.
    pub unsafe fn free_handle(self, ctx: *mut Ctx) {
        unsafe { heap::free_handle(self.obj(), &raw mut (*ctx).heap, ctx) }
    }

    /// One holder more, counted as emitted code's `ply_dup` counts one: an immortal block and one in
    /// a frame keep their count, and a shared one is counted atomically.
    pub fn dup(&self) {
        let o = self.obj();
        let rc = unsafe { heap::count_of(o) };
        if rc < SHARED {
            unsafe { (*o).rc = rc + 1 };
        } else if rc < heap::FRAME {
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
            rc if rc >= heap::FRAME => false,
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
        unsafe { (self.desc().release)(ctx, Handed(self.0)) };
    }

    /// The descriptor of the function value this block is, which its first word names.
    ///
    /// # Safety
    /// The block is a closure emitted code laid out, and live.
    unsafe fn desc(&self) -> &'static PlyFnDesc {
        unsafe { &**(self.0 as *const *const PlyFnDesc).byte_add(HEADER) }
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

impl Handed {
    /// The word a call of compiled code passes or returns.
    pub(crate) fn word(self) -> Word {
        self.0
    }

    pub(crate) fn of(w: Word) -> Handed {
        Handed(w)
    }
}

/// A runtime object compiled code holds, which the runtime lays out and so may read: a bridge (a
/// task, a channel or a credential), what a native made, or a scoped value.
#[repr(transparent)]
pub struct Handle(Word);

impl Handle {
    pub(crate) fn of(w: Word) -> Handle {
        Handle(w)
    }

    pub(crate) fn word(&self) -> Word {
        self.0
    }
}

/// A `Secret`'s handle: the runtime object holding the credential ([`crate::secret::Sealed`]),
/// borrowed by a native that takes it and owned by the code a native returns it to.
#[repr(transparent)]
pub struct SecretHandle(Word);

impl SecretHandle {
    pub(crate) fn of(w: Word) -> SecretHandle {
        SecretHandle(w)
    }

    pub(crate) fn word(&self) -> Word {
        self.0
    }
}

/// `type#T`: a type's descriptor, static data emitted code lays out, through which the runtime
/// counts and marshals a value of the type it holds as a [`Carried`]. Each entry takes the value's
/// word form; what `inc`, `dec` and `lower` return is unread.
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
    /// Appends the value's encoding to the context's lowering, borrowing it: the runtime's error
    /// where no diagnostic shows a value of the type.
    pub lower: unsafe extern "C" fn(*mut Ctx, Word) -> Word,
    /// Reads a value of the type from the context's lifting, returning its word form: the runtime's
    /// error for every type but `String`.
    pub lift: unsafe extern "C" fn(*mut Ctx) -> Word,
}

// SAFETY: a descriptor is read-only data, which nothing writes once its unit is loaded.
unsafe impl Sync for PlyType {}

/// `kept#T`: what a value a perform hands a program's clause is kept beside, static data emitted
/// code lays out: its type's descriptor, and the entries comparing two of its values.
#[repr(C)]
pub struct PlyKept {
    pub ty: *const PlyType,
    /// 1 where two values of the type, each a word form it borrows, are equal, else 0; none where
    /// `==` is not built at the type.
    pub equal: Option<unsafe extern "C" fn(*mut Ctx, Word, Word) -> Word>,
    /// The sign of the order of two values of the type, each a word form it borrows; none where
    /// `compare` is not built at the type.
    pub order: Option<unsafe extern "C" fn(*mut Ctx, Word, Word) -> Word>,
}

// SAFETY: a descriptor is read-only data, which nothing writes once its unit is loaded.
unsafe impl Sync for PlyKept {}

/// `op#effect.op@T1..Tk`: an operation at its perform's type arguments, static data emitted code
/// lays out and hands an operation's entry, through whose types the runtime crosses what the
/// perform passes and is returned.
#[repr(C)]
pub struct PlyOp {
    /// The effect's and the operation's names, by their places in the unit's field table.
    pub effect: i64,
    pub op: i64,
    /// What the operation returns.
    pub result: *const PlyType,
    /// Which parameters are the operation's own type parameter, bit `i` for parameter `i`: those
    /// the runtime keeps as [`Carried`] values rather than reading.
    pub own: u64,
    /// The parameters' types, `params_len` of them, own ones marked.
    pub params_len: i64,
    pub params: *const *const PlyType,
    /// The operation's own type parameters, `vars_len` of them: what the runtime keeps values of.
    pub vars_len: i64,
    pub vars: *const *const PlyType,
}

// SAFETY: a descriptor is read-only data, which nothing writes once its unit is loaded.
unsafe impl Sync for PlyOp {}

impl PlyOp {
    pub fn params(&self) -> &[&'static PlyType] {
        unsafe { types(self.params, self.params_len) }
    }

    pub fn vars(&self) -> &[&'static PlyType] {
        unsafe { types(self.vars, self.vars_len) }
    }

    pub fn result(&self) -> &'static PlyType {
        unsafe { &*self.result }
    }

    /// Whether parameter `i` is the operation's own type parameter.
    pub fn owns(&self, i: usize) -> bool {
        i < 64 && self.own & (1 << i) != 0
    }
}

/// # Safety
/// `at` is null with `n` 0, or holds `n` descriptors that live as long as their unit.
unsafe fn types<'a>(at: *const *const PlyType, n: i64) -> &'a [&'static PlyType] {
    if n == 0 {
        return &[];
    }
    unsafe { std::slice::from_raw_parts(at.cast::<&'static PlyType>(), n as usize) }
}

/// A function value emitted code made, which the runtime calls and lets go of through the
/// [`PlyFnDesc`] its block names.
impl Block {
    /// What the function returns for `args`, each a word form it takes; the block is borrowed.
    ///
    /// # Safety
    /// The block is a closure emitted code laid out, and live; `ctx` is the running entry's.
    pub unsafe fn call(&self, ctx: *mut Ctx, args: &[Word]) -> Word {
        unsafe { (self.desc().uniform)(ctx, self.0, args.as_ptr()) }
    }

    /// Marks the function and what it holds shared, through its descriptor, before a thread other
    /// than this one may hold it: whether it may.
    ///
    /// # Safety
    /// As [`Block::call`].
    pub unsafe fn share(&self, ctx: *mut Ctx) -> bool {
        unsafe { (self.desc().share)(ctx, self.0) != 0 }
    }

    /// One holder of the function fewer, the last releasing it.
    ///
    /// # Safety
    /// As [`Block::call`], and the caller holds a count of it.
    pub unsafe fn let_go_function(self, ctx: *mut Ctx) {
        unsafe {
            if self.last() {
                self.release_function(ctx);
            }
        }
    }

    pub(crate) fn of(w: Word) -> Block {
        Block(w)
    }

    pub(crate) fn word(&self) -> Word {
        self.0
    }
}

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

    /// The word, for the entries of its type alone, which borrow it.
    pub(crate) fn word(&self) -> Word {
        self.word
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

/// A keep root, `keep#T`, as emitted code writes one: given a value of its type, which it borrows,
/// a copy of it on the unit's persistent heap, immortal, that outlives every entry (a value already
/// immortal returned as itself); or 0 where the value may not outlive the entry that made it, as
/// one holding a continuation, a scoped value, a task, a channel or a secret may not.
pub type KeepRoot = unsafe extern "C" fn(*mut Ctx, Word) -> Word;

/// A function value's descriptor, static data emitted code lays out per lambda, which the first word
/// past a closure block's header names. The runtime calls through `uniform`, releases through
/// `release`, shares through `share` and keeps through `keep`; it never calls `typed`.
#[repr(C)]
pub struct PlyFnDesc {
    /// Calls the function with the block and every argument as its word form.
    pub uniform: unsafe extern "C" fn(*mut Ctx, Word, *const Word) -> Word,
    /// The entry at the function type's own convention, cast to it where emitted code calls it;
    /// none for a function value the runtime made, which is applied through `uniform`.
    pub typed: Option<unsafe extern "C" fn()>,
    /// Which convention `typed` takes, which a checked build compares at a call.
    pub convention: Word,
    /// Releases a block whose last holder let it go: its captures, then its memory.
    pub release: DropRoot,
    /// Marks the block and what it holds shared, before a thread other than the one that made it
    /// may hold it: anything but 0, or 0 where a capture cannot cross threads. A block is marked
    /// only once everything it holds is, so one marked already returns without a walk.
    pub share: unsafe extern "C" fn(*mut Ctx, Word) -> Word,
    /// The block copied to outlive every entry, as a memoized constant does; 0 where a capture may
    /// not outlive the entry that made it.
    pub keep: unsafe extern "C" fn(*mut Ctx, Word) -> Word,
}

// SAFETY: a descriptor is read-only data, which nothing writes once its unit is loaded.
unsafe impl Sync for PlyFnDesc {}

// SAFETY: each is passed as one machine word, as an `int64_t` is.
unsafe impl Taken for Block {
    const MACHINE: Machine = Machine::Block;
}
unsafe impl Taken for Handed {
    const MACHINE: Machine = Machine::Handed;
}
unsafe impl Taken for Handle {
    const MACHINE: Machine = Machine::Handle;
}
unsafe impl Taken for SecretHandle {
    const MACHINE: Machine = Machine::Secret;
}
unsafe impl Taken for DropRoot {
    const MACHINE: Machine = Machine::DropRoot;
}
unsafe impl Taken for KeepRoot {
    const MACHINE: Machine = Machine::KeepRoot;
}
unsafe impl Taken for *const PlyType {
    const MACHINE: Machine = Machine::Type;
}
unsafe impl Taken for *const PlyOp {
    const MACHINE: Machine = Machine::Op;
}
unsafe impl Taken for *const PlyKept {
    const MACHINE: Machine = Machine::Type;
}
unsafe impl Taken for *const Handed {
    const MACHINE: Machine = Machine::Words;
}

// SAFETY: each is returned as one machine word, as an `int64_t` is.
unsafe impl Returned for Block {
    const MACHINE: Option<Machine> = Some(Machine::Block);
    const RAISES: bool = false;
    fn of(w: Word) -> Block {
        Block(w)
    }
}

unsafe impl Returned for Handed {
    const MACHINE: Option<Machine> = Some(Machine::Handed);
    const RAISES: bool = false;
    fn of(w: Word) -> Handed {
        Handed(w)
    }
}

unsafe impl Returned for Handle {
    const MACHINE: Option<Machine> = Some(Machine::Handle);
    const RAISES: bool = false;
    fn of(w: Word) -> Handle {
        Handle(w)
    }
}

unsafe impl Returned for SecretHandle {
    const MACHINE: Option<Machine> = Some(Machine::Secret);
    const RAISES: bool = false;
    fn of(w: Word) -> SecretHandle {
        SecretHandle(w)
    }
}
