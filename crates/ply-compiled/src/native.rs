//! What a function compiled code calls through a unit's table takes and returns, read off its Rust
//! type, so what the runtime reports of a native is the function it binds and nothing written apart.

use crate::heap::Word;
use crate::rt::Ctx;

/// What a native's parameter or result is at the machine: what compiled code passes for it, each
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
    /// A runtime object the runtime lays out: a bridge, a scoped value or what a native made.
    Handle,
    /// A value's word form, which the runtime moves and never reads.
    Handed,
    /// A block emitted code lays out, of which the runtime reads the count at offset 0; a function
    /// value's is called and let go through its descriptor.
    Block,
    /// A type's descriptor: `type#T`, or `kept#T`, which names `type#T` beside its comparisons.
    Type,
    /// An operation's descriptor, `op#..`.
    Op,
    /// A drop root's address.
    DropRoot,
    /// A keep root's address.
    KeepRoot,
    /// How many bytes a native left as the context's taken bytes, which emitted code copies out.
    Taken,
    /// A `Secret`'s handle: borrowed by a native that takes one, owned by the code a native returns
    /// one to.
    Secret,
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
            Machine::Taken => "Taken",
            Machine::Secret => "Secret",
        }
    }
}

/// What a native takes past the context and what it returns, as its function's type says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Signature {
    pub params: &'static [Machine],
    /// `None` for a native that returns nothing.
    pub result: Option<Machine>,
}

impl Signature {
    /// The words a call passes past the context.
    pub fn words(&self) -> usize {
        self.params.len()
    }

    pub fn results(&self) -> bool {
        self.result.is_some()
    }
}

/// A raw 64-bit integer compiled code passes or receives.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct I64(pub i64);

/// A `Float`'s bits, as compiled code passes or receives them.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct F64(pub u64);

/// The address of bytes a native borrows from compiled code, to read or write as the native says;
/// their count is a parameter beside it.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Addr(pub *mut u8);

/// How many bytes a native left as the context's taken bytes ([`Ctx::give`]), which compiled code
/// copies out into a string or a byte string of its own.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TakenBytes(pub i64);

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

/// What a native returns to compiled code.
///
/// # Safety
/// A type with a result is returned as the C calling convention returns one `int64_t`, which is
/// what the emitted C declares a native with a result to return; one without is `()`.
pub unsafe trait Returned {
    /// `None` for `()`.
    const MACHINE: Option<Machine>;
    /// What the function returns when its work returned `w`.
    fn of(w: Word) -> Self;
}

unsafe impl Returned for () {
    const MACHINE: Option<Machine> = None;
    fn of(_: Word) -> Self {}
}

unsafe impl Returned for Word {
    const MACHINE: Option<Machine> = Some(Machine::Word);
    fn of(w: Word) -> Self {
        w
    }
}

unsafe impl Returned for I64 {
    const MACHINE: Option<Machine> = Some(Machine::I64);
    fn of(w: Word) -> Self {
        I64(w)
    }
}

unsafe impl Returned for F64 {
    const MACHINE: Option<Machine> = Some(Machine::F64);
    fn of(w: Word) -> Self {
        F64(w as u64)
    }
}

unsafe impl Returned for TakenBytes {
    const MACHINE: Option<Machine> = Some(Machine::Taken);
    fn of(w: Word) -> Self {
        TakenBytes(w)
    }
}

/// The type of a function compiled code calls with its context and machine words.
pub trait Native: Copy {
    const SIGNATURE: Signature;
    fn address(self) -> *const ();
}

macro_rules! natives_taking {
    ($($a:ident)*) => {
        impl<R: Returned, $($a: Taken),*> Native for unsafe extern "C" fn(*mut Ctx $(, $a)*) -> R {
            const SIGNATURE: Signature = Signature {
                params: &[$($a::MACHINE),*],
                result: R::MACHINE,
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
natives_taking!(A B C D E F G);
natives_taking!(A B C D E F G H);
natives_taking!(A B C D E F G H I);
natives_taking!(A B C D E F G H I J);
natives_taking!(A B C D E F G H I J K);
natives_taking!(A B C D E F G H I J K L);
