//! AST constructors for the tests. The evaluator is tested against the AST
//! directly so a parser change cannot silently redefine what is being checked.

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
        span: sp(),
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

/// `op(x̄) resume κ -> body`. The body's value is the whole `handle`'s result,
/// and reaching the resumption is up to the body.
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

pub fn fn_def(name: &str, params: &[&str], body: Expr) -> Item {
    Item::Fn(Box::new(FnDef {
        vis: Visibility::Private,
        name: id(name),
        generics: Generics::default(),
        params: params.iter().map(|p| param(p)).collect(),
        ret: None,
        effects: None,
        body,
        span: sp(),
    }))
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
    let int_ty = TypeExpr::Con {
        name: qname("Int"),
        args: Vec::new(),
        span: sp(),
    };
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
    let int_ty = TypeExpr::Con {
        name: qname("Int"),
        args: Vec::new(),
        span: sp(),
    };
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

/// One anonymous module standing alone: bare names stay bare, so a hand-built
/// AST reads exactly as it did before modules existed.
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
    let program = Program::single(module);
    let resolved = resolve(&program).expect("a module with no imports resolves");
    (program, resolved)
}
