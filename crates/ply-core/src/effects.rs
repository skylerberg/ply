//! The effect model: what a function's effects are made of.

use crate::Symbol;
use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Mode {
    Read,
    Write,
    /// An operation that does not come back; no effect of this mode stands for it.
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

/// The resource an effect touches; the variant order is the order the compiler sorts effects by.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Resource {
    Named(Symbol),
    /// A label the definition is generic over, filled at each call, numbered by where it first
    /// appears in its effects.
    Var(u32),
    Singleton,
    /// Every label: what an effect written `op[*]` stands for, so a list can say which effects it
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

/// Ordering is structural so effects are canonical, which content addressing depends on.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Effect {
    pub effect: Symbol,
    pub resource: Resource,
    pub mode: Mode,
    /// `Some` names one operation, which the effect of the same name, resource and mode covers.
    pub op: Option<Symbol>,
}

impl Effect {
    pub fn new(effect: impl Into<Symbol>, resource: Resource, mode: Mode) -> Self {
        Effect {
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
        Effect {
            effect: effect.into(),
            resource,
            mode,
            op: Some(op.into()),
        }
    }
}

impl fmt::Display for Effect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.op {
            Some(op) => write!(f, "{}.{}{}", self.effect, op, self.resource),
            None => write!(f, "{}.{}{}", self.effect, self.mode.as_str(), self.resource),
        }
    }
}
