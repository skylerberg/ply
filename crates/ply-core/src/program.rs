//! The program record: every table `packages/compiler/front.ply` answers for a program, as
//! `ply_compiled::c::dump` reads them. Its types are the front end's alone.

use crate::{
    Carry, CtorCarries, DefHash, Diagnostic, Footprint, HashOutput, Mode, Severity, SourceId, Span,
    Symbol,
};
use indexmap::IndexMap;
use std::fmt;

/// One keyable item of a module, in source order: what the backend's cache keys are minted from.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Ordinal {
    /// A `fn`, with the kind of each `requires` / `ensures` clause in source order.
    Fn(Symbol, Vec<SpecKind>),
    /// A `test`, by `<module>.<label>`.
    Test(Symbol),
    /// A `law`, by `<module>.<label>`.
    Law(Symbol),
}

/// A `type`; no table of [`CheckOutput`] holds its arity, visibility or span.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TypeDecl {
    pub name: Symbol,
    pub module: ModuleName,
    pub simple_name: Symbol,
    pub vis: Visibility,
    /// Type parameter count; their names never escape.
    pub arity: usize,
    pub span: Span,
}

/// One root the emitter offers: a definition, a spec clause, a test or a law part.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EmitterRoot {
    pub root: Symbol,
    pub arity: usize,
    /// Whether the compiler published the root pure: nothing else lets a memo answer for it. A
    /// test, a clause or a law part never is.
    pub pure: bool,
    /// The root's definition span, which a failure is reported against.
    pub span: Span,
    /// How its parameters read, over the type variables they share with its answer; a label
    /// parameter has none.
    pub params: Vec<Carry>,
    /// How its answer reads back out of compiled code.
    pub answer: Carry,
    /// The type variables whose witnesses lead `params`: an entry from outside passes each the
    /// type its arguments show for it.
    pub witnesses: Vec<usize>,
}

impl EmitterRoot {
    /// A pure root of no arguments: the unit gives it a memo slot.
    pub fn constant(&self) -> bool {
        self.pure && self.arity == 0
    }
}

#[derive(Clone, Debug, Default)]
pub struct Analysis {
    pub diagnostics: Vec<Diagnostic>,
    pub check: CheckOutput,
    pub hashes: HashOutput,
    /// The digest of [`hashes`](Analysis::hashes), as the compiler computed it.
    pub hashes_digest: DefHash,
    /// Every root the emitter offers, with the arity its body is emitted at.
    pub emitter_roots: Vec<EmitterRoot>,
    /// The emitter's constructors, in the order the emitted C names tags by.
    pub emitter_ctors: Vec<(Symbol, usize)>,
    /// How each constructor's fields read back out of compiled code.
    pub ctor_carries: CtorCarries,
    /// Per module in program order, its keyable items in source order.
    pub ordinals: Vec<(Symbol, Vec<Ordinal>)>,
    /// Every `type` the source declares, by program-wide name, in program order.
    pub types: IndexMap<Symbol, TypeDecl>,
}

impl Analysis {
    pub fn has_error(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    }
}

#[derive(Clone, Debug)]
pub struct OpInfo {
    pub name: Symbol,
    pub mode: Mode,
    pub resource_param: bool,
    pub span: Span,
    /// How each argument a host handler is given reads out of compiled code.
    pub params: Vec<Carry>,
}

/// `name` is program-wide (`store.db`) and equals the `effect` of every
/// [`EffectAtom`](crate::EffectAtom) it makes.
#[derive(Clone, Debug)]
pub struct EffectInfo {
    pub name: Symbol,
    pub module: ModuleName,
    pub simple_name: Symbol,
    pub nondet: bool,
    pub ops: IndexMap<Symbol, OpInfo>,
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
    /// `{}`, or `{sim.read}` for a concurrency law.
    pub footprint: Footprint,
    pub span: Span,
}

impl LawInfo {
    /// The root of the law's `part`, `guard` or `body`, as the front end's emitter names it.
    pub fn root(&self, part: &str) -> Symbol {
        self.module.qualify(&Symbol::new(format!(
            "{}.{part}",
            labelled("law", &self.name)
        )))
    }
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

impl TestInfo {
    /// The root the test runs as, which the front end's emitter names by the test's label.
    pub fn root(&self) -> Symbol {
        self.module
            .qualify(&Symbol::new(labelled("test", &self.name)))
    }
}

/// `kind`, `#` and a digest of `label`: a test's or a law's name within its module, which only its
/// label moves.
fn labelled(kind: &str, label: &str) -> String {
    let digest = blake3::hash(label.as_bytes());
    let hex: String = digest.as_bytes()[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("{kind}#{hex}")
}

#[derive(Clone, Debug)]
pub struct ModuleInfo {
    pub name: ModuleName,
    pub source: SourceId,
    /// Program-wide names of everything this module declares, in source order.
    pub items: Vec<Symbol>,
    /// Each module this one imports, as the program names it.
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

/// A module's dotted name from its path under the root: `store/orders.ply` is `store.orders`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ModuleName(Symbol);

impl Default for ModuleName {
    fn default() -> Self {
        ModuleName::anonymous()
    }
}

impl ModuleName {
    /// The module of source that has no project root: a snippet.
    pub fn anonymous() -> ModuleName {
        ModuleName(Symbol::new(""))
    }

    pub fn is_anonymous(&self) -> bool {
        self.0.as_str().is_empty()
    }

    /// Trusts the caller that every segment is an identifier.
    pub fn from_dotted(name: impl AsRef<str>) -> ModuleName {
        ModuleName(Symbol::new(name.as_ref()))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    pub fn segments(&self) -> impl Iterator<Item = &str> {
        self.0.as_str().split('.')
    }

    /// This module's `place` under its program-wide name, `store.orders.place`.
    pub fn qualify(&self, name: &Symbol) -> Symbol {
        if self.is_anonymous() {
            return name.clone();
        }
        Symbol::new(format!("{}.{}", self.0, name))
    }
}

impl fmt::Display for ModuleName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.as_str())
    }
}

/// `pub` exports an item.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Visibility {
    #[default]
    Private,
    Public,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SpecKind {
    Requires,
    Ensures,
}

impl SpecKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SpecKind::Requires => "requires",
            SpecKind::Ensures => "ensures",
        }
    }
}
