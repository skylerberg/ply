//! Pinned: concurrent crates are written against these shapes.

use ply_span::Symbol;
use ply_syntax::ast::Mode;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct TyVar(pub u32);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct RowVar(pub u32);

/// The resource an atom touches. Operations declared without `[r]` collapse to
/// `Singleton`, which conflicts with itself and nothing else.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub enum Resource {
    Named(Symbol),
    Singleton,
}

impl fmt::Display for Resource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Resource::Named(s) => write!(f, "[{s}]"),
            Resource::Singleton => Ok(()),
        }
    }
}

/// Ordering is structural so rows are canonical, which content addressing
/// depends on.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct EffectAtom {
    pub effect: Symbol,
    pub resource: Resource,
    pub mode: Mode,
}

impl EffectAtom {
    pub fn new(effect: impl Into<Symbol>, resource: Resource, mode: Mode) -> Self {
        EffectAtom {
            effect: effect.into(),
            resource,
            mode,
        }
    }

    /// Two atoms contend iff they name the same resource of the same effect and
    /// at least one writes. This is the whole basis of test scheduling.
    pub fn conflicts_with(&self, other: &EffectAtom) -> bool {
        self.effect == other.effect
            && self.resource == other.resource
            && (self.mode == Mode::Write || other.mode == Mode::Write)
    }
}

impl fmt::Display for EffectAtom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}{}", self.effect, self.mode.as_str(), self.resource)
    }
}

/// A set of atoms plus an optional tail variable. Because atoms are ground
/// labels, unification is set unification rather than general row polymorphism.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct Row {
    pub atoms: BTreeSet<EffectAtom>,
    pub tail: Option<RowVar>,
}

impl Row {
    pub fn empty() -> Self {
        Row::default()
    }

    pub fn open(tail: RowVar) -> Self {
        Row {
            atoms: BTreeSet::new(),
            tail: Some(tail),
        }
    }

    pub fn closed(atoms: impl IntoIterator<Item = EffectAtom>) -> Self {
        Row {
            atoms: atoms.into_iter().collect(),
            tail: None,
        }
    }

    pub fn singleton(atom: EffectAtom) -> Self {
        Row::closed([atom])
    }

    pub fn is_closed(&self) -> bool {
        self.tail.is_none()
    }

    pub fn is_pure(&self) -> bool {
        self.atoms.is_empty() && self.tail.is_none()
    }

    /// Row union. A tail on either side is kept; two distinct tails cannot both
    /// be preserved, so callers must unify them first.
    pub fn union(&self, other: &Row) -> Row {
        Row {
            atoms: self.atoms.union(&other.atoms).cloned().collect(),
            tail: self.tail.or(other.tail),
        }
    }

    pub fn without(&self, removed: &BTreeSet<EffectAtom>) -> Row {
        Row {
            atoms: self.atoms.difference(removed).cloned().collect(),
            tail: self.tail,
        }
    }

    pub fn contains(&self, atom: &EffectAtom) -> bool {
        self.atoms.contains(atom)
    }

    /// Discards the tail. Only valid once inference has closed the row.
    pub fn to_footprint(&self) -> Footprint {
        Footprint(self.atoms.clone())
    }
}

impl fmt::Display for Row {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let atoms: Vec<String> = self.atoms.iter().map(|a| a.to_string()).collect();
        match self.tail {
            None => write!(f, "{{{}}}", atoms.join(", ")),
            Some(RowVar(v)) if atoms.is_empty() => write!(f, "{{| e{v}}}"),
            Some(RowVar(v)) => write!(f, "{{{} | e{v}}}", atoms.join(", ")),
        }
    }
}

/// A closed row: exactly what a definition can do. Scheduling and the
/// determinism check operate on these.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Default, Serialize, Deserialize)]
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

    pub fn union(&self, other: &Footprint) -> Footprint {
        Footprint(self.0.union(&other.0).cloned().collect())
    }

    pub fn conflicts_with(&self, other: &Footprint) -> bool {
        // Both sides are sorted by (effect, resource, mode), so a merge walk
        // would beat this; the sets are small enough that it has not mattered.
        self.0
            .iter()
            .any(|a| other.0.iter().any(|b| a.conflicts_with(b)))
    }

    pub fn effects(&self) -> BTreeSet<&Symbol> {
        self.0.iter().map(|a| &a.effect).collect()
    }
}

impl fmt::Display for Footprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let atoms: Vec<String> = self.0.iter().map(|a| a.to_string()).collect();
        write!(f, "{{{}}}", atoms.join(", "))
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Type {
    Var(TyVar),
    Con(Symbol, Vec<Type>),
    Fn {
        params: Vec<Type>,
        ret: Box<Type>,
        effects: Row,
    },
    Record(BTreeMap<Symbol, Type>),
}

