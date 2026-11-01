use crate::heap::{self, HEADER, Heap, IMMORTAL, Obj, SHARED, Word};
use crate::native::{MachineType, Returned, Taken};
use crate::rt::Ctx;

#[repr(transparent)]
pub struct Block(Word);

impl Block {
    fn obj(&self) -> *mut Obj {
        self.0 as *mut Obj
    }

    pub fn alloc(heap: &mut Heap, bytes: Word) -> Block {
        Block(heap.raw_alloc(payload(bytes)) as Word)
    }

    pub fn alloc_immortal(heap: &mut Heap, bytes: Word) -> Block {
        let o = heap.raw_alloc(payload(bytes));
        unsafe { (*o).rc = IMMORTAL };
        Block(o as Word)
    }

    pub unsafe fn free(self, heap: *mut Heap, bytes: Word) {
        unsafe { heap::free_sized(self.obj(), heap, payload(bytes)) }
    }

    pub unsafe fn last_of_shared(self) -> bool {
        let o = self.obj();
        debug_assert!(
            matches!(unsafe { heap::count_of(o) }, rc if rc > SHARED && rc < heap::FRAME),
            "{self:?} was let go of as shared, and its count is not"
        );
        unsafe { heap::last_of_shared(o) }
    }

    pub unsafe fn free_handle(self, ctx: *mut Ctx) {
        unsafe { heap::free_handle(self.obj(), &raw mut (*ctx).heap, ctx) }
    }

    pub fn dup(&self) {
        let o = self.obj();
        let rc = unsafe { heap::count_of(o) };
        if rc < SHARED {
            unsafe { (*o).rc = rc + 1 };
        } else if rc < heap::FRAME {
            heap::inc_shared(self.0);
        }
    }

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

    pub unsafe fn release_function(self, ctx: *mut Ctx) {
        unsafe { (self.desc().release)(ctx, Handed(self.0)) };
    }

    unsafe fn desc(&self) -> &'static PlyFnDesc {
        unsafe { &**(self.0 as *const *const PlyFnDesc).byte_add(HEADER) }
    }
}

impl std::fmt::Debug for Block {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the block at {:#x}", self.0)
    }
}

fn payload(bytes: Word) -> usize {
    usize::try_from(bytes)
        .ok()
        .and_then(|b| b.checked_sub(HEADER))
        .unwrap_or_else(|| panic!("a block of {bytes} bytes is smaller than its header"))
}

#[repr(transparent)]
pub struct Handed(Word);

impl Handed {
    pub(crate) fn word(self) -> Word {
        self.0
    }

    pub(crate) fn of(w: Word) -> Handed {
        Handed(w)
    }
}

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

#[repr(C)]
pub struct PlyType {
    pub identity: u64,
    pub spelled: *const u8,
    pub spelled_len: u32,
    pub inc: unsafe extern "C" fn(*mut Ctx, Word) -> Word,
    pub dec: unsafe extern "C" fn(*mut Ctx, Word) -> Word,
    pub lower: unsafe extern "C" fn(*mut Ctx, Word) -> Word,
    pub lift: unsafe extern "C" fn(*mut Ctx) -> Word,
}

unsafe impl Sync for PlyType {}

#[repr(C)]
pub struct PlyKept {
    pub ty: *const PlyType,
    pub equal: Option<unsafe extern "C" fn(*mut Ctx, Word, Word) -> Word>,
    pub order: Option<unsafe extern "C" fn(*mut Ctx, Word, Word) -> Word>,
}

unsafe impl Sync for PlyKept {}

#[repr(C)]
pub struct PlyOp {
    pub effect: i64,
    pub op: i64,
    pub answer: *const PlyType,
    pub own: u64,
    pub params_len: i64,
    pub params: *const *const PlyType,
    pub vars_len: i64,
    pub vars: *const *const PlyType,
}

unsafe impl Sync for PlyOp {}

impl PlyOp {
    pub fn params(&self) -> &[&'static PlyType] {
        unsafe { types(self.params, self.params_len) }
    }

    pub fn vars(&self) -> &[&'static PlyType] {
        unsafe { types(self.vars, self.vars_len) }
    }

