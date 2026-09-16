//! Pinned: concurrent crates are written against these shapes.

use ply_span::Symbol;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Read,
    Write,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Read => "read",
            Mode::Write => "write",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct TyVar(pub u32);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct RowVar(pub u32);

/// The resource an atom touches.
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

/// Ordering is structural so rows are canonical, which content addressing depends on.
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

    /// Two atoms contend iff they name the same resource of the same effect and at least one
    /// writes.
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

/// A set of atoms plus an optional tail variable.
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

    /// Discards the tail.
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

/// A closed row: exactly what a definition can do.
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
        // Both sides are sorted by (effect, resource, mode), so a merge walk would beat this; the
        // sets are small enough that it has not mattered.
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
    /// IEEE-754 binary64.
    pub fn float() -> Type {
        Type::con("Float")
    }
    /// Exact base-10, sign plus a 96-bit mantissa and a scale of `0..=28`.
    pub fn decimal() -> Type {
        Type::con("Decimal")
    }
    pub fn unit() -> Type {
        Type::con("Unit")
    }
    pub fn list(t: Type) -> Type {
        Type::Con(Symbol::new("List"), vec![t])
    }
    /// Iteration is ascending by key, always.
    pub fn map(key: Type, value: Type) -> Type {
        Type::Con(Symbol::new("Map"), vec![key, value])
    }
    pub fn option(t: Type) -> Type {
        Type::Con(Symbol::new("Option"), vec![t])
    }
    pub fn result(ok: Type, err: Type) -> Type {
        Type::Con(Symbol::new("Result"), vec![ok, err])
    }
    pub fn iter(seed: Type, stop: Type) -> Type {
        Type::Con(Symbol::new("Iter"), vec![seed, stop])
    }
    /// A credential.
    pub fn secret(inner: Type) -> Type {
        Type::Con(Symbol::new(SECRET), vec![inner])
    }

    /// Whether a solved type mentions a `Secret` anywhere.
    pub fn mentions_secret(&self) -> bool {
        match self {
            Type::Con(name, args) => {
                name.as_str() == SECRET || args.iter().any(Type::mentions_secret)
            }
            Type::Fn { params, ret, .. } => {
                params.iter().any(Type::mentions_secret) || ret.mentions_secret()
            }
            Type::Record(fields) => fields.values().any(Type::mentions_secret),
            Type::Var(_) => false,
        }
    }
}

/// The builtin type constructor the secret containment claim is about.
pub const SECRET: &str = "Secret";

/// `Some(n)` when a record's `len` fields are exactly `_0` to `_{n-1}` — the record a tuple is
/// sugar for (GUIDE §5.3) — so it can be shown as the tuple it was written as. Two or more make
/// a tuple, so a lone `_0` is a record.
pub fn tuple_arity(len: usize, has: impl Fn(&Symbol) -> bool) -> Option<usize> {
    (len >= 2 && (0..len).all(|i| has(&Symbol::new(format!("_{i}"))))).then_some(len)
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
                if let Some(n) = tuple_arity(fields.len(), |k| fields.contains_key(k)) {
                    let ts: Vec<String> = (0..n)
                        .map(|i| fields[&Symbol::new(format!("_{i}"))].to_string())
                        .collect();
                    return write!(f, "({})", ts.join(", "));
                }
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

/// A fixed-width integer type: eight of them, the widths the machine has.
///
/// `Int` is not one of these and is not a member of the family. It is the type a program counts
/// and indexes with, and it stays exactly what it was — sixty-four bits, signed, checked. These
/// exist for the algorithms that are *written* in a width: a hash's thirty-two-bit word, a byte,
/// a wire format's field. Arithmetic on them is checked as `Int`'s is; `wrap_add` and its siblings
/// are how a program says it meant the wrap.
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
}

/// Every fixed-width integer type, in the order the guide lists them.
pub const INT_TYPES: [IntTy; 8] = [
    IntTy::U8,
    IntTy::U16,
    IntTy::U32,
    IntTy::U64,
    IntTy::I8,
    IntTy::I16,
    IntTy::I32,
    IntTy::I64,
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
        }
    }

    pub fn from_name(name: &str) -> Option<IntTy> {
        INT_TYPES.into_iter().find(|t| t.name() == name)
    }

    /// `u32_of_int`, and its seven siblings.
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
        }
    }

    /// `int_of_u32`, and its seven siblings.
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
        }
    }

    pub fn signed(self) -> bool {
        matches!(self, IntTy::I8 | IntTy::I16 | IntTy::I32 | IntTy::I64)
    }

    /// The largest value, as the `u64` the representation carries.
    pub fn max(self) -> u64 {
        if self.signed() {
            (1u64 << (self.bits() - 1)) - 1
        } else {
            u64::MAX >> (64 - self.bits())
        }
    }

    /// The smallest value, as an `i128` so that a signed minimum and an unsigned zero are one
    /// vocabulary.
    pub fn min(self) -> i128 {
        if self.signed() {
            -(1i128 << (self.bits() - 1))
        } else {
            0
        }
    }

    /// Whether `v` — a literal's value, or an `Int` being converted — is one of this type's.
    pub fn holds(self, v: i128) -> bool {
        v >= self.min() && v <= self.max() as i128
    }

    /// `bits` reduced to this width and then extended the way this type reads it: zero-extended
    /// when unsigned, sign-extended when signed. Every `Fixed` in the tree is in this form, so
    /// two of one type compare, hash and render as that type orders them.
    pub fn normalize(self, bits: u64) -> u64 {
        let w = self.bits();
        if w == 64 {
            return bits;
        }
        let low = bits & (u64::MAX >> (64 - w));
        if self.signed() && low >> (w - 1) == 1 {
            low | (u64::MAX << w)
        } else {
            low
        }
    }

    /// The mathematical value the bits stand for.
    pub fn value(self, bits: u64) -> i128 {
        if self.signed() {
            (bits as i64) as i128
        } else {
            bits as i128
        }
    }
}

impl fmt::Display for IntTy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The `Type` an [`IntTy`] names.
pub fn int_ty(t: IntTy) -> Type {
    Type::con(t.name())
}
