//! Pinned: downstream crates are written against these shapes concurrently, so
//! changing a variant is a cross-crate breaking change.

use ply_span::{Diagnostic, SourceId, Span, Symbol, codes};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ident {
    pub name: Symbol,
    pub span: Span,
}

impl Ident {
    pub fn new(name: impl Into<Symbol>, span: Span) -> Self {
        Ident {
            name: name.into(),
            span,
        }
    }
}

/// A reference to a top-level name, optionally qualified by a module binder:
/// `place` or `orders::place`.
///
/// Only the bare form can be shadowed by a local binder. The qualified form is
/// the escape hatch from every name collision, so it never resolves to anything
/// but the named module's export.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QName {
    pub module: Option<Ident>,
    pub name: Ident,
    pub span: Span,
}

impl QName {
    pub fn bare(name: Ident) -> QName {
        let span = name.span;
        QName {
            module: None,
            name,
            span,
        }
    }

    pub fn qualified(module: Ident, name: Ident) -> QName {
        let span = module.span.to(name.span);
        QName {
            module: Some(module),
            name,
            span,
        }
    }

    pub fn is_bare(&self) -> bool {
        self.module.is_none()
    }

    pub fn symbol(&self) -> &Symbol {
        &self.name.name
    }
}

impl From<Ident> for QName {
    fn from(name: Ident) -> QName {
        QName::bare(name)
    }
}

impl fmt::Display for QName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.module {
            Some(m) => write!(f, "{}::{}", m.name, self.name.name),
            None => write!(f, "{}", self.name.name),
        }
    }
}

/// A module's dotted name, derived from its file's path relative to the project
/// root: `store/orders.ply` is `store.orders`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ModuleName(Symbol);

impl Default for ModuleName {
    fn default() -> Self {
        ModuleName::anonymous()
    }
}

impl ModuleName {
    /// The module of source that has no project root: a snippet handed to
    /// [`crate::parse`]. It has the empty dotted name, [`ModuleName::qualify`]
    /// leaves names bare, and it can neither be imported nor import.
    pub fn anonymous() -> ModuleName {
        ModuleName(Symbol::new(""))
    }

    pub fn is_anonymous(&self) -> bool {
        self.0.as_str().is_empty()
    }

