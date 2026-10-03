//! The program record: every table `crates/ply-compiler/ply/front.ply` answers for a program, as
//! `ply_codegen::c::dump` reads them. Its types are the front end's alone.

use crate::{
    Carry, CtorCarries, DefHash, Diagnostic, Footprint, HashOutput, Mode, Severity, SourceId, Span,
    Symbol, codes,
};
use indexmap::IndexMap;
use std::fmt;
use std::path::Path;

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

/// A parameter as the source wrote it, which a spec clause's binders are named and placed by.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct WrittenParam {
    pub name: Symbol,
    pub span: Span,
}

/// What a `fn`'s source says and [`DefInfo`] does not.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct DefWritten {
    pub vis: Visibility,
    /// A `reuse fn`, which a gate has to know without a parse.
    pub reuse: bool,
    /// In source order.
    pub params: Vec<WrittenParam>,
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
    /// The closure's packages as `(prefix, declared dep prefixes)`; empty for a project
    /// without packages.
    pub packages: Vec<(String, Vec<String>)>,
    /// The closure's dependencies, in package order, as a lockfile pins them. The root package is
    /// the project's own sources and has no entry.
    pub pins: Vec<Pinned>,
    /// Each module's package, in program order: an index into `packages`, or one past the end
    /// for a module the toolchain ships.
    pub module_packages: Vec<usize>,
    pub check: CheckOutput,
    pub hashes: HashOutput,
    /// The digest of [`hashes`](Analysis::hashes), as the compiler computed it.
    pub hashes_digest: DefHash,
    /// The emitter's root cache keys, by root name, as the compiler computed them.
    pub keys: IndexMap<Symbol, String>,
    /// Every root the emitter offers, with the arity its body is emitted at.
    pub emitter_roots: Vec<EmitterRoot>,
    /// The emitter's constructors, in the order the emitted C names tags by.
    pub emitter_ctors: Vec<(Symbol, usize)>,
    /// How each constructor's fields read back out of compiled code.
    pub ctor_carries: CtorCarries,
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
    /// What the front end embedded, its `List<embed.EmbedAt>` as [`crate::codec`] encodes it and
    /// empty when nothing was: every pass that parses the program's text again is handed it back.
    pub embeds: Vec<u8>,
    /// What every definition and test published, its `front.Rows` as [`crate::codec`] encodes it
    /// and empty when the answer carried none: the emitter walks only the bodies it lowers.
    pub rows: Vec<u8>,
    /// What the front end's check read of the bodies it walked, a `front.WalkedFacts` in the encoding
    /// `std.bin` reads it with, empty when the answer carried none.
    pub walked: Vec<u8>,
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

    /// Every directory component and the file stem must be a Ply identifier.
    pub fn from_relative_path(path: &Path) -> Result<ModuleName, Diagnostic> {
        let invalid = |what: &str| {
            Diagnostic::error(
                codes::INVALID_MODULE_PATH,
                format!("`{}` cannot be a module: {what}", path.display()),
            )
            .primary(Span::DUMMY, "this file is not addressable as a module")
            .note("rename it so every directory and the file stem is a plain identifier")
        };

        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| invalid("its file name is not valid UTF-8"))?;

        let mut segments: Vec<&str> = Vec::new();
        for component in path.parent().into_iter().flat_map(|p| p.components()) {
            let text = component
                .as_os_str()
                .to_str()
                .ok_or_else(|| invalid("a directory name is not valid UTF-8"))?;
            segments.push(text);
        }
        segments.push(stem);

        for segment in &segments {
            if !is_ident(segment) {
                return Err(invalid(&format!("`{segment}` is not an identifier")));
            }
        }
        Ok(ModuleName(Symbol::new(segments.join("."))))
    }

    /// Trusts the caller that every segment is an identifier.
    pub fn from_dotted(name: impl AsRef<str>) -> ModuleName {
        ModuleName(Symbol::new(name.as_ref()))
    }

    pub fn as_symbol(&self) -> &Symbol {
        &self.0
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

/// The grammar's identifier rule, which module paths are held to as well. It is ASCII, the rule
/// the lexer implements (`crates/ply-compiler/ply/lexer.ply`) and the walk repeats
/// (`crates/ply-cli/ply/sources.ply`).
pub fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(is_ident_start) && chars.all(is_ident_continue)
}

pub fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

pub fn is_ident_continue(c: char) -> bool {
    is_ident_start(c) || c.is_ascii_digit()
}

/// `pub` exports an item.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Visibility {
    #[default]
    Private,
    Public,
}

impl Visibility {
    pub fn is_public(self) -> bool {
        self == Visibility::Public
    }
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
