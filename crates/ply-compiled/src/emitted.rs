use crate::heap::{self, HEADER, Heap, IMMORTAL, Object, SHARED, Word};
use crate::native::{MachineType, Returned, Taken};
use crate::rt::Context;

#[repr(transparent)]
pub struct Block(Word);

impl Block {
    fn object(&self) -> *mut Object {
        self.0 as *mut Object
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
        unsafe { heap::free_sized(self.object(), heap, payload(bytes)) }
    }

    pub unsafe fn last_of_shared(self) -> bool {
        let o = self.object();
        debug_assert!(
            matches!(unsafe { heap::count_of(o) }, rc if rc > SHARED && rc < heap::FRAME),
            "{self:?} was let go of as shared, and its count is not"
        );
        unsafe { heap::last_of_shared(o) }
    }

    pub unsafe fn free_handle(self, context: *mut Context) {
        unsafe { heap::free_handle(self.object(), &raw mut (*context).heap, context) }
    }

    pub fn dup(&self) {
        let o = self.object();
        let rc = unsafe { heap::count_of(o) };
        if rc < SHARED {
            unsafe { (*o).rc = rc + 1 };
        } else if rc < heap::FRAME {
            heap::inc_shared(self.0);
        }
    }

    pub unsafe fn last(&self) -> bool {
        let o = self.object();
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

    pub unsafe fn release_function(self, context: *mut Context) {
        unsafe { (self.descriptor().release)(context, OwnedWord(self.0)) };
    }

    unsafe fn descriptor(&self) -> &'static PlyFunctionDescriptor {
        unsafe { &**(self.0 as *const *const PlyFunctionDescriptor).byte_add(HEADER) }
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
pub struct OwnedWord(Word);

impl OwnedWord {
    pub(crate) fn word(self) -> Word {
        self.0
    }

    pub(crate) fn of(w: Word) -> OwnedWord {
        OwnedWord(w)
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
    pub inc: unsafe extern "C" fn(*mut Context, Word) -> Word,
    pub dec: unsafe extern "C" fn(*mut Context, Word) -> Word,
    pub lower: unsafe extern "C" fn(*mut Context, Word) -> Word,
    pub lift: unsafe extern "C" fn(*mut Context) -> Word,
}

unsafe impl Sync for PlyType {}

#[repr(C)]
pub struct PlyKept {
    pub ty: *const PlyType,
    pub equal: Option<unsafe extern "C" fn(*mut Context, Word, Word) -> Word>,
    pub order: Option<unsafe extern "C" fn(*mut Context, Word, Word) -> Word>,
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
    pub variables_length: i64,
    pub variables: *const *const PlyType,
}

unsafe impl Sync for PlyOp {}

impl PlyOp {
    pub fn params(&self) -> &[&'static PlyType] {
        unsafe { types(self.params, self.params_len) }
    }

    pub fn variables(&self) -> &[&'static PlyType] {
        unsafe { types(self.variables, self.variables_length) }
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
    pub unsafe fn call(&self, context: *mut Context, args: &[Word]) -> Word {
        unsafe { (self.descriptor().uniform)(context, self.0, args.as_ptr()) }
    }

    pub unsafe fn share(&self, context: *mut Context) -> bool {
        unsafe { (self.descriptor().share)(context, self.0) != 0 }
    }

    pub unsafe fn release_function_if_last(self, context: *mut Context) {
        unsafe {
            if self.last() {
                self.release_function(context);
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
    pub unsafe fn new(w: OwnedWord, ty: *const PlyType) -> Carried {
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

    pub fn dup(&self, context: *mut Context) -> Carried {
        unsafe { (self.ty.inc)(context, self.word) };
        Carried {
            word: self.word,
            ty: self.ty,
        }
    }

    pub fn release(self, context: *mut Context) {
        unsafe { (self.ty.dec)(context, self.word) };
    }

    pub fn into_owned_word(self) -> OwnedWord {
        OwnedWord(self.word)
    }
}

pub type DropRoot = unsafe extern "C" fn(*mut Context, OwnedWord) -> Word;

pub type KeepRoot = unsafe extern "C" fn(*mut Context, Word) -> Word;

#[repr(C)]
pub struct PlyFunctionDescriptor {
    pub uniform: unsafe extern "C" fn(*mut Context, Word, *const Word) -> Word,
    pub typed: Option<unsafe extern "C" fn()>,
    pub convention: Word,
    pub release: DropRoot,
    pub share: unsafe extern "C" fn(*mut Context, Word) -> Word,
    pub keep: unsafe extern "C" fn(*mut Context, Word) -> Word,
}

unsafe impl Sync for PlyFunctionDescriptor {}

unsafe impl Taken for Block {
    const MACHINE: MachineType = MachineType::Block;
}
unsafe impl Taken for OwnedWord {
    const MACHINE: MachineType = MachineType::OwnedWord;
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
unsafe impl Taken for KeepRoot {
    const MACHINE: MachineType = MachineType::KeepRoot;
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
unsafe impl Taken for *const OwnedWord {
    const MACHINE: MachineType = MachineType::Words;
}

unsafe impl Returned for Block {
    const MACHINE: Option<MachineType> = Some(MachineType::Block);
    fn of(w: Word) -> Block {
        Block(w)
    }
}

unsafe impl Returned for OwnedWord {
    const MACHINE: Option<MachineType> = Some(MachineType::OwnedWord);
    fn of(w: Word) -> OwnedWord {
        OwnedWord(w)
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
