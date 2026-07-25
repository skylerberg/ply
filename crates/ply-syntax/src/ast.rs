//! Pinned: downstream crates are written against these shapes concurrently, so
//! changing a variant is a cross-crate breaking change.

use ply_span::{Span, Symbol};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ident {
    pub name: Symbol,
    pub span: Span,
}

impl Ident {
    pub fn new(name: impl Into<Symbol>, span: Span) -> Self {
        Ident { name: name.into(), span }
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

#[derive(Clone, Debug, Default)]
pub struct Module {
    pub items: Vec<Item>,
}

#[derive(Clone, Debug)]
pub enum Item {
    Fn(FnDef),
    Type(TypeDef),
    Effect(EffectDef),
    Test(TestDef),
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
    Var(Ident),
    Con { name: Ident, args: Vec<TypeExpr>, span: Span },
    Fn { params: Vec<TypeExpr>, ret: Box<TypeExpr>, effects: Option<RowExpr>, span: Span },
    Record { fields: Vec<(Ident, TypeExpr)>, span: Span },
    Unit { span: Span },
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

#[derive(Clone, Debug)]
pub struct AtomExpr {
    pub effect: Ident,
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

#[derive(Clone, Debug)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum ExprKind {
    Lit(Lit),
    Var(Ident),
    Binary { op: BinOp, lhs: Box<Expr>, rhs: Box<Expr> },
    Unary { op: UnOp, operand: Box<Expr> },
    Lambda { params: Vec<Param>, body: Box<Expr> },
    App { func: Box<Expr>, args: Vec<Expr> },
    If { cond: Box<Expr>, then_branch: Box<Expr>, else_branch: Box<Expr> },
    Match { scrutinee: Box<Expr>, arms: Vec<MatchArm> },
    Block { stmts: Vec<Stmt>, tail: Option<Box<Expr>> },
    Record { fields: Vec<(Ident, Expr)> },
    Field { base: Box<Expr>, field: Ident },
    List { items: Vec<Expr> },

    /// `db.get[users](key)`
    Perform { effect: Ident, op: Ident, resource: Option<Ident>, args: Vec<Expr> },

    Handle { body: Box<Expr>, clauses: Vec<HandleClause>, return_clause: Option<Box<ReturnClause>> },

    /// `with_cell[users](init) { c -> body }`. A builtin rather than a
    /// user-level effect so its atoms are discharged at the region boundary and
    /// provably cannot escape.
    WithCell { resource: Ident, init: Box<Expr>, binder: Ident, body: Box<Expr> },
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
    Let { pat: Pattern, ty: Option<TypeExpr>, value: Expr, span: Span },
    Expr(Expr),
}

#[derive(Clone, Debug)]
pub struct HandleClause {
    pub effect: Ident,
    pub op: Ident,
    pub resource: Option<Ident>,
    pub params: Vec<Ident>,
    /// The clause body's value is returned to the perform site. Tail-resumptive
    /// only in v0, so no continuation is reified.
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
    Ctor { name: Ident, args: Vec<Pattern> },
    Record { fields: Vec<(Ident, Pattern)>, rest: bool },
    List { items: Vec<Pattern>, rest: Option<Box<Pattern>> },
}
