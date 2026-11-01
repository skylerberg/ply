use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum IntegerType {
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

pub const INT_TYPES: [IntegerType; 10] = [
    IntegerType::U8,
    IntegerType::U16,
    IntegerType::U32,
    IntegerType::U64,
    IntegerType::I8,
    IntegerType::I16,
    IntegerType::I32,
    IntegerType::I64,
    IntegerType::U128,
    IntegerType::I128,
];

impl IntegerType {
    pub fn name(self) -> &'static str {
        match self {
            IntegerType::U8 => "U8",
            IntegerType::U16 => "U16",
            IntegerType::U32 => "U32",
            IntegerType::U64 => "U64",
            IntegerType::I8 => "I8",
            IntegerType::I16 => "I16",
            IntegerType::I32 => "I32",
            IntegerType::I64 => "I64",
            IntegerType::U128 => "U128",
            IntegerType::I128 => "I128",
        }
    }

    pub fn from_name(name: &str) -> Option<IntegerType> {
        INT_TYPES.into_iter().find(|t| t.name() == name)
    }

    pub fn bits(self) -> u32 {
        match self {
            IntegerType::U8 | IntegerType::I8 => 8,
            IntegerType::U16 | IntegerType::I16 => 16,
            IntegerType::U32 | IntegerType::I32 => 32,
            IntegerType::U64 | IntegerType::I64 => 64,
            IntegerType::U128 | IntegerType::I128 => 128,
        }
    }

    pub fn signed(self) -> bool {
        matches!(
            self,
            IntegerType::I8
                | IntegerType::I16
                | IntegerType::I32
                | IntegerType::I64
                | IntegerType::I128
        )
    }

    pub fn max(self) -> u128 {
        if self.signed() {
            (1u128 << (self.bits() - 1)) - 1
        } else {
            u128::MAX >> (128 - self.bits())
        }
    }

    pub fn min(self) -> i128 {
        if self.signed() {
            i128::MIN >> (128 - self.bits())
        } else {
            0
        }
    }

    pub fn holds(self, v: i128) -> bool {
        v >= self.min() && (v < 0 || v as u128 <= self.max())
    }

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

impl fmt::Display for IntegerType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Fixed {
    pub ty: IntegerType,
    bits: u128,
}

impl Fixed {
    pub fn new(ty: IntegerType, bits: u128) -> Fixed {
        Fixed {
            ty,
            bits: ty.normalize(bits),
        }
    }

    pub fn bits(self) -> u128 {
        self.bits
    }

    pub fn to_i128(self) -> Option<i128> {
        if self.ty.signed() {
            Some(self.bits as i128)
        } else {
            i128::try_from(self.bits).ok()
        }
    }

    pub fn raw(self) -> u128 {
        if self.ty.bits() == 128 {
            self.bits
        } else {
            self.bits & (u128::MAX >> (128 - self.ty.bits()))
        }
    }
}
