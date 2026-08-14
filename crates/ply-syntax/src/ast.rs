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
}

impl Item {
    pub fn span(&self) -> Span {
        match self {
            Item::Fn(d) => d.span,
            Item::Type(d) => d.span,
            Item::Effect(d) => d.span,
            Item::Test(d) => d.span,
        }
    }

    /// `None` for a `test`, which has no name a reference could reach.
    pub fn name(&self) -> Option<&Ident> {
        match self {
            Item::Fn(d) => Some(&d.name),
            Item::Type(d) => Some(&d.name),
            Item::Effect(d) => Some(&d.name),
            Item::Test(_) => None,
        }
    }

    pub fn visibility(&self) -> Visibility {
        match self {
            Item::Fn(d) => d.vis,
            Item::Type(d) => d.vis,
            Item::Effect(d) => d.vis,
            Item::Test(_) => Visibility::Private,
        }
    }
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

/// A written effect row: `{db.read[users], clock.read | e}`.
#[derive(Clone, Debug)]
pub struct RowExpr {
    pub atoms: Vec<AtomExpr>,
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
    Unit,
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
