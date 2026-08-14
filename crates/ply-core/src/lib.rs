pub mod env;
pub mod infer;
pub mod prelude;
pub mod print;
mod scc;
pub mod ty;
pub mod unify;

#[cfg(test)]
mod tests;

use indexmap::IndexMap;
use ply_span::{Diagnostic, SourceId, Span, Symbol};
use ply_syntax::ast::{Mode, Module, ModuleName, Program, SpecKind};
use ply_syntax::resolve::Resolved;

pub use print::{print_row, print_scheme, print_type};
pub use ty::{EffectAtom, Footprint, Resource, Row, RowVar, Scheme, TyVar, Type};

#[derive(Clone, Debug)]
pub struct OpInfo {
    pub name: Symbol,
    pub mode: Mode,
    pub resource_param: bool,
    pub params: Vec<Type>,
    pub ret: Type,
    pub span: Span,
    /// `Some` only for a prelude operation, whose signature is constructed
    /// rather than parsed and may be polymorphic in a type and in an effect row
    /// — `task.spawn` needs both. `None` for every user-declared operation,
    /// which stays monomorphic and types exactly as it did. Surface syntax for
    /// declaring one is deliberately not in M7.
    pub scheme: Option<Scheme>,
}

/// `name` is the program-wide name, `store.db`, and so is the `effect` field of
/// every [`EffectAtom`] it produces. Two modules may each declare an `effect db`
/// without their atoms contending.
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
///
/// A spec is a claim *about* a definition rather than part of it, so it is
/// erased by normalization and appears nowhere in [`DefInfo::footprint`]: adding
/// one changes no definition hash, re-runs no test, and moves no test out of its
/// concurrency group.
#[derive(Clone, Debug)]
pub struct SpecInfo {
    pub kind: SpecKind,
    /// Position among the owner's clauses, in source order. Part of the
    /// obligation's cache key, so reordering two clauses re-runs both.
    pub index: usize,
    /// Always empty — a spec expression's row must be pure, or it could change
    /// what it observes. Carried rather than assumed so that an audit asserts
    /// on a value.
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

/// A standalone `law`. Labelled rather than named, like a [`TestInfo`], so
/// nothing can reference it.
#[derive(Clone, Debug)]
pub struct LawInfo {
    /// The declared label, as written. Not unique program-wide; `key` is.
    pub name: String,
    pub module: ModuleName,
    /// `<module>.<label>`, what this law's hash and obligation are keyed by.
    pub key: Symbol,
    /// Position in [`CheckOutput::laws`].
    pub index: usize,
    /// Empty for a ground law, which is decided by evaluating it.
    pub binders: Vec<LawBinder>,
    pub has_guard: bool,
    /// `{}`, or `{sim.read}` for a concurrency law — which is discharged by
    /// exhaustive interleaving search rather than by a static argument.
    /// Nothing else type-checks.
    pub footprint: Footprint,
    pub span: Span,
}

/// Everywhere in [`CheckOutput`], `name` is the program-wide name and equals
/// this entry's key; `simple_name` is what the source wrote.
#[derive(Clone, Debug)]
pub struct DefInfo {
    pub name: Symbol,
    pub module: ModuleName,
    pub simple_name: Symbol,
    pub scheme: Scheme,
    /// Closed after solving. Empty for a pure function.
    pub footprint: Footprint,
    /// `requires` / `ensures`, in source order. Never restored from a cached
    /// interface: a spec is erased from the definition's hash, so a spec edit
    /// does not move it and gate 2 would otherwise skip a clause that changed.
    pub spec: Vec<SpecInfo>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct TestInfo {
    /// The declared label, as written. Not unique program-wide; `key` is.
    pub name: String,
    pub module: ModuleName,
    /// `<module>.<label>`: unique program-wide, and what a test's hash, closure
    /// and cache entry are keyed by.
    pub key: Symbol,
    /// Position in [`CheckOutput::tests`], which is the order the modules were
    /// loaded in and then source order within each.
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

/// Every map is keyed by program-wide name, so entries from different modules
/// cannot collide and no key is ever rewritten when a definition moves.
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

/// A published interface a caller already holds. Handing one over is how a
/// definition gets compiled once ever rather than once per run: inference
/// publishes it and never walks the body that produced it, so the cost of a
/// check is the definitions that actually changed rather than everything they
/// can be reached from.
///
/// The caller owes the guarantee that comes with it — that the definition's
/// normalized form and every name its interface is written in are unchanged. It
/// is not checked here and cannot be: the evidence is a `DefHash` and a witness,
/// neither of which this crate knows about.
#[derive(Clone, Debug)]
pub struct KnownDef {
    pub scheme: Scheme,
    pub footprint: Footprint,
}

#[derive(Clone, Debug)]
pub struct KnownTest {
    pub footprint: Footprint,
}

#[derive(Clone, Debug, Default)]
pub struct Known {
    /// Program-wide name -> interface. A mutually recursive group is only
    /// skipped when every one of its members appears here.
    pub defs: IndexMap<Symbol, KnownDef>,
    /// Module name -> one slot per `test` in that module, in source order.
    pub tests: IndexMap<Symbol, Vec<Option<KnownTest>>>,
}

impl Known {
    pub fn is_empty(&self) -> bool {
        self.defs.is_empty() && self.tests.is_empty()
    }
}

/// Modules are inferred in [`Resolved::order`], so every name a module reaches
/// already has a scheme by the time its bodies are walked. Definition-level
/// mutual recursion inside one module still goes through the existing SCC path;
/// across modules it cannot arise, because an import cycle was already rejected.
pub fn check_program(
    program: &Program,
    resolved: &Resolved,
) -> Result<CheckOutput, Vec<Diagnostic>> {
    infer::check_program_with(program, resolved, &Known::default())
}

/// [`check_program`] with interfaces supplied for definitions the caller has
/// already proved unchanged.
pub fn check_program_with(
    program: &Program,
    resolved: &Resolved,
    known: &Known,
) -> Result<CheckOutput, Vec<Diagnostic>> {
    infer::check_program_with(program, resolved, known)
}

/// Checks one module with nothing imported. Convenience for snippets and tests.
pub fn check_module(module: &Module) -> Result<CheckOutput, Vec<Diagnostic>> {
    infer::check_module(module)
}
