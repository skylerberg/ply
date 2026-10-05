//! The effect model: the atoms a row is made of, and the closed row a definition publishes.

use crate::Symbol;
use std::collections::{BTreeMap, BTreeSet};
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
    /// appears in its footprint.
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

/// The name a label variable on its own prints under: `l`, `m`, `n`, then a round (`l1`). Among
/// other atoms it is [`atom_texts`] that names it, against what else is there.
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

/// Each atom's text, a label variable named with the first letter, then round, that no resource
/// among the atoms and no other label holds, so `[l]` means one thing among them.
pub fn atom_texts(atoms: &BTreeSet<EffectAtom>) -> Vec<String> {
    let taken: BTreeSet<&str> = atoms
        .iter()
        .filter_map(|a| match &a.resource {
            Resource::Named(name) => Some(name.as_str()),
            _ => None,
        })
        .collect();
    let mut labels: BTreeMap<u32, String> = BTreeMap::new();
    atoms
        .iter()
        .map(|a| match a.resource {
            Resource::Var(v) => {
                let name = match labels.get(&v) {
                    Some(name) => name.clone(),
                    None => {
                        let name = (0..)
                            .map(label_var_name)
                            .find(|n| {
                                !taken.contains(n.as_str())
                                    && !labels.values().any(|held| held == n)
                            })
                            .expect("the letters and their rounds do not run out");
                        labels.insert(v, name.clone());
                        name
                    }
                };
                a.text_with(&format!("[{name}]"))
            }
            _ => a.text_with(&a.resource.to_string()),
        })
        .collect()
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

    /// The mode atom of this atom's effect and resource.
    pub fn mode_atom(&self) -> EffectAtom {
        EffectAtom::new(self.effect.clone(), self.resource.clone(), self.mode)
    }

    pub fn conflicts_with(&self, other: &EffectAtom) -> bool {
        self.effect == other.effect
            && self.resource == other.resource
            && (self.mode == Mode::Write || other.mode == Mode::Write)
    }

    /// Whether performing `other` is within what this atom permits: a mode atom covers every
    /// operation of its mode, and an operation atom covers itself.
    pub fn covers(&self, other: &EffectAtom) -> bool {
        self.effect == other.effect
            && self.resource == other.resource
            && (self == other || (self.op.is_none() && self.mode == other.mode))
    }

    /// An operation atom with the mode its declaration gives it; a mode atom is unchanged.
    pub fn with_declared_mode(
        mut self,
        mode_of: &dyn Fn(&Symbol, &Symbol) -> Option<Mode>,
    ) -> Self {
        if let Some(mode) = self.op.as_ref().and_then(|op| mode_of(&self.effect, op)) {
            self.mode = mode;
        }
        self
    }

    /// The atom with its resource already written: a label variable's name depends on the text it
    /// is printed in, which [`fmt::Display`] cannot see and [`atom_texts`] can.
    pub fn text_with(&self, resource: &str) -> String {
        match &self.op {
            Some(op) => format!("{}.{}{resource}", self.effect, op),
            None => format!("{}.{}{resource}", self.effect, self.mode.as_str()),
        }
    }
}

impl fmt::Display for EffectAtom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text_with(&self.resource.to_string()))
    }
}

/// A closed row: exactly what a definition can do.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub struct Footprint(pub BTreeSet<EffectAtom>);

impl Footprint {
    pub fn empty() -> Self {
        Footprint(BTreeSet::new())
    }

    pub fn from_atoms(atoms: impl IntoIterator<Item = EffectAtom>) -> Self {
        Footprint(atoms.into_iter().collect())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn atoms(&self) -> impl Iterator<Item = &EffectAtom> {
        self.0.iter()
    }

    pub fn contains(&self, atom: &EffectAtom) -> bool {
        self.0.contains(atom)
    }

    /// Whether some atom here covers `atom` (see [`EffectAtom::covers`]).
    pub fn covers(&self, atom: &EffectAtom) -> bool {
        self.0.iter().any(|a| a.covers(atom))
    }

    pub fn resolve_modes(&mut self, mode_of: &dyn Fn(&Symbol, &Symbol) -> Option<Mode>) {
        self.0 = std::mem::take(&mut self.0)
            .into_iter()
            .map(|a| a.with_declared_mode(mode_of))
            .collect();
    }

    pub fn union(&self, other: &Footprint) -> Footprint {
        Footprint(self.0.union(&other.0).cloned().collect())
    }

    pub fn conflicts_with(&self, other: &Footprint) -> bool {
        self.0
            .iter()
            .any(|a| other.0.iter().any(|b| a.conflicts_with(b)))
    }
}

impl fmt::Display for Footprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{{{}}}", atom_texts(&self.0).join(", "))
    }
}
