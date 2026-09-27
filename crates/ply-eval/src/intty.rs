//! Fixed-width integers, with the arithmetic the value model checks them with.

use std::fmt;

/// A fixed-width integer type; `Int` is not one. Arithmetic is checked unless `wrap_*` is used.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum IntTy {
    U8,
    U16,
    U32,
    U64,
    I8,
    I16,
    I32,
    I64,
}

pub const INT_TYPES: [IntTy; 8] = [
    IntTy::U8,
    IntTy::U16,
    IntTy::U32,
    IntTy::U64,
    IntTy::I8,
    IntTy::I16,
    IntTy::I32,
    IntTy::I64,
];

impl IntTy {
    pub fn name(self) -> &'static str {
        match self {
            IntTy::U8 => "U8",
            IntTy::U16 => "U16",
            IntTy::U32 => "U32",
            IntTy::U64 => "U64",
            IntTy::I8 => "I8",
            IntTy::I16 => "I16",
            IntTy::I32 => "I32",
            IntTy::I64 => "I64",
        }
    }

    pub fn from_name(name: &str) -> Option<IntTy> {
        INT_TYPES.into_iter().find(|t| t.name() == name)
    }

    /// `u32_of_int`, and its seven siblings.
    pub fn of_int_name(self) -> &'static str {
        match self {
            IntTy::U8 => "u8_of_int",
            IntTy::U16 => "u16_of_int",
            IntTy::U32 => "u32_of_int",
            IntTy::U64 => "u64_of_int",
            IntTy::I8 => "i8_of_int",
            IntTy::I16 => "i16_of_int",
            IntTy::I32 => "i32_of_int",
            IntTy::I64 => "i64_of_int",
        }
    }

    /// `int_of_u32`, and its seven siblings.
    pub fn to_int_name(self) -> &'static str {
        match self {
            IntTy::U8 => "int_of_u8",
            IntTy::U16 => "int_of_u16",
            IntTy::U32 => "int_of_u32",
            IntTy::U64 => "int_of_u64",
            IntTy::I8 => "int_of_i8",
            IntTy::I16 => "int_of_i16",
            IntTy::I32 => "int_of_i32",
            IntTy::I64 => "int_of_i64",
        }
    }

    pub fn of_int_from_name(name: &str) -> Option<IntTy> {
        INT_TYPES.into_iter().find(|t| t.of_int_name() == name)
    }

    pub fn to_int_from_name(name: &str) -> Option<IntTy> {
        INT_TYPES.into_iter().find(|t| t.to_int_name() == name)
    }

    pub fn bits(self) -> u32 {
        match self {
            IntTy::U8 | IntTy::I8 => 8,
            IntTy::U16 | IntTy::I16 => 16,
            IntTy::U32 | IntTy::I32 => 32,
            IntTy::U64 | IntTy::I64 => 64,
        }
    }

    pub fn signed(self) -> bool {
        matches!(self, IntTy::I8 | IntTy::I16 | IntTy::I32 | IntTy::I64)
    }

    /// The largest value, as the `u64` the representation carries.
    pub fn max(self) -> u64 {
        if self.signed() {
            (1u64 << (self.bits() - 1)) - 1
        } else {
            u64::MAX >> (64 - self.bits())
        }
    }

    /// The smallest value, as an `i128`.
    pub fn min(self) -> i128 {
        if self.signed() {
            -(1i128 << (self.bits() - 1))
        } else {
            0
        }
    }

    /// Whether `v` is one of this type's values.
    pub fn holds(self, v: i128) -> bool {
        v >= self.min() && v <= self.max() as i128
    }

    /// Truncated to this width, then zero- or sign-extended; every `Fixed` is in this form.
    pub fn normalize(self, bits: u64) -> u64 {
        let w = self.bits();
        if w == 64 {
            return bits;
        }
        let low = bits & (u64::MAX >> (64 - w));
        if self.signed() && low >> (w - 1) == 1 {
            low | (u64::MAX << w)
        } else {
            low
        }
    }

    /// The mathematical value the bits stand for.
    pub fn value(self, bits: u64) -> i128 {
        if self.signed() {
            (bits as i64) as i128
        } else {
            bits as i128
        }
    }
}

impl fmt::Display for IntTy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}
