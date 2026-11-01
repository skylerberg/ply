use crate::Symbol;
use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Mode {
    Read,
    Write,
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

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Resource {
    Named(Symbol),
    Var(u32),
    Singleton,
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

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Effect {
    pub effect: Symbol,
    pub resource: Resource,
    pub mode: Mode,
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
