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

pub fn qname(name: &str) -> QName {
    QName::bare(id(name))
}

pub fn var(name: &str) -> Expr {
    ex(ExprKind::Var(qname(name)))
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

pub fn param(name: &str) -> Param {
    Param {
        name: id(name),
        ty: None,
        default: None,
        span: sp(),
    }
}

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
        ret: None,
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

pub fn record(fields: Vec<(&str, Expr)>) -> Expr {
    ex(ExprKind::Record {
        fields: fields.into_iter().map(|(n, e)| (id(n), e)).collect(),
    })
}

pub fn pvar(name: &str) -> Pattern {
    Pattern {
        kind: PatternKind::Var(id(name)),
        span: sp(),
    }
}

pub fn tcon(name: &str) -> TypeExpr {
    TypeExpr::Con {
        name: qname(name),
        args: Vec::new(),
        span: sp(),
    }
}

/// Every parameter and the return typed; the effect row stays inferred.
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
        reuse: None,
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

/// One anonymous module, so bare names stay bare.
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
