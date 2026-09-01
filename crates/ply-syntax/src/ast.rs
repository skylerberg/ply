//! Pinned: downstream crates are written against these shapes concurrently, so changing a variant
//! is a cross-crate breaking change.

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

/// A reference to a top-level name, optionally qualified by a module binder: `place` or
/// `orders::place`.
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

/// A module's dotted name, derived from its file's path relative to the project root:
/// `store/orders.ply` is `store.orders`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ModuleName(Symbol);

impl Default for ModuleName {
    fn default() -> Self {
        ModuleName::anonymous()
    }
}

impl ModuleName {
    /// The module of source that has no project root: a snippet handed to [`crate::parse`].
    pub fn anonymous() -> ModuleName {
        ModuleName(Symbol::new(""))
    }

    pub fn is_anonymous(&self) -> bool {
        self.0.as_str().is_empty()
    }

    /// Every directory component and the file stem must be a Ply identifier; anything else is
    /// [`codes::INVALID_MODULE_PATH`].
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

/// One file, one module.
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

/// Every module in the project.
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

/// `import store.orders`, `import store.orders as ord`, `import store.orders (place, cancel)`.
#[derive(Clone, Debug)]
pub struct ImportDecl {
    /// One `Ident` per dotted segment, so a diagnostic can point at the segment that went wrong
    /// rather than the whole declaration.
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

    /// The module binder this import introduces, if any.
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

/// Boxed because a `fn` is several times the size of the other variants and a module holds one
/// `Vec<Item>` per file: unboxed, a program of ten thousand definitions pays the largest variant's
/// width for every item it has.
#[derive(Clone, Debug)]
pub enum Item {
    Fn(Box<FnDef>),
    Type(Box<TypeDef>),
    Effect(Box<EffectDef>),
    Test(Box<TestDef>),
    /// `law "label" forall (x: T) where g { body }`.
    Law(Box<LawDef>),
    /// `derive json for Order`.
    Derive(Box<DeriveDef>),
    /// `effect set Web = {db.read[users], log.write}`.
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

    /// `None` for a `test`, a `law`, a `derive` and an `effect set`, none of which have a name a
    /// reference could reach.
    pub fn name(&self) -> Option<&Ident> {
        match self {
            Item::Fn(d) => Some(&d.name),
            Item::Type(d) => Some(&d.name),
            Item::Effect(d) => Some(&d.name),
            Item::Test(_) | Item::Law(_) | Item::Derive(_) | Item::EffectSet(_) => None,
        }
    }

    /// A `derive` carries no `pub` of its own: its generated definitions take the target type's
    /// visibility, so a type you can name is a type you can encode and the two cannot drift.
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
#[derive(Clone, Debug)]
pub struct EffectSetDef {
    pub name: Ident,
    /// Members written as atoms, in source order.
    pub atoms: Vec<AtomExpr>,
    /// Members naming another set, in source order.
    pub includes: Vec<QName>,
    /// Every atom this set denotes, after expanding `includes` transitively: sorted and
    /// deduplicated by written form.
    pub expansion: Vec<AtomExpr>,
    pub span: Span,
}

/// The derivations the language defines.
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

