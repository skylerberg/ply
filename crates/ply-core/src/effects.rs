//! The effect model: the atoms a row is made of.

use crate::Symbol;
use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Mode {
    Read,
    Write,
    /// An operation that does not come back; no mode atom stands for it.
    Raise,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Read => "read",
            Mode::Write => "write",
            Mode::Raise => "raise",
        }
    }
}

/// The resource an atom touches; the variant order is the atom order the compiler sorts rows by.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Resource {
    Named(Symbol),
    /// A label the definition is generic over, filled at each call, numbered by where it first
    /// appears in its effects.
    Var(u32),
    Singleton,
    /// Every label: what an atom written `op[*]` stands for, so a row can say which atoms it
    /// consumes without naming a label.
    Every,
}

impl fmt::Display for Resource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Resource::Named(s) => write!(f, "[{s}]"),
            Resource::Var(v) => write!(f, "[{}]", label_var_name(*v)),
            Resource::Singleton => Ok(()),
            Resource::Every => write!(f, "[*]"),
        }
    }
}

const LABEL_LETTERS: &[u8] = b"lmn";

/// The name a label variable prints under: `l`, `m`, `n`, then a round (`l1`).
pub fn label_var_name(v: u32) -> String {
    let i = v as usize;
    let c = char::from(LABEL_LETTERS[i % LABEL_LETTERS.len()]);
    let round = i / LABEL_LETTERS.len();
    if round == 0 {
        c.to_string()
    } else {
        format!("{c}{round}")
    }
}

/// Ordering is structural so rows are canonical, which content addressing depends on.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct EffectAtom {
    pub effect: Symbol,
    pub resource: Resource,
    pub mode: Mode,
    /// `Some` names one operation, which the mode atom of the same effect and resource covers.
    pub op: Option<Symbol>,
}

impl EffectAtom {
    pub fn new(effect: impl Into<Symbol>, resource: Resource, mode: Mode) -> Self {
        EffectAtom {
            effect: effect.into(),
            resource,
            mode,
            op: None,
        }
    }

    /// `mode` is the operation's declared mode.
    pub fn operation(
        effect: impl Into<Symbol>,
        resource: Resource,
        mode: Mode,
        op: impl Into<Symbol>,
    ) -> Self {
        EffectAtom {
            effect: effect.into(),
            resource,
            mode,
            op: Some(op.into()),
        }
    }
}

impl fmt::Display for EffectAtom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.op {
            Some(op) => write!(f, "{}.{}{}", self.effect, op, self.resource),
            None => write!(f, "{}.{}{}", self.effect, self.mode.as_str(), self.resource),
        }
    }
}
