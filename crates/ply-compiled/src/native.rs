//! What a function compiled code calls through a unit's table takes and answers, read off its Rust
//! type, so what the runtime reports of a native is the function it binds and nothing written apart.

use crate::heap::Word;
use crate::rt::Ctx;

/// What a native's parameter or answer is at the machine: what compiled code passes for it, each
/// in one machine word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Machine {
    /// A uniform word, which holds a value of any type.
    Word,
    /// The address of uniform words.
    Words,
    /// A 64-bit integer, raw: an `Int`, a width of at most 64 bits, a `Bool` as 0 or 1, a `Char`'s
    /// scalar value, or a code.
    I64,
    /// A `Float`'s bits.
    F64,
    /// The address of bytes, their count passed beside it.
    Addr,
    /// The address of words the native writes.
    Out,
    /// A runtime object the runtime lays out: a bridge or a hold.
    Handle,
    /// A value's word form, which the runtime moves and never reads.
    Handed,
    /// A block emitted code lays out, of which the runtime reads the count at offset 0; a function
    /// value's is called and let go through its descriptor.
    Block,
    /// A type's descriptor, `type#T`.
    Type,
    /// An operation's descriptor, `op#..`.
    Op,
    /// A drop root's address.
    DropRoot,
    /// A keep root's address.
    KeepRoot,
}

impl Machine {
    /// The name a runtime's description lists the kind under.
    pub fn name(self) -> &'static str {
        match self {
            Machine::Word => "Word",
            Machine::Words => "Words",
            Machine::I64 => "I64",
            Machine::F64 => "F64",
            Machine::Addr => "Addr",
            Machine::Out => "Out",
            Machine::Handle => "Handle",
            Machine::Handed => "Handed",
            Machine::Block => "Block",
            Machine::Type => "Type",
            Machine::Op => "Op",
            Machine::DropRoot => "DropRoot",
            Machine::KeepRoot => "KeepRoot",
        }
    }
}

/// What a native takes past the context and what it answers, as its function's type says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Signature {
    pub params: &'static [Machine],
    /// `None` for a native that answers nothing.
    pub answer: Option<Machine>,
    /// Whether a failure of it is a raise of `abort.raise`, which a `handle` answers, rather than a
    /// fault that ends the run.
    pub raises: bool,
}

impl Signature {
    /// The words a call passes past the context.
    pub fn words(&self) -> usize {
        self.params.len()
    }

    pub fn answers(&self) -> bool {
        self.answer.is_some()
    }
}

/// The word a native that may raise answers: meaningless where it left a raise in the context.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Raising(pub Word);

/// What a native that answers nothing but may raise returns: nothing, the raise left in the context.
/// Transparent over `()`, it is returned as `()` is, so the emitted C declares the native `void`;
/// a native returning it allows `improper_ctypes_definitions`, which does not know that.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Raises(pub ());

/// A raw 64-bit integer compiled code passes or is answered.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct I64(pub i64);

/// A `Float`'s bits, as compiled code passes or is answered them.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct F64(pub u64);

/// The address of bytes compiled code lends a native, to read or to write as the native says;
/// their count is a parameter beside it.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Addr(pub *mut u8);

/// The address of words a native writes for compiled code, as many as the native says.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Out(pub *mut i64);

/// A parameter compiled code passes in one machine word.
///
/// # Safety
/// The type is passed as the C calling convention passes one `int64_t`, which is what the emitted C
/// declares every parameter of a native as.
pub unsafe trait Taken {
    const MACHINE: Machine;
}

unsafe impl Taken for Word {
    const MACHINE: Machine = Machine::Word;
}

unsafe impl Taken for *const Word {
    const MACHINE: Machine = Machine::Words;
}

unsafe impl Taken for I64 {
    const MACHINE: Machine = Machine::I64;
}

unsafe impl Taken for F64 {
    const MACHINE: Machine = Machine::F64;
}

unsafe impl Taken for Addr {
    const MACHINE: Machine = Machine::Addr;
}

unsafe impl Taken for Out {
    const MACHINE: Machine = Machine::Out;
}

/// What a native answers compiled code.
///
/// # Safety
/// A type that answers is returned as the C calling convention returns one `int64_t`, which is what
/// the emitted C declares a native that answers to return; one that does not is `()` or [`Raises`].
pub unsafe trait Answered {
    /// `None` for `()` and [`Raises`].
    const MACHINE: Option<Machine>;
    const RAISES: bool;
    /// What the function answers when its work answered `w`.
    fn of(w: Word) -> Self;
}

unsafe impl Answered for () {
    const MACHINE: Option<Machine> = None;
    const RAISES: bool = false;
    fn of(_: Word) -> Self {}
}

unsafe impl Answered for Word {
    const MACHINE: Option<Machine> = Some(Machine::Word);
    const RAISES: bool = false;
    fn of(w: Word) -> Self {
        w
    }
}

unsafe impl Answered for Raising {
    const MACHINE: Option<Machine> = Some(Machine::Word);
    const RAISES: bool = true;
    fn of(w: Word) -> Self {
        Raising(w)
    }
}

unsafe impl Answered for Raises {
    const MACHINE: Option<Machine> = None;
    const RAISES: bool = true;
    fn of(_: Word) -> Self {
        Raises(())
    }
}

unsafe impl Answered for I64 {
    const MACHINE: Option<Machine> = Some(Machine::I64);
    const RAISES: bool = false;
    fn of(w: Word) -> Self {
        I64(w)
    }
}

unsafe impl Answered for F64 {
    const MACHINE: Option<Machine> = Some(Machine::F64);
    const RAISES: bool = false;
    fn of(w: Word) -> Self {
        F64(w as u64)
    }
}

/// The type of a function compiled code calls with its context and machine words.
pub trait Native: Copy {
    const SIGNATURE: Signature;
    fn address(self) -> *const ();
}

macro_rules! natives_taking {
    ($($a:ident)*) => {
        impl<R: Answered, $($a: Taken),*> Native for unsafe extern "C" fn(*mut Ctx $(, $a)*) -> R {
            const SIGNATURE: Signature = Signature {
                params: &[$($a::MACHINE),*],
                answer: R::MACHINE,
                raises: R::RAISES,
            };
            fn address(self) -> *const () {
                self as *const ()
            }
        }
    };
}

natives_taking!();
natives_taking!(A);
natives_taking!(A B);
natives_taking!(A B C);
natives_taking!(A B C D);
natives_taking!(A B C D E);
natives_taking!(A B C D E F);