    /// Distinguishes derivers in a definition hash and in a stored body.
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
#[derive(Clone, Debug)]
pub struct DeriveDef {
    pub deriver: Deriver,
    pub deriver_span: Span,
    pub target: Ident,
    pub span: Span,
}

/// `where derivable(json, a)` on a signature.
#[derive(Clone, Debug)]
pub struct Constraint {
    pub deriver: Deriver,
    pub deriver_span: Span,
    /// A type parameter of the enclosing signature.
    pub param: Ident,
    pub span: Span,
}

/// What a generated definition was generated from.
#[derive(Clone, Debug)]
pub struct Derived {
    pub deriver: Deriver,
    /// The target's simple name, as its `derive` wrote it.
    pub target: Symbol,
}

/// One `name: value` argument at a call site.
#[derive(Clone, Debug)]
pub struct NamedArg {
    pub name: Ident,
    pub value: Expr,
    pub span: Span,
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
    /// What a call that does not fill this parameter passes instead.
    pub default: Option<Expr>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct FnDef {
    pub vis: Visibility,
    pub name: Ident,
    pub generics: Generics,
    pub params: Vec<Param>,
    pub ret: Option<TypeExpr>,
    /// The `/ {...}` annotation.
    pub effects: Option<RowExpr>,
    /// `where derivable(json, a), derivable(ord, k)`, written after the effect row and before any
    /// `requires`.
    pub constraints: Vec<Constraint>,
    /// Set on a definition expansion generated from a `derive`, and `None` on everything a human
    /// wrote.
    pub derived: Option<Derived>,
    /// `requires` / `ensures`, in source order.
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

    /// Distinguishes the two in a spec hash.
    pub fn tag(self) -> u8 {
        match self {
            SpecKind::Requires => 1,
            SpecKind::Ensures => 2,
        }
    }
}

/// `requires amount > 0`, `ensures result.balance == acct.balance - amount`.
#[derive(Clone, Debug)]
pub struct SpecClause {
    pub kind: SpecKind,
    pub expr: Expr,
    pub span: Span,
}

/// A `forall` binder.
#[derive(Clone, Debug)]
pub struct Binder {
    pub name: Ident,
    pub ty: TypeExpr,
    pub span: Span,
}

/// ```text law "credit and debit cancel" forall (a: Account, n: Int) where n > 0 && n <= a.balance
/// { credited(debited(a, n), n) == a } ```.
#[derive(Clone, Debug)]
pub struct LawDef {
    pub name: String,
    pub name_span: Span,
    /// `law/host`: the **body** may carry any row, and the law is then a claim about the world
    /// rather than about the program alone.
    pub host: bool,
    /// Empty for a ground law, which is a claim over a domain of one point and is therefore decided
    /// by evaluating it.
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
    /// Declared as `op[r](..)`: call sites must supply a resource label, and the atom performed is
    /// keyed by it.
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
    /// A type parameter.
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

/// A written effect row: `{db.read[users], clock.read | e}`, or `{Web, random.read}` naming an
/// [`EffectSetDef`].
#[derive(Clone, Debug)]
pub struct RowExpr {
    pub atoms: Vec<AtomExpr>,
    /// The `effect set`s this row was written with, in source order.
    pub aliases: Vec<QName>,
    pub tail: Option<Ident>,
    pub span: Span,
}

/// `db.read[users]`, or `store::db.read[users]` for an imported effect.
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
    /// `b"GET "`.
    Bytes(Vec<u8>),
    /// IEEE-754 binary64.
    Float(f64),
    /// Sign and magnitude in `mantissa`, digits after the point in `scale`.
    Decimal {
        mantissa: i128,
        scale: u32,
    },
    Unit,
}

/// The shortest text that reads back as this `f64`, always distinguishable from an integer.
pub fn render_float(f: f64) -> String {
    if f.is_nan() {
        return "NaN".to_string();
    }
    if f.is_infinite() {
        return if f > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    // Rust's `{}` is shortest-round-tripping in *digits* but always positional, so `1e300` comes
    // back as three hundred and one characters.
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
    /// The bit operators of `docs/adr/0033-bits-and-files.md` §2, defined at
    /// `Int` only. `Shl` discards what it shifts out — the one deliberate
    /// exception to checked arithmetic — and a shift count outside `0..=63`
    /// raises. Appended rather than filed beside `And`/`Or` so that
    /// `ply_hash::normalize::binop_byte` can append its bytes too: an existing
    /// byte that moves is every cached result invalidated.
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    Ushr,
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
            BinOp::BitAnd => "&",
            BinOp::BitOr => "|",
            BinOp::BitXor => "^",
            BinOp::Shl => "<<",
            BinOp::Shr => ">>",
            BinOp::Ushr => ">>>",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UnOp {
    Neg,
    Not,
    /// Prefix `~`: the two's-complement complement of an `Int`. `!` stays
    /// `Bool`-only, so neither operator can be written where the other is meant
    /// (ADR 0033 §2.2).
    BitNot,
}

#[derive(Debug)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

/// Hand-written so the copy grows the host stack once per level, as parsing, inference and
/// normalization already do.
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
        /// Arguments written `name: value`, which follow every positional one.
        named: Vec<NamedArg>,
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

    /// `{..base, f: e}` — **parse-time only**.
    RecordUpdate {
        base: Box<Expr>,
        fields: Vec<(Ident, Expr)>,
    },

    Field {
        base: Box<Expr>,
        field: Ident,
    },

    /// `e?` — **parse-time only**, exactly as [`ExprKind::RecordUpdate`] is.
    Try {
        operand: Box<Expr>,
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

    /// `with_cell[users](init) { c -> body }`.
    WithCell {
        resource: Ident,
        init: Box<Expr>,
        binder: Ident,
        body: Box<Expr>,
    },

    /// `with_region[r] { body }`.
    WithRegion {
        region: Ident,
        body: Box<Expr>,
    },

    /// `simulate { body }`.
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
    /// `op(x) resume k -> ...` binds the delimited continuation as `k`, and the clause's body then
    /// has the whole `handle`'s type rather than the operation's.
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

/// Evaluates without calling anything and without performing anything, so it cannot diverge and
/// cannot be observed by, or observe, its neighbours.
pub fn is_pure(e: &Expr) -> bool {
    crate::effect_set::grow(|| match &e.kind {
        ExprKind::App { .. }
        | ExprKind::Perform { .. }
        | ExprKind::Handle { .. }
        | ExprKind::WithCell { .. }
        | ExprKind::WithRegion { .. }
        | ExprKind::Simulate { .. } => false,
        // Conservative, and only ever consulted before expansion: a `?` that is being asked about
        // here is one the scan did not reach in evaluation order, which means it sits behind a
        // conditional or inside a nested block, and both of those are refused.
        ExprKind::Try { .. } => false,
        ExprKind::Lit(_) | ExprKind::Var(_) => true,
        ExprKind::Binary { lhs, rhs, .. } => is_pure(lhs) && is_pure(rhs),
        ExprKind::Unary { operand, .. } => is_pure(operand),
        ExprKind::Lambda { body, .. } => is_pure(body),
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => is_pure(cond) && is_pure(then_branch) && is_pure(else_branch),
        ExprKind::Match { scrutinee, arms } => {
            is_pure(scrutinee)
                && arms
                    .iter()
                    .all(|a| is_pure(&a.body) && a.guard.as_ref().is_none_or(is_pure))
        }
        ExprKind::Block { stmts, tail } => {
            stmts.iter().all(|s| match s {
                Stmt::Let { value, .. } => is_pure(value),
                Stmt::Expr(e) => is_pure(e),
            }) && tail.as_deref().is_none_or(is_pure)
        }
        ExprKind::Record { fields } => fields.iter().all(|(_, v)| is_pure(v)),
        ExprKind::RecordUpdate { base, fields } => {
            is_pure(base) && fields.iter().all(|(_, v)| is_pure(v))
        }
        ExprKind::Field { base, .. } => is_pure(base),
        ExprKind::List { items } => items.iter().all(is_pure),
    })
}

/// Whether an expression may be a parameter's default: [`is_pure`], widened to admit a
/// *constructor* application.
pub fn is_default_expr(e: &Expr) -> bool {
    crate::effect_set::grow(|| match &e.kind {
        ExprKind::App { func, args, named } => {
            named.is_empty()
                && matches!(&func.kind, ExprKind::Var(q) if is_ctor_name(q.symbol()))
                && args.iter().all(is_default_expr)
        }
        ExprKind::Perform { .. }
        | ExprKind::Handle { .. }
        | ExprKind::WithCell { .. }
        | ExprKind::WithRegion { .. }
        | ExprKind::Simulate { .. }
        | ExprKind::Try { .. } => false,
        ExprKind::Lit(_) | ExprKind::Var(_) => true,
        ExprKind::Binary { lhs, rhs, .. } => is_default_expr(lhs) && is_default_expr(rhs),
        ExprKind::Unary { operand, .. } => is_default_expr(operand),
        ExprKind::Lambda { body, .. } => is_default_expr(body),
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => is_default_expr(cond) && is_default_expr(then_branch) && is_default_expr(else_branch),
        ExprKind::Match { scrutinee, arms } => {
            is_default_expr(scrutinee)
                && arms.iter().all(|a| {
                    is_default_expr(&a.body) && a.guard.as_ref().is_none_or(is_default_expr)
                })
        }
        ExprKind::Block { stmts, tail } => {
            stmts.iter().all(|s| match s {
                Stmt::Let { value, .. } => is_default_expr(value),
                Stmt::Expr(e) => is_default_expr(e),
            }) && tail.as_deref().is_none_or(is_default_expr)
        }
        ExprKind::Record { fields } => fields.iter().all(|(_, v)| is_default_expr(v)),
        ExprKind::RecordUpdate { base, fields } => {
            is_default_expr(base) && fields.iter().all(|(_, v)| is_default_expr(v))
        }
        ExprKind::Field { base, .. } => is_default_expr(base),
        ExprKind::List { items } => items.iter().all(is_default_expr),
    })
}

/// The grammar's constructor rule: a leading uppercase letter.
pub fn is_ctor_name(name: &Symbol) -> bool {
    name.as_str().chars().next().is_some_and(char::is_uppercase)
}