    pub fn result(&self) -> &'static PlyType {
        unsafe { &*self.answer }
    }

    pub fn owns(&self, i: usize) -> bool {
        i < 64 && self.own & (1 << i) != 0
    }
}

unsafe fn types<'a>(at: *const *const PlyType, n: i64) -> &'a [&'static PlyType] {
    if n == 0 {
        return &[];
    }
    unsafe { std::slice::from_raw_parts(at.cast::<&'static PlyType>(), n as usize) }
}

impl Block {
    pub unsafe fn call(&self, ctx: *mut Ctx, args: &[Word]) -> Word {
        unsafe { (self.desc().uniform)(ctx, self.0, args.as_ptr()) }
    }

    pub unsafe fn share(&self, ctx: *mut Ctx) -> bool {
        unsafe { (self.desc().share)(ctx, self.0) != 0 }
    }

    pub unsafe fn release_function_if_last(self, ctx: *mut Ctx) {
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

#[repr(C)]
#[must_use = "a carried value is released through its type or handed back"]
pub struct Carried {
    word: Word,
    ty: &'static PlyType,
}

impl Carried {
    pub unsafe fn new(w: Handed, ty: *const PlyType) -> Carried {
        Carried {
            word: w.0,
            ty: unsafe { &*ty },
        }
    }

    pub fn ty(&self) -> &'static PlyType {
        self.ty
    }

    pub(crate) fn word(&self) -> Word {
        self.word
    }

    pub fn dup(&self, ctx: *mut Ctx) -> Carried {
        unsafe { (self.ty.inc)(ctx, self.word) };
        Carried {
            word: self.word,
            ty: self.ty,
        }
    }

    pub fn release(self, ctx: *mut Ctx) {
        unsafe { (self.ty.dec)(ctx, self.word) };
    }

    pub fn handed(self) -> Handed {
        Handed(self.word)
    }
}

pub type DropRoot = unsafe extern "C" fn(*mut Ctx, Handed) -> Word;

#[repr(C)]
pub struct PlyFnDesc {
    pub uniform: unsafe extern "C" fn(*mut Ctx, Word, *const Word) -> Word,
    pub typed: Option<unsafe extern "C" fn()>,
    pub convention: Word,
    pub release: DropRoot,
    pub share: unsafe extern "C" fn(*mut Ctx, Word) -> Word,
    pub keep: unsafe extern "C" fn(*mut Ctx, Word) -> Word,
}

unsafe impl Sync for PlyFnDesc {}

unsafe impl Taken for Block {
    const MACHINE: MachineType = MachineType::Block;
}
unsafe impl Taken for Handed {
    const MACHINE: MachineType = MachineType::Handed;
}
unsafe impl Taken for Handle {
    const MACHINE: MachineType = MachineType::Handle;
}
unsafe impl Taken for SecretHandle {
    const MACHINE: MachineType = MachineType::Secret;
}
unsafe impl Taken for DropRoot {
    const MACHINE: MachineType = MachineType::DropRoot;
}
unsafe impl Taken for *const PlyType {
    const MACHINE: MachineType = MachineType::Type;
}
unsafe impl Taken for *const PlyOp {
    const MACHINE: MachineType = MachineType::Op;
}
unsafe impl Taken for *const PlyKept {
    const MACHINE: MachineType = MachineType::Type;
}
unsafe impl Taken for *const Handed {
    const MACHINE: MachineType = MachineType::Words;
}

unsafe impl Returned for Block {
    const MACHINE: Option<MachineType> = Some(MachineType::Block);
    fn of(w: Word) -> Block {
        Block(w)
    }
}

unsafe impl Returned for Handed {
    const MACHINE: Option<MachineType> = Some(MachineType::Handed);
    fn of(w: Word) -> Handed {
        Handed(w)
    }
}

unsafe impl Returned for Handle {
    const MACHINE: Option<MachineType> = Some(MachineType::Handle);
    fn of(w: Word) -> Handle {
        Handle(w)
    }
}

unsafe impl Returned for SecretHandle {
    const MACHINE: Option<MachineType> = Some(MachineType::Secret);
    fn of(w: Word) -> SecretHandle {
        SecretHandle(w)
    }
}
