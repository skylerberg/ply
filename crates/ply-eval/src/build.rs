//! AST constructors for the tests.

use ply_span::{SourceId, Span};
use ply_syntax::ast::*;
use ply_syntax::resolve::{Resolved, resolve};

pub fn sp() -> Span {
    Span::new(SourceId(0), 0, 1)
}

pub fn at(start: u32, end: u32) -> Span {
    Span::new(SourceId(0), start, end)
}

pub fn id(name: &str) -> Ident {
    Ident::new(name, sp())
}

pub fn ex(kind: ExprKind) -> Expr {
    Expr { kind, span: sp() }
}

pub fn spanned(mut e: Expr, span: Span) -> Expr {
    e.span = span;
    e
}

pub fn int(i: i64) -> Expr {
    ex(ExprKind::Lit(Lit::Int(i)))
}

pub fn boolean(b: bool) -> Expr {
    ex(ExprKind::Lit(Lit::Bool(b)))
}

pub fn string(s: &str) -> Expr {
    ex(ExprKind::Lit(Lit::Str(s.to_string())))
}

pub fn bytes(b: &[u8]) -> Expr {
    ex(ExprKind::Lit(Lit::Bytes(b.to_vec())))
}

pub fn unit() -> Expr {
    ex(ExprKind::Lit(Lit::Unit))
}

pub fn qname(name: &str) -> QName {
    QName::bare(id(name))
}

pub fn var(name: &str) -> Expr {
    ex(ExprKind::Var(qname(name)))
}

pub fn var_at(name: &str, span: Span) -> Expr {
    Expr {
        kind: ExprKind::Var(QName::bare(Ident::new(name, span))),
        span,
    }
}

pub fn call(func: Expr, args: Vec<Expr>) -> Expr {
    ex(ExprKind::App {
        func: Box::new(func),
        args,
        named: Vec::new(),
    })
}

pub fn callv(name: &str, args: Vec<Expr>) -> Expr {
    call(var(name), args)
}

pub fn bin(op: BinOp, lhs: Expr, rhs: Expr) -> Expr {
    ex(ExprKind::Binary {
        op,
        lhs: Box::new(lhs),
        rhs: Box::new(rhs),
    })
}

pub fn un(op: UnOp, operand: Expr) -> Expr {
    ex(ExprKind::Unary {
        op,
        operand: Box::new(operand),
    })
}

pub fn if_(cond: Expr, then_branch: Expr, else_branch: Expr) -> Expr {
    ex(ExprKind::If {
        cond: Box::new(cond),
        then_branch: Box::new(then_branch),
        else_branch: Box::new(else_branch),
    })
}

pub fn param(name: &str) -> Param {
    Param {
        name: id(name),
        ty: None,
        default: None,
        span: sp(),
    }
}

/// `name: ty`. A `fn` parameter has to be written now, so every parameter of a
/// definition that will be checked comes through here.
pub fn param_ty(name: &str, ty: TypeExpr) -> Param {
    Param {
        ty: Some(ty),
        ..param(name)
    }
}

pub fn lam(params: &[&str], body: Expr) -> Expr {
    ex(ExprKind::Lambda {
        params: params.iter().map(|p| param(p)).collect(),
        body: Box::new(body),
    })
}

pub fn block(stmts: Vec<Stmt>, tail: Option<Expr>) -> Expr {
    ex(ExprKind::Block {
        stmts,
        tail: tail.map(Box::new),
    })
}

pub fn let_(pat: Pattern, value: Expr) -> Stmt {
    Stmt::Let {
        pat,
        ty: None,
        value: Box::new(value),
        span: sp(),
    }
}

pub fn letv(name: &str, value: Expr) -> Stmt {
    let_(pvar(name), value)
}

pub fn discard(e: Expr) -> Stmt {
    Stmt::Expr(e)
}

pub fn list(items: Vec<Expr>) -> Expr {
    ex(ExprKind::List { items })
}

pub fn record(fields: Vec<(&str, Expr)>) -> Expr {
    ex(ExprKind::Record {
        fields: fields.into_iter().map(|(n, e)| (id(n), e)).collect(),
    })
}

pub fn field(base: Expr, name: &str) -> Expr {
    ex(ExprKind::Field {
        base: Box::new(base),
        field: id(name),
    })
}

pub fn pwild() -> Pattern {
    Pattern {
        kind: PatternKind::Wildcard,
        span: sp(),
    }
}

pub fn pvar(name: &str) -> Pattern {
    Pattern {
        kind: PatternKind::Var(id(name)),
        span: sp(),
    }
}