impl Type {
    pub fn con(name: &str) -> Type {
        Type::Con(Symbol::new(name), Vec::new())
    }
    pub fn int() -> Type {
        Type::con("Int")
    }
    pub fn bool() -> Type {
        Type::con("Bool")
    }
    pub fn string() -> Type {
        Type::con("String")
    }
    pub fn bytes() -> Type {
        Type::con("Bytes")
    }
    /// IEEE-754 binary64. Deliberately distinct from [`Type::decimal`]: `==` on
    /// this type is not reflexive, so nothing about it may be `proved`.
    pub fn float() -> Type {
        Type::con("Float")
    }
    /// Exact base-10, sign plus a 96-bit mantissa and a scale of `0..=28`. What
    /// money is written in, and the only numeric type whose `==` is an
    /// equivalence relation over its whole domain besides `Int`.
    pub fn decimal() -> Type {
        Type::con("Decimal")
    }
    pub fn unit() -> Type {
        Type::con("Unit")
    }
    pub fn list(t: Type) -> Type {
        Type::Con(Symbol::new("List"), vec![t])
    }
    /// Iteration is ascending by key, always. The key type must be ordered —
    /// `derivable(ord, k)` — which is what makes that order a function of the
    /// value rather than of insertion history or a hasher's seed.
    pub fn map(key: Type, value: Type) -> Type {
        Type::Con(Symbol::new("Map"), vec![key, value])
    }
    pub fn option(t: Type) -> Type {
        Type::Con(Symbol::new("Option"), vec![t])
    }
    pub fn result(ok: Type, err: Type) -> Type {
        Type::Con(Symbol::new("Result"), vec![ok, err])
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Type::Var(TyVar(v)) => write!(f, "t{v}"),
            Type::Con(name, args) if args.is_empty() => write!(f, "{name}"),
            Type::Con(name, args) => {
                let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
                write!(f, "{name}<{}>", args.join(", "))
            }
            Type::Fn {
                params,
                ret,
                effects,
            } => {
                let ps: Vec<String> = params.iter().map(|p| p.to_string()).collect();
                write!(f, "({}) -> {ret}", ps.join(", "))?;
                if !effects.is_pure() {
                    write!(f, " / {effects}")?;
                }
                Ok(())
            }
            Type::Record(fields) => {
                let fs: Vec<String> = fields.iter().map(|(k, v)| format!("{k}: {v}")).collect();
                write!(f, "{{{}}}", fs.join(", "))
            }
        }
    }
}

/// Row variables generalize alongside type variables.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Scheme {
    pub ty_vars: Vec<TyVar>,
    pub row_vars: Vec<RowVar>,
    pub ty: Type,
}

impl Scheme {
    pub fn mono(ty: Type) -> Self {
        Scheme {
            ty_vars: Vec::new(),
            row_vars: Vec::new(),
            ty,
        }
    }
}

impl fmt::Display for Scheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.ty)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn atom(effect: &str, resource: Option<&str>, mode: Mode) -> EffectAtom {
        EffectAtom::new(
            effect,
            resource
                .map(|r| Resource::Named(Symbol::new(r)))
                .unwrap_or(Resource::Singleton),
            mode,
        )
    }

    #[test]
    fn reads_of_the_same_resource_do_not_conflict() {
        let a = Footprint::from_atoms([atom("db", Some("users"), Mode::Read)]);
        let b = Footprint::from_atoms([atom("db", Some("users"), Mode::Read)]);
        assert!(!a.conflicts_with(&b));
    }

    #[test]
    fn a_write_conflicts_with_a_read_of_the_same_resource() {
        let r = Footprint::from_atoms([atom("db", Some("users"), Mode::Read)]);
        let w = Footprint::from_atoms([atom("db", Some("users"), Mode::Write)]);
        assert!(r.conflicts_with(&w));
        assert!(w.conflicts_with(&r));
    }

    #[test]
    fn writes_to_distinct_resources_do_not_conflict() {
        let a = Footprint::from_atoms([atom("db", Some("users"), Mode::Write)]);
        let b = Footprint::from_atoms([atom("db", Some("orders"), Mode::Write)]);
        assert!(!a.conflicts_with(&b));
    }

    #[test]
    fn same_resource_name_under_different_effects_does_not_conflict() {
        let a = Footprint::from_atoms([atom("db", Some("users"), Mode::Write)]);
        let b = Footprint::from_atoms([atom("cache", Some("users"), Mode::Write)]);
        assert!(!a.conflicts_with(&b));
    }

    #[test]
    fn singleton_resources_conflict_only_within_their_effect() {
        let a = Footprint::from_atoms([atom("clock", None, Mode::Write)]);
        let b = Footprint::from_atoms([atom("clock", None, Mode::Read)]);
        let c = Footprint::from_atoms([atom("random", None, Mode::Write)]);
        assert!(a.conflicts_with(&b));
        assert!(!a.conflicts_with(&c));
    }

    #[test]
    fn the_empty_footprint_conflicts_with_nothing() {
        let e = Footprint::empty();
        let w = Footprint::from_atoms([atom("db", Some("users"), Mode::Write)]);
        assert!(!e.conflicts_with(&w));
        assert!(!w.conflicts_with(&e));
        assert!(!e.conflicts_with(&e));
    }

    #[test]
    fn row_display_round_trips_the_surface_syntax() {
        let r = Row::closed([atom("db", Some("users"), Mode::Read)]);
        assert_eq!(r.to_string(), "{db.read[users]}");
        let open = Row {
            atoms: r.atoms.clone(),
            tail: Some(RowVar(3)),
        };
        assert_eq!(open.to_string(), "{db.read[users] | e3}");
        assert_eq!(Row::empty().to_string(), "{}");
    }

    #[test]
    fn without_removes_handled_atoms_and_keeps_the_tail() {
        let read = atom("db", Some("users"), Mode::Read);
        let write = atom("db", Some("users"), Mode::Write);
        let row = Row {
            atoms: [read.clone(), write.clone()].into(),
            tail: Some(RowVar(1)),
        };
        let handled: BTreeSet<_> = [read].into();
        let out = row.without(&handled);
        assert_eq!(out.atoms, [write].into());
        assert_eq!(out.tail, Some(RowVar(1)));
    }
}
