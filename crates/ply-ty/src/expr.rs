//! Operators and literals, shared by the syntax tree, checker, evaluator and code generator.

use crate::IntTy;

#[derive(Clone, Debug, PartialEq)]
pub enum Lit {
    Int(i64),
    /// A suffixed literal such as `255u8`; `bits` is normalized by [`IntTy::normalize`].
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
    // `{}` is always positional, so `1e300` would print every digit.
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
    /// `Int` only. `Shl` discards what it shifts out; a shift count outside `0..=63` raises.
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
    /// Prefix `~` on an `Int`; `!` stays `Bool`-only.
    BitNot,
}