pub fn pint(i: i64) -> Pattern {
    Pattern {
        kind: PatternKind::Lit(Lit::Int(i)),
        span: sp(),
    }
}

pub fn pstr(s: &str) -> Pattern {
    Pattern {
        kind: PatternKind::Lit(Lit::Str(s.to_string())),
        span: sp(),
    }
}

pub fn pbytes(b: &[u8]) -> Pattern {
    Pattern {
        kind: PatternKind::Lit(Lit::Bytes(b.to_vec())),
        span: sp(),
    }
}

pub fn punit() -> Pattern {
    Pattern {
        kind: PatternKind::Lit(Lit::Unit),
        span: sp(),
    }
}

pub fn pbool(b: bool) -> Pattern {
    Pattern {
        kind: PatternKind::Lit(Lit::Bool(b)),
        span: sp(),
    }
}

pub fn pctor(name: &str, args: Vec<Pattern>) -> Pattern {
    Pattern {
        kind: PatternKind::Ctor {
            name: qname(name),
            args,
        },
        span: sp(),
    }
}

pub fn plist(items: Vec<Pattern>, rest: Option<Pattern>) -> Pattern {
    Pattern {
        kind: PatternKind::List {
            items,
            rest: rest.map(Box::new),
        },
        span: sp(),
    }
}

pub fn prec(fields: Vec<(&str, Pattern)>, rest: bool) -> Pattern {
    Pattern {
        kind: PatternKind::Record {
            fields: fields.into_iter().map(|(n, p)| (id(n), p)).collect(),
            rest,
        },
        span: sp(),
    }
}

pub fn arm(pat: Pattern, body: Expr) -> MatchArm {
    MatchArm {
        pat,
        guard: None,
        body,
        span: sp(),
    }
}

pub fn guarded(pat: Pattern, guard: Expr, body: Expr) -> MatchArm {
    MatchArm {
        pat,
        guard: Some(guard),
        body,
        span: sp(),
    }
}

pub fn match_(scrutinee: Expr, arms: Vec<MatchArm>) -> Expr {
    ex(ExprKind::Match {
        scrutinee: Box::new(scrutinee),
        arms,
    })
}

pub fn perform(effect: &str, op: &str, resource: Option<&str>, args: Vec<Expr>) -> Expr {
    ex(ExprKind::Perform {
        effect: qname(effect),
        op: id(op),
        resource: resource.map(id),
        args,
    })
}

pub fn clause(
    effect: &str,
    op: &str,
    resource: Option<&str>,
    params: &[&str],
    body: Expr,
) -> HandleClause {
    HandleClause {
        effect: qname(effect),
        op: id(op),
        resource: resource.map(id),
        params: params.iter().map(|p| id(p)).collect(),
        resume: None,
        body,
        span: sp(),
    }
}

/// `op(x̄) resume κ -> body`.
pub fn general_clause(
    effect: &str,
    op: &str,
    resource: Option<&str>,
    params: &[&str],
    binder: &str,
    body: Expr,
) -> HandleClause {
    HandleClause {
        resume: Some(id(binder)),
        ..clause(effect, op, resource, params, body)
    }
}

pub fn handle(body: Expr, clauses: Vec<HandleClause>) -> Expr {
    ex(ExprKind::Handle {
        body: Box::new(body),
        clauses,
        return_clause: None,
    })
}

pub fn handle_ret(body: Expr, clauses: Vec<HandleClause>, binder: &str, ret: Expr) -> Expr {
    ex(ExprKind::Handle {
        body: Box::new(body),
        clauses,
        return_clause: Some(Box::new(ReturnClause {
            binder: id(binder),
            body: ret,
            span: sp(),
        })),
    })
}

pub fn with_cell(resource: &str, init: Expr, binder: &str, body: Expr) -> Expr {
    ex(ExprKind::WithCell {
        resource: id(resource),
        init: Box::new(init),
        binder: id(binder),
        body: Box::new(body),
    })
}

/// A nullary type constructor: `Int`, `Bool`, `Bytes`, `Unit`, `String`,
/// `Float`.
pub fn tcon(name: &str) -> TypeExpr {
    TypeExpr::Con {
        name: qname(name),
        args: Vec::new(),
        span: sp(),
    }
}

/// A type constructor applied to arguments: `List<Int>`, `Map<String, Int>`.
pub fn tapp(name: &str, args: Vec<TypeExpr>) -> TypeExpr {
    TypeExpr::Con {
        name: qname(name),
        args,
        span: sp(),
    }
}

