//! The type vocabulary: what a checked program's types, rows, footprints and interfaces look
//! like, shared by the checker that produces them and everything downstream that reads them.
//! Pinned: concurrent crates are written against these shapes.

pub mod decl;
pub mod expr;
pub mod front;
pub mod hash;
pub mod parse;
pub mod prelude;
pub mod print;
pub mod ty;

use indexmap::IndexMap;
use ply_span::{SourceId, Span, Symbol};

pub use decl::{
    Deriver, ModuleName, SpecKind, Visibility, is_ident, is_ident_continue, is_ident_start,
};
pub use expr::{BinOp, Lit, UnOp, render_float};
pub use front::{
    DefWritten, EffectSet, Front, Hashed, Literal, Ordinal, TypeDecl, WrittenParam, read_front,
    write_front,
};
pub use hash::{DefHash, HashOutput, spec_hash};
pub use parse::{parse_atom, parse_footprint, parse_row, parse_scheme, parse_type};
pub use print::{print_row, print_scheme, print_type};
pub use ty::*;

#[derive(Clone, Debug)]
pub struct OpInfo {
    pub name: Symbol,
    pub mode: Mode,
    pub resource_param: bool,
    pub params: Vec<Type>,
    pub ret: Type,
    pub span: Span,
    /// `Some` only for a prelude operation, whose signature is constructed rather than parsed and
    /// may be polymorphic in a type and in an effect row — `task.spawn` needs both.
    pub scheme: Option<Scheme>,
}

/// `name` is the program-wide name, `store.db`, and so is the `effect` field of every
/// [`EffectAtom`] it produces.
#[derive(Clone, Debug)]
pub struct EffectInfo {
    pub name: Symbol,
    pub module: ModuleName,
    pub simple_name: Symbol,
    pub nondet: bool,
    pub ops: IndexMap<Symbol, OpInfo>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct CtorInfo {
    pub name: Symbol,
    pub module: ModuleName,
    pub simple_name: Symbol,
    pub type_name: Symbol,
    /// Position among the owning type's variants, in declaration order.
    pub index: usize,
    pub arity: usize,
    pub fields: Vec<Type>,
    /// Nullary variants have the sum type itself; the rest have a function type.
    pub scheme: Scheme,
    pub span: Span,
}

/// A `requires` or `ensures` clause that type-checked.
#[derive(Clone, Debug)]
pub struct SpecInfo {
    pub kind: SpecKind,
    /// Position among the owner's clauses, in source order.
    pub index: usize,
    /// Always empty — a spec expression's row must be pure, or it could change what it observes.
    pub footprint: Footprint,
    pub span: Span,
}

/// A `forall` binder, after its declared type is resolved.
#[derive(Clone, Debug)]
pub struct LawBinder {
    pub name: Symbol,
    pub ty: Type,
    pub span: Span,
}

/// A standalone `law`.
#[derive(Clone, Debug)]
pub struct LawInfo {
    /// The declared label, as written.
    pub name: String,
    pub module: ModuleName,
    /// `<module>.<label>`, what this law's hash and obligation are keyed by.
    pub key: Symbol,
    /// Position in [`CheckOutput::laws`].
    pub index: usize,
    /// Empty for a ground law, which is decided by evaluating it.
    pub binders: Vec<LawBinder>,
    pub has_guard: bool,
    /// `law/host`: the body may carry any row, so this law reaches the world.
    pub host: bool,
    /// `{}`, or `{sim.read}` for a concurrency law — which is discharged by exhaustive interleaving
    /// search rather than by a static argument — or any row at all when [`host`](LawInfo::host) is
    /// set.
    pub footprint: Footprint,
    pub span: Span,
}

/// A published `where derivable(D, a)`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DefConstraint {
    pub deriver: Deriver,
    /// Index into [`Scheme::ty_vars`].
    pub param: usize,
}

/// Everywhere in [`CheckOutput`], `name` is the program-wide name and equals this entry's key;
/// `simple_name` is what the source wrote.
#[derive(Clone, Debug)]
pub struct DefInfo {
    pub name: Symbol,
    pub module: ModuleName,
    pub simple_name: Symbol,
    pub scheme: Scheme,
    /// The **published** row: the `/ {..}` annotation when there is one, and the inferred row when
    /// there is not.
    pub footprint: Footprint,
    /// What row inference computed for the **body**.
    pub performed: Footprint,
    /// The `effect set`s this definition's row was written with, in source order, by simple name.
    pub row_aliases: Vec<Symbol>,
    /// `where derivable(D, a)`, sorted and deduplicated exactly as the hash encodes them.
    pub constraints: Vec<DefConstraint>,
    /// `requires` / `ensures`, in source order.
    pub spec: Vec<SpecInfo>,
    /// Whether running this definition can execute a `perform` that [`DefInfo::footprint`] does not
    /// show.
    pub internally_effectful: bool,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct TestInfo {
    /// The declared label, as written.
    pub name: String,
    pub module: ModuleName,
    /// `<module>.<label>`: unique program-wide, and what a test's hash, closure and cache entry are
    /// keyed by.
    pub key: Symbol,
    /// Position in [`CheckOutput::tests`], which is the order the modules were loaded in and then
    /// source order within each.
    pub index: usize,
    pub nondet: bool,
    pub footprint: Footprint,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct ModuleInfo {
    pub name: ModuleName,
    pub source: SourceId,
    /// Program-wide names of everything this module declares, in source order.
    pub items: Vec<Symbol>,
    pub imports: Vec<ModuleName>,
}

/// Every map is keyed by program-wide name, so entries from different modules cannot collide and no
/// key is ever rewritten when a definition moves.
#[derive(Clone, Debug, Default)]
pub struct CheckOutput {
    pub defs: IndexMap<Symbol, DefInfo>,
    pub tests: Vec<TestInfo>,
    pub laws: Vec<LawInfo>,
    pub effects: IndexMap<Symbol, EffectInfo>,
    pub ctors: IndexMap<Symbol, CtorInfo>,
    pub modules: IndexMap<Symbol, ModuleInfo>,
}

impl CheckOutput {
    pub fn effect_of(&self, atom: &EffectAtom) -> Option<&EffectInfo> {
        self.effects.get(&atom.effect)
    }

    pub fn is_nondet(&self, atom: &EffectAtom) -> bool {
        self.effects.get(&atom.effect).is_some_and(|e| e.nondet)
    }
}

/// A published interface a caller already holds.
#[derive(Clone, Debug)]
pub struct KnownDef {
    pub scheme: Scheme,
    pub footprint: Footprint,
    /// What the body performed when it was last walked.
    pub performed: Footprint,
}

#[derive(Clone, Debug)]
pub struct KnownTest {
    pub footprint: Footprint,
}

#[derive(Clone, Debug, Default)]
pub struct Known {
    /// Program-wide name -> interface.
    pub defs: IndexMap<Symbol, KnownDef>,
    /// Module name -> one slot per `test` in that module, in source order.
    pub tests: IndexMap<Symbol, Vec<Option<KnownTest>>>,
}

impl Known {
    pub fn is_empty(&self) -> bool {
        self.defs.is_empty() && self.tests.is_empty()
    }
}
