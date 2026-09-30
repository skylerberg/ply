//! What the runtime reads of a checked program: its names, rows, hashes and tables, as the front
//! end answers them. Its types are the front end's alone.

pub mod decl;
pub mod front;
pub mod hash;
pub mod ty;

use indexmap::IndexMap;
use ply_span::{SourceId, Span, Symbol};

pub use decl::{ModuleName, SpecKind, Visibility, is_ident, is_ident_continue, is_ident_start};
pub use front::{DefWritten, EffectSet, Front, Hashed, Literal, Ordinal, TypeDecl, WrittenParam};
pub use hash::{DefHash, HashOutput};
pub use ty::*;

#[derive(Clone, Debug)]
pub struct OpInfo {
    pub name: Symbol,
    pub mode: Mode,
    pub resource_param: bool,
    pub span: Span,
}

/// `name` is program-wide (`store.db`) and equals the `effect` of every [`EffectAtom`] it makes.
#[derive(Clone, Debug)]
pub struct EffectInfo {
    pub name: Symbol,
    pub module: ModuleName,
    pub simple_name: Symbol,
    pub nondet: bool,
    pub ops: IndexMap<Symbol, OpInfo>,
    pub span: Span,
}

/// A `requires` or `ensures` clause that type-checked.
#[derive(Clone, Debug)]
pub struct SpecInfo {
    pub kind: SpecKind,
    /// Position among the owner's clauses, in source order.
    pub index: usize,
    /// Always empty: a spec expression must not change what it observes.
    pub footprint: Footprint,
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
    pub has_guard: bool,
    /// `law/host`: the body may carry any row.
    pub host: bool,
    /// `{}`, `{sim.read}` for a concurrency law, or any row when [`host`](LawInfo::host) is set.
    pub footprint: Footprint,
    pub span: Span,
}

/// Everywhere in [`CheckOutput`], `name` is the program-wide name and equals this entry's key;
/// `simple_name` is what the source wrote.
#[derive(Clone, Debug)]
pub struct DefInfo {
    pub name: Symbol,
    pub module: ModuleName,
    pub simple_name: Symbol,
    /// The published row: the `/ {..}` annotation if written, else the inferred row.
    pub footprint: Footprint,
    /// The row inference computed for the body.
    pub performed: Footprint,
    /// The `effect set`s this definition's row was written with, in source order, by simple name.
    pub row_aliases: Vec<Symbol>,
    /// `requires` / `ensures`, in source order.
    pub spec: Vec<SpecInfo>,
    /// Whether running this can execute a `perform` that [`DefInfo::footprint`] does not show.
    pub internally_effectful: bool,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct TestInfo {
    /// The declared label, as written.
    pub name: String,
    pub module: ModuleName,
    /// `<module>.<label>`: unique program-wide; keys the test's hash, closure and cache entry.
    pub key: Symbol,
    /// Position in [`CheckOutput::tests`]: module load order, then source order.
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

/// Every map is keyed by program-wide name, so entries from different modules cannot collide.
#[derive(Clone, Debug, Default)]
pub struct CheckOutput {
    pub defs: IndexMap<Symbol, DefInfo>,
    pub tests: Vec<TestInfo>,
    pub laws: Vec<LawInfo>,
    pub effects: IndexMap<Symbol, EffectInfo>,
    pub modules: IndexMap<Symbol, ModuleInfo>,
}
