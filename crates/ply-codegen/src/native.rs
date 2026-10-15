//! What a function compiled code calls through a unit's table takes and answers, read off its Rust
//! type, so what the runtime reports of a native is the function it binds and nothing written apart.

use crate::heap::Word;
use crate::rt::Ctx;

/// What a native takes past the context and what it answers, as its function's type says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Signature {
    pub words: usize,
    pub answers: bool,
    /// Whether a failure of it is a raise of `abort.raise`, which a `handle` answers, rather than a
    /// fault that ends the run.
    pub raises: bool,
}

/// The word a native that may raise answers: meaningless where it left a raise in the context.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Raising(pub Word);

/// A parameter compiled code passes in one machine word.
///
/// # Safety
/// The type is passed as the C calling convention passes one `int64_t`, which is what the emitted C
/// declares every parameter of a native as.
pub unsafe trait Taken {}

unsafe impl Taken for Word {}
unsafe impl Taken for *const Word {}

/// What a native answers compiled code.
///
/// # Safety
/// A type that answers is returned as the C calling convention returns one `int64_t`, which is what
/// the emitted C declares a native that answers to return; one that does not is `()`.
pub unsafe trait Answered {
    const ANSWERS: bool;
    const RAISES: bool;
    /// What the function answers when its work answered `w`.
    fn of(w: Word) -> Self;
}

unsafe impl Answered for () {
    const ANSWERS: bool = false;
    const RAISES: bool = false;
    fn of(_: Word) -> Self {}
}

unsafe impl Answered for Word {
    const ANSWERS: bool = true;
    const RAISES: bool = false;
    fn of(w: Word) -> Self {
        w
    }
}

unsafe impl Answered for Raising {
    const ANSWERS: bool = true;
    const RAISES: bool = true;
    fn of(w: Word) -> Self {
        Raising(w)
    }
}

/// The type of a function compiled code calls with its context and machine words.
pub trait Native: Copy {
    const SIGNATURE: Signature;
    fn address(self) -> *const ();
}

macro_rules! count {
    () => { 0 };
    ($x:ident $($rest:ident)*) => { 1 + count!($($rest)*) };
}

macro_rules! natives_taking {
    ($($a:ident)*) => {
        impl<R: Answered, $($a: Taken),*> Native for unsafe extern "C" fn(*mut Ctx $(, $a)*) -> R {
            const SIGNATURE: Signature = Signature {
                words: count!($($a)*),
                answers: R::ANSWERS,
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
