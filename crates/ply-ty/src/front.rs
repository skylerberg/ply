//! The front end's answer: every table `crates/ply-compiler/ply/front.ply` answers for a program,
//! as `ply_codegen::c::dump` reads them.

use crate::hash::{DefHash, HashOutput};
use crate::{CheckOutput, Footprint, ModuleName, SpecKind, Visibility};
use indexmap::IndexMap;
use ply_span::{Diagnostic, Severity, Span, Symbol};
use std::collections::BTreeSet;

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

/// One entry of the hasher's item order: what one of its hash rows is about.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Hashed {
    /// A `fn`, `type` or `effect`, by program-wide name; one entry for a name in two namespaces.
    Def(Symbol),
    /// A test, by its position in `CheckOutput::tests`.
    Test(usize),
    /// A law, by its position in `CheckOutput::laws`.
    Law(usize),
}

/// A parameter as the source wrote it, which a spec clause's binders are named and placed by.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct WrittenParam {
    pub name: Symbol,
    pub span: Span,
}

/// What a `fn`'s source says and [`DefInfo`](crate::DefInfo) does not.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct DefWritten {
    pub vis: Visibility,
    /// A `reuse fn`, which a gate has to know without a parse.
    pub reuse: bool,
    /// In source order.
    pub params: Vec<WrittenParam>,
    pub requires_literals: Vec<Literal>,
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

/// One `effect set` of a module.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EffectSet {
    /// The simple name, which is what a row writes.
    pub name: Symbol,
    /// The sets this one includes, by simple name, in source order.
    pub includes: Vec<Symbol>,
    /// The expansion as program-wide atoms; an atom naming an unresolved effect is dropped.
    pub atoms: Footprint,
}

/// A literal a guard mentions, which is where the witness search looks for a domain.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Literal {
    Int(i64),
    Str(String),
    Bytes(Vec<u8>),
}

/// One dependency as `ply.lock` pins it: what it calls itself, the version it declares, and the
/// BLAKE3 digest of the modules it contributed. Read from the front end's answer, whose package
/// judgments are what make a pin mean one thing rather than a path.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pinned {
    pub name: String,
    pub version: String,
    pub digest: String,
}

/// One root the emitter offers: a definition, a spec clause, a test or a law part.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EmitterRoot {
    pub root: Symbol,
    pub arity: usize,
    pub scalar: bool,
    /// Whether the root's scheme mentions a fixed-width type, which the compiled seam cannot
    /// carry.
    pub width: bool,
    /// The root's definition span, which a failure is reported against.
    pub span: Span,
}

#[derive(Clone, Debug, Default)]
pub struct Front {
    pub diagnostics: Vec<Diagnostic>,
    /// Dependency-first module order, by module name.
    pub order: Vec<Symbol>,
    /// The closure's packages as `(prefix, declared dep prefixes)`; empty for a project
    /// without packages.
    pub packages: Vec<(String, Vec<String>)>,
    /// The closure's dependencies, in package order, as a lockfile pins them. The root package is
    /// the project's own sources and has no entry.
    pub pins: Vec<Pinned>,
    /// Each module's package, in program order: an index into `packages`, or one past the end
    /// for a module the toolchain ships.
    pub mod_pkg: Vec<usize>,
    pub check: CheckOutput,
    pub hashes: HashOutput,
    /// The digest of [`hashes`](Front::hashes), as the compiler computed it.
    pub hashes_digest: DefHash,
    /// The emitter's root cache keys, by root name, as the compiler computed them.
    pub keys: IndexMap<Symbol, String>,
    /// Every root the emitter offers, with the arity its body is emitted at and whether its
    /// parameters and answer are all `Int` or `Bool`.
    pub emitter_roots: Vec<EmitterRoot>,
    /// The emitter's constructors, in the order the emitted C names tags by.
    pub emitter_ctors: Vec<(Symbol, usize)>,
    /// The roots that are pure and take no parameters, which the emitted unit memoizes.
    pub emitter_constants: BTreeSet<Symbol>,
    /// The hasher's item order: every hashed name, test and law, as its rows come.
    pub hash_order: Vec<Hashed>,
    /// Per module in program order, its keyable items in source order.
    pub ordinals: Vec<(Symbol, Vec<Ordinal>)>,
    /// Every `fn`'s, `type`'s and `effect`'s stored body, in the hasher's item order.
    pub bodies: Vec<(Symbol, Vec<u8>)>,
    /// Parallel to `CheckOutput::tests`.
    pub test_bodies: Vec<Vec<u8>>,
    /// What each `fn`'s source wrote, by program-wide name: one entry per `CheckOutput::defs`.
    pub defs_written: IndexMap<Symbol, DefWritten>,
    /// Every `type` the source declares, by program-wide name, in program order.
    pub types: IndexMap<Symbol, TypeDecl>,
    /// Whether each `effect` was written `pub`; a prelude effect has no entry and is public.
    pub effects_written: IndexMap<Symbol, Visibility>,
    /// Parallel to `CheckOutput::tests`: the span of each test's label.
    pub test_name_spans: Vec<Span>,
    /// Parallel to `CheckOutput::laws`: the literals each law's guard mentions, in walk order.
    pub law_literals: Vec<Vec<Literal>>,
    /// Every module's `effect set`s in source order; a module that declares none has no entry.
    pub effect_sets: IndexMap<Symbol, Vec<EffectSet>>,
}

impl Front {
    pub fn has_error(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    }
}
