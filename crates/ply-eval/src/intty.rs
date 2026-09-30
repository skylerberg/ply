//! Fixed-width integers, with the arithmetic the value model checks them with.

use serde::{Deserialize, Serialize};
use std::fmt;

/// A fixed-width integer type; `Int` is not one. Arithmetic is checked unless `wrap_*` is used.
/// The 128-bit widths come last, so each narrower width keeps its number in every format.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub enum IntTy {
    U8,
    U16,
    U32,
    U64,
    I8,
    I16,
    I32,
    I64,
    U128,
    I128,
}

pub const INT_TYPES: [IntTy; 10] = [
    IntTy::U8,
    IntTy::U16,
    IntTy::U32,
    IntTy::U64,
    IntTy::I8,
    IntTy::I16,
    IntTy::I32,
    IntTy::I64,
    IntTy::U128,
    IntTy::I128,
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
            IntTy::U128 => "U128",
            IntTy::I128 => "I128",
        }
    }

    pub fn from_name(name: &str) -> Option<IntTy> {
        INT_TYPES.into_iter().find(|t| t.name() == name)
    }

    /// `u32_of_int`, and its siblings.
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
            IntTy::U128 => "u128_of_int",
            IntTy::I128 => "i128_of_int",
        }
    }

    /// `int_of_u32`, and its siblings.
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
            IntTy::U128 => "int_of_u128",
            IntTy::I128 => "int_of_i128",
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
            IntTy::U128 | IntTy::I128 => 128,
        }
    }

    pub fn signed(self) -> bool {
        matches!(
            self,
            IntTy::I8 | IntTy::I16 | IntTy::I32 | IntTy::I64 | IntTy::I128
        )
    }

    /// The largest value.
    pub fn max(self) -> u128 {
        if self.signed() {
            (1u128 << (self.bits() - 1)) - 1
        } else {
            u128::MAX >> (128 - self.bits())
        }
    }

    /// The smallest value.
    pub fn min(self) -> i128 {
        if self.signed() {
            // `i128::MIN` for `I128`, where the shift below would overflow.
            i128::MIN >> (128 - self.bits())
        } else {
            0
        }
    }

    /// Whether `v` is one of this type's values.
    pub fn holds(self, v: i128) -> bool {
        v >= self.min() && (v < 0 || v as u128 <= self.max())
    }

    /// Truncated to this width, then zero- or sign-extended; every `Fixed` is in this form.
    pub fn normalize(self, bits: u128) -> u128 {
        let w = self.bits();
        if w == 128 {
            return bits;
        }
        let low = bits & (u128::MAX >> (128 - w));
        if self.signed() && low >> (w - 1) == 1 {
            low | (u128::MAX << w)
        } else {
            low
        }
    }
}

impl fmt::Display for IntTy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}