    /// Every directory component and the file stem must be a Ply identifier;
    /// anything else is [`codes::INVALID_MODULE_PATH`].
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
            if !crate::lexer::is_ident(segment) {
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

    /// The name a bare `import` binds this module as: its last segment.
    pub fn default_binder(&self) -> Symbol {
        Symbol::new(self.0.as_str().rsplit('.').next().unwrap_or(""))
    }

    /// This module's `place` under its program-wide name, `store.orders.place`.
    /// A `.` cannot occur in an identifier, so a qualified name can never
    /// collide with one written in source.
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

/// `pub` exports an item. A `pub type` exports its constructors with it.
///
/// Visibility is namespace metadata: it is erased by normalization, so adding or
/// removing `pub` changes no definition hash.
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

/// One file, one module. `imports` are lexically before every item.
#[derive(Clone, Debug)]
pub struct Module {
    pub name: ModuleName,
    pub source: SourceId,
    pub imports: Vec<ImportDecl>,
    pub items: Vec<Item>,
}

impl Default for Module {
    fn default() -> Self {
        Module {
            name: ModuleName::anonymous(),
            source: Span::DUMMY.source,
            imports: Vec::new(),
            items: Vec::new(),
        }
    }
}

/// Every module in the project. Order is the load order — paths sorted — which
/// is metadata and never enters a hash.
#[derive(Clone, Debug, Default)]
pub struct Program {
    pub modules: Vec<Module>,
}

impl Program {
    pub fn single(module: Module) -> Program {
        Program {
            modules: vec![module],
        }
    }

    pub fn find(&self, name: &ModuleName) -> Option<&Module> {
        self.modules.iter().find(|m| &m.name == name)
    }

    pub fn index_of(&self, name: &ModuleName) -> Option<usize> {
        self.modules.iter().position(|m| &m.name == name)
    }
}

/// `import store.orders`, `import store.orders as ord`,
/// `import store.orders (place, cancel)`.
#[derive(Clone, Debug)]
pub struct ImportDecl {
    /// One `Ident` per dotted segment, so a diagnostic can point at the segment
    /// that went wrong rather than the whole declaration.
    pub path: Vec<Ident>,
    pub kind: ImportKind,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum ImportKind {
    /// Binds the module under its last path segment.
    Module,
    Alias(Ident),
    /// Binds those names unqualified, and binds no module binder at all.
    Names(Vec<Ident>),
}

impl ImportDecl {
    pub fn module_name(&self) -> ModuleName {
        ModuleName::from_dotted(
            self.path
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>()
                .join("."),
        )
    }

    pub fn path_span(&self) -> Span {
        match (self.path.first(), self.path.last()) {
            (Some(first), Some(last)) => first.span.to(last.span),
            _ => self.span,
        }
    }

    /// The module binder this import introduces, if any. A selective import
    /// introduces none.
    pub fn binder(&self) -> Option<Symbol> {
        match &self.kind {
            ImportKind::Module => self.path.last().map(|s| s.name.clone()),
            ImportKind::Alias(a) => Some(a.name.clone()),
            ImportKind::Names(_) => None,
        }
    }

    pub fn binder_span(&self) -> Span {
        match &self.kind {
            ImportKind::Module => self.path.last().map_or(self.span, |s| s.span),
            ImportKind::Alias(a) => a.span,
            ImportKind::Names(_) => self.span,
        }
    }
}

/// Boxed because a `fn` is several times the size of the other variants and a
/// module holds one `Vec<Item>` per file: unboxed, a program of ten thousand
/// definitions pays the largest variant's width for every item it has.
#[derive(Clone, Debug)]
pub enum Item {
    Fn(Box<FnDef>),
    Type(Box<TypeDef>),
    Effect(Box<EffectDef>),
    Test(Box<TestDef>),
    /// `law "label" forall (x: T) where g { body }`. Labelled like a `test`, so
    /// nothing can reference it and it is never `pub`.
    Law(Box<LawDef>),
    /// `derive json for Order`. Declares nothing itself: expansion walks the
    /// target's structure and appends ordinary [`Item::Fn`]s to the module, and
    /// those are what the rest of the pipeline sees. So every consumer that
    /// enumerates definitions is right to skip this variant.
    Derive(Box<DeriveDef>),
    /// `effect set Web = {db.read[users], log.write}`. Declares nothing a
    /// reference can reach: the parser has already expanded every row that
    /// names it, so what survives here is provenance for `--explain`. Skipped
    /// by every consumer that enumerates definitions, exactly as
    /// [`Item::Derive`] is.
    EffectSet(Box<EffectSetDef>),
}

impl Item {
    pub fn span(&self) -> Span {
        match self {
            Item::Fn(d) => d.span,
            Item::Type(d) => d.span,
            Item::Effect(d) => d.span,
            Item::Test(d) => d.span,
            Item::Law(d) => d.span,
            Item::Derive(d) => d.span,
            Item::EffectSet(d) => d.span,
        }
    }

    /// `None` for a `test`, a `law`, a `derive` and an `effect set`, none of
    /// which have a name a reference could reach. A `derive`'s generated
    /// definitions do, and they are [`Item::Fn`]s of their own; an `effect
    /// set`'s name is consumed by the parser and lives in no namespace
    /// `resolve` knows about.
    pub fn name(&self) -> Option<&Ident> {
        match self {
            Item::Fn(d) => Some(&d.name),
            Item::Type(d) => Some(&d.name),
            Item::Effect(d) => Some(&d.name),
            Item::Test(_) | Item::Law(_) | Item::Derive(_) | Item::EffectSet(_) => None,
        }
    }

    /// A `derive` carries no `pub` of its own: its generated definitions take
    /// the target type's visibility, so a type you can name is a type you can
    /// encode and the two cannot drift.
    pub fn visibility(&self) -> Visibility {
        match self {
            Item::Fn(d) => d.vis,
            Item::Type(d) => d.vis,
            Item::Effect(d) => d.vis,
            Item::Test(_) | Item::Law(_) | Item::Derive(_) | Item::EffectSet(_) => {
                Visibility::Private
            }
        }
    }
}

/// `effect set Web = {db.read[users], log.write, Inner}`.
///
/// An abbreviation for a row, and nothing more. Its name is namespace metadata
/// — erased by normalization — while its [`expansion`] enters a hash exactly as
/// the row it stands for would, because that expansion is the published upper
/// bound a caller is checked against.
///
/// A member is an **atom** or another set, never a whole effect. "Every atom of
/// `db`" is every resource label anywhere in the program, so an unrelated table
/// in an unrelated module would change the expansion — and therefore the
/// declared row, and therefore the hash — of every definition annotated with
/// this set, which is exactly the rule that nothing outside a definition's own
/// reachable graph may enter its hash.
///
/// Sets are **module-local**: [`includes`] is a [`QName`] only so that a
/// qualified reference can be refused with a diagnostic that says why. Gate 1
/// skips a file whose raw bytes are unchanged, so a set expanding across a
/// module boundary would let an edit in the declaring module leave a stale
/// published row behind — a footprint that under-reports, which is a green
/// result rather than a loud one.
///
/// [`expansion`]: EffectSetDef::expansion
/// [`includes`]: EffectSetDef::includes
#[derive(Clone, Debug)]
pub struct EffectSetDef {
    pub name: Ident,
    /// Members written as atoms, in source order.
    pub atoms: Vec<AtomExpr>,
    /// Members naming another set, in source order.
    pub includes: Vec<QName>,
    /// Every atom this set denotes, after expanding `includes` transitively:
    /// sorted and deduplicated by written form.
    ///
    /// Sorted rather than left in the order the members were written, so that
    /// reordering them — or splitting one set into two — produces the same
    /// expansion. That is what a reader diffing two `--explain` outputs sees,
    /// and it is one fewer thing they have to know the row encoder fixes up
    /// later.
    pub expansion: Vec<AtomExpr>,
    pub span: Span,
}

/// The derivations the language defines. Fixed: there are no user-defined
/// derivers, and `row` waits for W4's `Row` type rather than shipping as a
/// codec over a type that does not exist.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Deriver {
    Json,
    Eq,
    Ord,
}

impl Deriver {
    pub const ALL: &'static [Deriver] = &[Deriver::Json, Deriver::Eq, Deriver::Ord];

    pub fn from_name(name: &str) -> Option<Deriver> {
        Some(match name {
            "json" => Deriver::Json,
            "eq" => Deriver::Eq,
            "ord" => Deriver::Ord,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Deriver::Json => "json",
            Deriver::Eq => "eq",
            Deriver::Ord => "ord",
        }
    }

    /// The dictionary type a derivation of this kind produces.
    pub fn dictionary(self) -> &'static str {
        match self {
            Deriver::Json => "JsonCodec",
            Deriver::Eq => "EqDict",
            Deriver::Ord => "OrdDict",
        }
    }

    /// Distinguishes derivers in a definition hash and in a stored body. Pinned
    /// rather than derived from the variant order, which is a cache key nobody
    /// should be able to move by sorting an enum.
    pub fn tag(self) -> u8 {
        match self {
            Deriver::Json => 1,
            Deriver::Eq => 2,
            Deriver::Ord => 3,
        }
    }

    pub fn from_tag(tag: u8) -> Option<Deriver> {
        Some(match tag {
            1 => Deriver::Json,
            2 => Deriver::Eq,
            3 => Deriver::Ord,
            _ => return None,
        })
    }
}

impl fmt::Display for Deriver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// `derive json for Order`.
///
/// The target is an [`Ident`] and not a [`QName`] on purpose: a `derive` may
/// only name a type its own module declares. Without that rule two modules can
/// each derive for one type and produce two names for one canonical encoding,
/// which is the divergence ADR 0010 has no resolution layer to prevent.
#[derive(Clone, Debug)]
pub struct DeriveDef {
    pub deriver: Deriver,
    pub deriver_span: Span,
    pub target: Ident,
    pub span: Span,
}

/// `where derivable(json, a)` on a signature.
///
/// Checked at the **signature** rather than at instantiation, which is the one
/// axis on which bare structural reflection is genuinely worse than typeclasses:
/// an error deep inside an expansion is a search, and a search inside an edit
/// loop is what an agent actually pays for.
#[derive(Clone, Debug)]
pub struct Constraint {
    pub deriver: Deriver,
    pub deriver_span: Span,
    /// A type parameter of the enclosing signature. Never a concrete type: a
    /// constraint on a type the compiler can already see is either trivially
    /// true or an error, and neither is worth writing.
    pub param: Ident,
    pub span: Span,
}

/// What a generated definition was generated from. Provenance, so `--explain`
/// and `ply check --types` can label it — and erased by normalization, because
/// a hand-written definition byte-identical to a generated one is the same
/// computation and must share its hash.
#[derive(Clone, Debug)]
pub struct Derived {
    pub deriver: Deriver,
    /// The target's simple name, as its `derive` wrote it.
    pub target: Symbol,
}

/// Type parameters and effect-row parameters, written `<a, b | e>`.
#[derive(Clone, Debug, Default)]
pub struct Generics {
    pub types: Vec<Ident>,
    pub effects: Vec<Ident>,
}

#[derive(Clone, Debug)]
pub struct Param {
    pub name: Ident,
    pub ty: Option<TypeExpr>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct FnDef {
    pub vis: Visibility,
    pub name: Ident,
    pub generics: Generics,
    pub params: Vec<Param>,
    pub ret: Option<TypeExpr>,
    /// The `/ {...}` annotation. When present it is the published signature and
    /// inference must produce a subset of it; when absent the row is inferred.
    pub effects: Option<RowExpr>,
    /// `where derivable(json, a), derivable(ord, k)`, written after the effect
    /// row and before any `requires`.
    ///
    /// Part of the published signature and therefore **kept** by normalization,
    /// unlike `spec`. Adding a constraint narrows the call sites the signature
    /// admits, so a caller checked against the unconstrained form has to be
    /// rechecked — and gate 2 only rechecks a definition whose dependency's
    /// hash moved. Erasing this would leave that caller accepted against a
    /// signature that no longer admits it.
    pub constraints: Vec<Constraint>,
    /// Set on a definition expansion generated from a `derive`, and `None` on
    /// everything a human wrote. Provenance only: erased by normalization.
    pub derived: Option<Derived>,
    /// `requires` / `ensures`, in source order. A spec is a claim *about* this
    /// definition rather than part of it, so it is erased by normalization:
    /// writing one changes no definition hash and re-runs no test. The claim
    /// gets its own hash, which covers this definition's.
    pub spec: Vec<SpecClause>,
    pub body: Expr,
    pub span: Span,
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

    /// Distinguishes the two in a spec hash. Part of a cache key, so it is
    /// pinned rather than derived from the variant order.
    pub fn tag(self) -> u8 {
        match self {
            SpecKind::Requires => 1,
            SpecKind::Ensures => 2,
        }
    }
}

/// `requires amount > 0`, `ensures result.balance == acct.balance - amount`.
///
/// The expression's row must be empty: a claim that can perform effects can
/// change what it observes. `result` is bound only in an `ensures`.
#[derive(Clone, Debug)]
pub struct SpecClause {
    pub kind: SpecKind,
    pub expr: Expr,
    pub span: Span,
}

/// A `forall` binder. Its type is mandatory — inferring it would make a law's
/// meaning depend on how its body happened to be written — so this is not a
/// [`Param`].
#[derive(Clone, Debug)]
pub struct Binder {
    pub name: Ident,
    pub ty: TypeExpr,
    pub span: Span,
}

/// ```text
/// law "credit and debit cancel"
///   forall (a: Account, n: Int) where n > 0 && n <= a.balance {
///     credited(debited(a, n), n) == a
///   }
/// ```
///
/// `guard`'s row must be empty. `body`'s row must be empty too, unless it is
/// exactly `{sim.read}`, which makes this a concurrency law discharged by
/// exhaustive interleaving search rather than by a static argument — or unless
/// the law is declared `law/host`, which is [`host`](LawDef::host).
#[derive(Clone, Debug)]
pub struct LawDef {
    pub name: String,
    pub name_span: Span,
    /// `law/host`: the **body** may carry any row, and the law is then a claim
    /// about the world rather than about the program alone.
    ///
    /// Declared rather than inferred, for the reason every other relaxation in
    /// this language is declared: a law that could touch the world without
    /// saying so is the one shape a reader cannot audit. Three things follow
    /// from it and each is enforced somewhere else — it can never be `proved`
    /// (`ply-prove` refuses to lower a body whose row is non-empty), it is never
    /// cached in either direction, and under a hermetic run it is reported
    /// `W0604 unattempted` rather than green.
    ///
    /// The **guard** is unaffected: it stays pure under `E0417`, because a guard
    /// decides the domain and a guard that could act would be choosing which
    /// cases to be judged on.
    pub host: bool,
    /// Empty for a ground law, which is a claim over a domain of one point and
    /// is therefore decided by evaluating it.
    pub binders: Vec<Binder>,
    pub guard: Option<Expr>,
    pub body: Expr,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct TypeDef {
    pub vis: Visibility,
    pub name: Ident,
    pub params: Vec<Ident>,
    pub body: TypeDefBody,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum TypeDefBody {
    Alias(TypeExpr),
    Sum(Vec<VariantDef>),
}

#[derive(Clone, Debug)]
pub struct VariantDef {
    pub name: Ident,
    pub fields: Vec<TypeExpr>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct EffectDef {
    pub vis: Visibility,
    pub name: Ident,
    pub nondet: bool,
    pub ops: Vec<OpDef>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct OpDef {
    pub name: Ident,
    pub mode: Mode,
    /// Declared as `op[r](..)`: call sites must supply a resource label, and the
    /// atom performed is keyed by it.
    pub resource_param: bool,
    pub params: Vec<TypeExpr>,
    pub ret: TypeExpr,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct TestDef {
    pub name: String,
    pub name_span: Span,
    /// `test/nondet`: exempt from the determinism check and never cached.
    pub nondet: bool,
    pub body: Expr,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum TypeExpr {
    /// A type parameter. Never module-qualified — it is bound by the enclosing
    /// `<..>`, not by any module.
    Var(Ident),
    Con {
        name: QName,
        args: Vec<TypeExpr>,
        span: Span,
    },
    Fn {
        params: Vec<TypeExpr>,
        ret: Box<TypeExpr>,
        effects: Option<RowExpr>,
        span: Span,
    },
    Record {
        fields: Vec<(Ident, TypeExpr)>,
        span: Span,
    },
    Unit {
        span: Span,
    },
}

impl TypeExpr {
    pub fn span(&self) -> Span {
        match self {
            TypeExpr::Var(i) => i.span,
            TypeExpr::Con { span, .. }
            | TypeExpr::Fn { span, .. }
            | TypeExpr::Record { span, .. }
            | TypeExpr::Unit { span } => *span,
        }
    }
}

/// A written effect row: `{db.read[users], clock.read | e}`, or
/// `{Web, random.read}` naming an [`EffectSetDef`].
///
/// `atoms` is always complete: [`crate::parse_module`] expands every set before
/// it returns, so an unexpanded row never escapes the parser and no crate can
/// forget to run the expander.
#[derive(Clone, Debug)]
pub struct RowExpr {
    pub atoms: Vec<AtomExpr>,
    /// The `effect set`s this row was written with, in source order.
    ///
    /// Provenance for `--explain` and nothing else: **erased by
    /// normalization**, so a row written `{Web}` and one written with `Web`'s
    /// expansion are the same definition and share a hash. A qualified name is
    /// representable only so that it can be refused with a diagnostic saying
    /// sets are module-local.
    pub aliases: Vec<QName>,
    pub tail: Option<Ident>,
    pub span: Span,
}

/// `db.read[users]`, or `store::db.read[users]` for an imported effect.
///
/// The resource label is deliberately not namespaced: two modules writing
/// `[users]` name the same resource, and must, or the scheduler would run
/// contending tests concurrently.
#[derive(Clone, Debug)]
pub struct AtomExpr {
    pub effect: QName,
    pub mode: Mode,
    pub resource: Option<Ident>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Lit {
    Int(i64),
    Bool(bool),
    Str(String),
    /// `b"GET "`. ASCII plus `\xNN`: a source character above `U+007F` is
    /// refused, because the bytes of this literal may not depend on the file's
    /// encoding.
    Bytes(Vec<u8>),
    /// IEEE-754 binary64. Two of these are one definition iff their **bit
    /// patterns** agree, which is why `0.0` and `-0.0` are two definitions and
    /// why a normalizer that folded them would be wrong: `1.0 / -0.0` still
    /// tells them apart.
    Float(f64),
    /// Sign and magnitude in `mantissa`, digits after the point in `scale`.
    /// The scale is **kept**: `1.50m` is `(150, 2)` and `1.5m` is `(15, 1)`, so
    /// the two are equal in value and differently hashed. Both consequences of
    /// one decision, and both stated rather than smoothed over.
    Decimal {
        mantissa: i128,
        scale: u32,
    },
    Unit,
}

/// The shortest text that reads back as this `f64`, always distinguishable from
/// an integer. `{}` on an `f64` is already shortest-round-tripping; what it does
/// not do is keep `1` and `1.0` apart, and those are two types here.
pub fn render_float(f: f64) -> String {
    if f.is_nan() {
        return "NaN".to_string();
    }
    if f.is_infinite() {
        return if f > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    // Rust's `{}` is shortest-round-tripping in *digits* but always positional,
    // so `1e300` comes back as three hundred and one characters. `{:e}` is the
    // same value in the other notation; taking the shorter of the two is what
    // "shortest" means, and positional wins a tie because it is the form
    // somebody wrote.
    let positional = format!("{f}");
    let exponential = format!("{f:e}");
    let text = if exponential.len() < positional.len() {
        exponential
    } else {
        positional
    };
    if text.contains(['.', 'e', 'E']) {
        text
    } else {
        format!("{text}.0")
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    Concat,
}

impl BinOp {
    /// The operator as it is written, for a diagnostic that quotes it.
    pub fn text(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Rem => "%",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::And => "&&",
            BinOp::Or => "||",
            BinOp::Concat => "++",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Debug)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

/// Hand-written so the copy grows the host stack once per level, as parsing,
/// inference and normalization already do. A derived `Clone` recurses to the
/// depth of the expression on whatever stack the caller happens to have, and the
/// tree-walker copies a body per closure on a worker thread — so a chain of
/// operators the front end accepts could abort the run rather than evaluate.
impl Clone for Expr {
    fn clone(&self) -> Expr {
        const RED_ZONE: usize = 256 * 1024;
        const NEW_SEGMENT: usize = 2 * 1024 * 1024;
        stacker::maybe_grow(RED_ZONE, NEW_SEGMENT, || Expr {
            kind: self.kind.clone(),
            span: self.span,
        })
    }
}

#[derive(Clone, Debug)]
pub enum ExprKind {
    Lit(Lit),
    Var(QName),
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Unary {
        op: UnOp,
        operand: Box<Expr>,
    },
    Lambda {
        params: Vec<Param>,
        body: Box<Expr>,
    },
    App {
        func: Box<Expr>,
        args: Vec<Expr>,
    },
    If {
        cond: Box<Expr>,
        then_branch: Box<Expr>,
        else_branch: Box<Expr>,
    },
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm>,
    },
    Block {
        stmts: Vec<Stmt>,
        tail: Option<Box<Expr>>,
    },
    Record {
        fields: Vec<(Ident, Expr)>,
    },
    Field {
        base: Box<Expr>,
        field: Ident,
    },
    List {
        items: Vec<Expr>,
    },

    /// `db.get[users](key)`, or `store::db.get[users](key)`.
    Perform {
        effect: QName,
        op: Ident,
        resource: Option<Ident>,
        args: Vec<Expr>,
    },

    Handle {
        body: Box<Expr>,
        clauses: Vec<HandleClause>,
        return_clause: Option<Box<ReturnClause>>,
    },

    /// `with_cell[users](init) { c -> body }`. A builtin rather than a
    /// user-level effect so its atoms are discharged at the region boundary and
    /// provably cannot escape.
    WithCell {
        resource: Ident,
        init: Box<Expr>,
        binder: Ident,
        body: Box<Expr>,
    },

    /// `simulate { body }`. A `handle` with a fixed clause set: it installs the
    /// seeded scheduler over `task`, `clock` and `random`, and its own row gains
    /// `sim.read`, the seed dependency. There is no seed in the syntax — one
    /// written in source would be part of the definition's hash, making every
    /// seed a different definition.
    Simulate {
        body: Box<Expr>,
    },
}

#[derive(Clone, Debug)]
pub struct MatchArm {
    pub pat: Pattern,
    pub guard: Option<Expr>,
    pub body: Expr,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum Stmt {
    Let {
        pat: Pattern,
        ty: Option<TypeExpr>,
        value: Box<Expr>,
        span: Span,
    },
    Expr(Expr),
}

#[derive(Clone, Debug)]
pub struct HandleClause {
    pub effect: QName,
    pub op: Ident,
    pub resource: Option<Ident>,
    pub params: Vec<Ident>,
    /// `op(x) resume k -> ...` binds the delimited continuation as `k`, and the
    /// clause's body then has the whole `handle`'s type rather than the
    /// operation's. `None` is the tail-resumptive form, where the body's value
    /// goes straight back to the perform site — which is `op(x) resume k ->
    /// k(e)` with the resumption supplied by the machine.
    pub resume: Option<Ident>,
    pub body: Expr,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct ReturnClause {
    pub binder: Ident,
    pub body: Expr,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct Pattern {
    pub kind: PatternKind,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum PatternKind {
    Wildcard,
    Var(Ident),
    Lit(Lit),
    Ctor {
        name: QName,
        args: Vec<Pattern>,
    },
    Record {
        fields: Vec<(Ident, Pattern)>,
        rest: bool,
    },
    List {
        items: Vec<Pattern>,
        rest: Option<Box<Pattern>>,
    },
}
