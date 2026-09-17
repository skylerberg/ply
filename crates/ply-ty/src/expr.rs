//! The expression vocabulary a lowered body still speaks after the tree is gone: operators and
//! literals, shared by the syntax tree, the checker, the evaluator and the code generator.

use crate::IntTy;

#[derive(Clone, Debug, PartialEq)]
pub enum Lit {
    Int(i64),
    /// `255u8`, `0x6A09_E667u32`. A suffixed literal is that type and nothing else, exactly as a
    /// `Decimal` literal is written `1m` — Ply has no numeric tower and no implicit widening, so a
    /// literal cannot be a value of two types and the spelling is what says which.
    ///
    /// `bits` is the value as the type reads it, normalized by [`IntTy::normalize`], so the lexer
    /// has already refused anything the type does not hold.
    Fixed {
        ty: IntTy,
        bits: u64,
    },
    Bool(bool),
    Str(String),
    /// `b"GET "`.
    Bytes(Vec<u8>),
    /// IEEE-754 binary64.
    Float(f64),
    /// Sign and magnitude in `mantissa`, digits after the point in `scale`.
    Decimal {
        mantissa: i128,
        scale: u32,
    },
    Unit,
}

/// The shortest text that reads back as this `f64`, always distinguishable from an integer.
pub fn render_float(f: f64) -> String {
    if f.is_nan() {
        return "NaN".to_string();
    }
    if f.is_infinite() {
        return if f > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    // Rust's `{}` is shortest-round-tripping in *digits* but always positional, so `1e300` comes
    // back as three hundred and one characters.
    let positional = format!("{f}");
    let exponential = format!("{f:e}");
    let text = if exponential.len() < positional.len() {
        exponential
    } else {
        positional
    };
    if text.contains(['.', 'e', 'E']) {
        text
    } else {
        format!("{text}.0")
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    Concat,
    /// The bit operators, defined at
    /// `Int` only. `Shl` discards what it shifts out — the one deliberate
    /// exception to checked arithmetic — and a shift count outside `0..=63`
    /// raises. Appended rather than filed beside `And`/`Or` so that
    /// `ply_hash::normalize::binop_byte` can append its bytes too: an existing
    /// byte that moves is every cached result invalidated.
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    Ushr,
}

impl BinOp {
    /// The operator as it is written, for a diagnostic that quotes it.
    pub fn text(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Rem => "%",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::And => "&&",
            BinOp::Or => "||",
            BinOp::Concat => "++",
            BinOp::BitAnd => "&",
            BinOp::BitOr => "|",
            BinOp::BitXor => "^",
            BinOp::Shl => "<<",
            BinOp::Shr => ">>",
            BinOp::Ushr => ">>>",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UnOp {
    Neg,
    Not,
    /// Prefix `~`: the two's-complement complement of an `Int`. `!` stays
    /// `Bool`-only, so neither operator can be written where the other is meant
    /// (the shift semantics).
    BitNot,
}
