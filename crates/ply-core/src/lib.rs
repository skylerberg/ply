pub mod derivable;
pub mod env;
pub mod infer;
pub mod prelude;
pub mod print;
mod scc;
pub mod ty;
pub mod unify;

#[cfg(test)]
mod numerics;
#[cfg(test)]
mod tests;

use indexmap::IndexMap;
use ply_span::{Diagnostic, SourceId, Span, Symbol};
use ply_syntax::ast::{Deriver, Mode, Module, ModuleName, Program, SpecKind};
use ply_syntax::resolve::Resolved;

pub use derivable::{Adt, Blocked, Context as Derivability, Why, derivable, ordered};
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
    /// `law/host`: the body may carry any row, so this law reaches the world.
    ///
    /// It can never be `proved` — the prover's lowering refuses a body whose row
    /// is non-empty, so the certificate cannot be constructed — it is never
    /// cached, and under a hermetic run it is `W0604 unattempted` rather than
    /// green.
    pub host: bool,
    /// `{}`, or `{sim.read}` for a concurrency law — which is discharged by
    /// exhaustive interleaving search rather than by a static argument — or any
    /// row at all when [`host`](LawInfo::host) is set.
    pub footprint: Footprint,
    pub span: Span,
}

/// A published `where derivable(D, a)`.
///
/// The parameter is named by its **position** in the scheme's quantified list
/// rather than by the name the source wrote, for the same reason a hash carries
/// a de Bruijn level: renaming a type parameter may not change an interface.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DefConstraint {
    pub deriver: Deriver,
    /// Index into [`Scheme::ty_vars`].
    pub param: usize,
}

/// Everywhere in [`CheckOutput`], `name` is the program-wide name and equals
/// this entry's key; `simple_name` is what the source wrote.
#[derive(Clone, Debug)]
pub struct DefInfo {
    pub name: Symbol,
    pub module: ModuleName,
    pub simple_name: Symbol,
    pub scheme: Scheme,
    /// The **published** row: the `/ {..}` annotation when there is one, and
    /// the inferred row when there is not. Closed after solving, and empty for
    /// a pure function. This is what callers are checked against, what the
    /// scheduler builds the conflict graph from, and what the frame condition
    /// of a spec is taken over.
    pub footprint: Footprint,
    /// What row inference computed for the **body**. Always a subset of
    /// [`DefInfo::footprint`], and equal to it for a definition with no
    /// annotation.
    ///
    /// Provenance, and only that: it enters no hash, no cache key, no
    /// scheduling decision and no determinism verdict. It exists so that the
    /// cost of a declared row wider than the body's — which an `effect set`
    /// makes systematic, since one set is written for a whole service and most
    /// definitions touch a part of it — is published rather than left to be
    /// discovered from an `isolated: n of m` number nobody wrote down.
    pub performed: Footprint,
    /// The `effect set`s this definition's row was written with, in source
    /// order, by simple name.
    ///
    /// Namespace metadata like a module name or an import alias: erased by
    /// normalization, so a row written `/ {Web}` and one written with `Web`'s
    /// atoms are one definition with one hash. Carried so `--explain` can print
    /// how the row was spelled beside what it means.
    pub row_aliases: Vec<Symbol>,
    /// `where derivable(D, a)`, sorted and deduplicated exactly as the hash
    /// encodes them. Part of the published signature: adding one narrows the
    /// call sites this definition admits, so a caller checked against the
    /// unconstrained form has to be rechecked, and gate 2 only rechecks a
    /// definition whose dependency's hash moved.
    pub constraints: Vec<DefConstraint>,
    /// `requires` / `ensures`, in source order. Never restored from a cached
    /// interface: a spec is erased from the definition's hash, so a spec edit
    /// does not move it and gate 2 would otherwise skip a clause that changed.
    pub spec: Vec<SpecInfo>,
    /// Whether running this definition can execute a `perform` that
    /// [`DefInfo::footprint`] does not show.
    ///
    /// An atom performed inside a call either escapes — and then the published
    /// row carries it — or is discharged by a `handle` inside the call, and then
    /// no row anywhere records that it happened. The second kind is invisible to
    /// [`DefInfo::footprint`] and to [`DefInfo::performed`] alike, because row
    /// inference discharges it. This is the fact that distinguishes such a
    /// definition from a genuinely pure one, and it is what
    /// `ply_eval::compiled::admit` reads: a native body performs nothing, so
    /// entering one in place of a definition that performs and handles its own
    /// operations loses every atom it would have recorded.
    ///
    /// **Transitive, and it has to be.** A definition that merely *calls* one
    /// that discharges its own effects publishes an empty row too and is written
    /// with neither `perform` nor `handle`, so a per-body syntactic bit clears
    /// it and loses exactly the same atoms. True here means "this body is
    /// written with `perform` or `handle`, or something reachable from it is".
    ///
    /// **True is the safe value and it is the default.** Inference constructs
    /// every `DefInfo` with this set, and only `Checker::mark_internal_effects`
    /// lowers it, for a definition it positively cleared. A definition nothing
    /// walked — a module gate 1 skipped, an `ExprKind` a future scan forgets —
    /// stays refused rather than becoming enterable.
    pub internally_effectful: bool,
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
    /// What the body performed when it was last walked. Restored rather than
    /// recomputed, because the point of handing an interface over is that the
    /// body is not walked at all — and `ply check --explain` must print the
    /// same bytes on a warm run as on a cold one, or the reviewing command's
    /// output is a function of what the cache held.
    pub performed: Footprint,
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