/// A type parameter, bound by the enclosing definition's `<..>` rather than by
/// any module. Only [`fn_def_poly`] binds one.
pub fn tvar(name: &str) -> TypeExpr {
    TypeExpr::Var(id(name))
}

/// A definition with **no** written signature, which the checker now rejects
/// with E0126 MISSING_SIGNATURE. For hashing and evaluation fixtures only —
/// neither reads a written type, and neither runs the checker. A fixture that
/// will be checked wants [`fn_def_sig`].
pub fn fn_def(name: &str, params: &[&str], body: Expr) -> Item {
    Item::Fn(Box::new(FnDef {
        vis: Visibility::Private,
        name: id(name),
        generics: Generics::default(),
        params: params.iter().map(|p| param(p)).collect(),
        ret: None,
        effects: None,
        constraints: Vec::new(),
        derived: None,
        spec: Vec::new(),
        body,
        span: sp(),
    }))
}

/// [`fn_def`] with the signature written: every parameter typed and a return
/// type, which is what a definition needs to clear E0126. The effect row stays
/// absent, because rows are still inferred.
pub fn fn_def_sig(name: &str, params: &[(&str, TypeExpr)], ret: TypeExpr, body: Expr) -> Item {
    Item::Fn(Box::new(FnDef {
        vis: Visibility::Private,
        name: id(name),
        generics: Generics::default(),
        params: params.iter().map(|(n, t)| param_ty(n, t.clone())).collect(),
        ret: Some(ret),
        effects: None,
        constraints: Vec::new(),
        derived: None,
        spec: Vec::new(),
        body,
        span: sp(),
    }))
}

/// [`fn_def_sig`] with `generics` bound, so a parameter can be written at a
/// [`tvar`].
///
/// A signature the checker cannot suggest — `fn head(xs) = len(xs)` publishes
/// `<a>(List<a>) -> Int` — has to be written at the same generality it was
/// inferred at, because a `Type::Var` is what `compiled`'s argument gate refuses
/// and a monomorphic `List<Int>` in its place would be carried instead.
pub fn fn_def_poly(
    name: &str,
    generics: &[&str],
    params: &[(&str, TypeExpr)],
    ret: TypeExpr,
    body: Expr,
) -> Item {
    let Item::Fn(mut def) = fn_def_sig(name, params, ret, body) else {
        unreachable!("`fn_def_sig` builds an `Item::Fn`")
    };
    def.generics.types = generics.iter().map(|g| id(g)).collect();
    Item::Fn(def)
}

pub fn test_def(name: &str, body: Expr) -> Item {
    Item::Test(Box::new(TestDef {
        name: name.to_string(),
        name_span: sp(),
        nondet: false,
        body,
        span: sp(),
    }))
}

pub fn type_def(name: &str, variants: &[(&str, usize)]) -> Item {
    let int_ty = tcon("Int");
    Item::Type(Box::new(TypeDef {
        vis: Visibility::Private,
        name: id(name),
        params: Vec::new(),
        body: TypeDefBody::Sum(
            variants
                .iter()
                .map(|(n, arity)| VariantDef {
                    name: id(n),
                    fields: (0..*arity).map(|_| int_ty.clone()).collect(),
                    span: sp(),
                })
                .collect(),
        ),
        span: sp(),
    }))
}

pub fn effect_def(name: &str, ops: &[(&str, Mode, bool)]) -> Item {
    let int_ty = tcon("Int");
    Item::Effect(Box::new(EffectDef {
        vis: Visibility::Private,
        name: id(name),
        nondet: false,
        ops: ops
            .iter()
            .map(|(n, mode, resource_param)| OpDef {
                name: id(n),
                mode: *mode,
                resource_param: *resource_param,
                params: vec![int_ty.clone()],
                ret: int_ty.clone(),
                span: sp(),
            })
            .collect(),
        span: sp(),
    }))
}

/// One anonymous module standing alone: bare names stay bare, so a hand-built AST reads exactly as
/// it did before modules existed.
pub fn module(items: Vec<Item>) -> Module {
    Module {
        name: ModuleName::anonymous(),
        source: SourceId(0),
        imports: Vec::new(),
        items,
    }
}

pub fn standalone(items: Vec<Item>) -> (Program, Resolved) {
    standalone_module(module(items))
}

pub fn standalone_module(module: Module) -> (Program, Resolved) {
    let mut program = Program::single(module);
    let resolved = resolve(&mut program).expect("a module with no imports resolves");
    (program, resolved)
}
