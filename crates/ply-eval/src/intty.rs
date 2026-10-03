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
