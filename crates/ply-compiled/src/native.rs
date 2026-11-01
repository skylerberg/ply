use crate::heap::Word;
use crate::rt::Context;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MachineType {
    Word,
    Words,
    I64,
    F64,
    Address,
    Out,
    Handle,
    Handed,
    Block,
    Type,
    Op,
    DropRoot,
    KeepRoot,
    Taken,
    Secret,
}

impl MachineType {
    pub fn name(self) -> &'static str {
        match self {
            MachineType::Word => "Word",
            MachineType::Words => "Words",
            MachineType::I64 => "I64",
            MachineType::F64 => "F64",
            MachineType::Address => "Addr",
            MachineType::Out => "Out",
            MachineType::Handle => "Handle",
            MachineType::Handed => "Handed",
            MachineType::Block => "Block",
            MachineType::Type => "Type",
            MachineType::Op => "Op",
            MachineType::DropRoot => "DropRoot",
            MachineType::KeepRoot => "KeepRoot",
            MachineType::Taken => "Taken",
            MachineType::Secret => "Secret",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Signature {
    pub params: &'static [MachineType],
    pub result: Option<MachineType>,
}

impl Signature {
    pub fn words(&self) -> usize {
        self.params.len()
    }

    pub fn results(&self) -> bool {
        self.result.is_some()
    }
}

#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct I64(pub i64);

#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct F64(pub u64);

#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Address(pub *mut u8);

#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TakenBytes(pub i64);

#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Out(pub *mut i64);

pub unsafe trait Taken {
    const MACHINE: MachineType;
}

unsafe impl Taken for Word {
    const MACHINE: MachineType = MachineType::Word;
}

unsafe impl Taken for *const Word {
    const MACHINE: MachineType = MachineType::Words;
}

unsafe impl Taken for I64 {
    const MACHINE: MachineType = MachineType::I64;
}

unsafe impl Taken for F64 {
    const MACHINE: MachineType = MachineType::F64;
}

unsafe impl Taken for Address {
    const MACHINE: MachineType = MachineType::Address;
}

unsafe impl Taken for Out {
    const MACHINE: MachineType = MachineType::Out;
}

pub unsafe trait Returned {
    const MACHINE: Option<MachineType>;
    fn of(w: Word) -> Self;
}

unsafe impl Returned for () {
    const MACHINE: Option<MachineType> = None;
    fn of(_: Word) -> Self {}
}

unsafe impl Returned for Word {
    const MACHINE: Option<MachineType> = Some(MachineType::Word);
    fn of(w: Word) -> Self {
        w
    }
}

unsafe impl Returned for I64 {
    const MACHINE: Option<MachineType> = Some(MachineType::I64);
    fn of(w: Word) -> Self {
        I64(w)
    }
}

unsafe impl Returned for F64 {
    const MACHINE: Option<MachineType> = Some(MachineType::F64);
    fn of(w: Word) -> Self {
        F64(w as u64)
    }
}

unsafe impl Returned for TakenBytes {
    const MACHINE: Option<MachineType> = Some(MachineType::Taken);
    fn of(w: Word) -> Self {
        TakenBytes(w)
    }
}

pub trait Native: Copy {
    const SIGNATURE: Signature;
    fn address(self) -> *const ();
}

macro_rules! natives_taking {
    ($($a:ident)*) => {
        impl<R: Returned, $($a: Taken),*> Native for unsafe extern "C" fn(*mut Context $(, $a)*) -> R {
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
