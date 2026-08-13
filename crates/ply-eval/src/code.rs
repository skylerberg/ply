//! The AST, lowered so that a subexpression can be held by a continuation
//! frame.
//!
//! A frame has to name the expression it will evaluate next, and a closure has
//! to name its body. Neither can be `&Expr` without a lifetime on [`Value`],
//! which would spread through `World`, `Env` and every crate that holds an
//! evaluated value; and neither can be an owned `Expr`, because a frame is
//! pushed per node and cloning a subtree per push is quadratic. `Rc` on every
//! node is the representation that is cheap in both directions: `lower` runs
//! once per machine, and after it every handle to a subexpression is one
//! pointer.
//!
//! The shape mirrors `ply_syntax::ast::ExprKind` one to one. Anything the
//! machine never has to suspend inside — patterns, names, literals, operators —
//! is reused from the AST rather than mirrored.

use ply_span::{Span, Symbol};
use ply_syntax::ast::{
    BinOp, Expr, ExprKind, HandleClause, Ident, Lit, MatchArm, Pattern, QName, ReturnClause,
    Stmt as AstStmt, UnOp,
};
use std::rc::Rc;

pub type Code = Rc<Node>;

pub struct Node {
    pub kind: NodeKind,
    pub span: Span,
}

pub enum NodeKind {
    Lit(Lit),
    Var(QName),
    Unary {
        op: UnOp,
        operand: Code,
    },
    Binary {
        op: BinOp,
        lhs: Code,
        rhs: Code,
    },
    Lambda {
        params: Rc<Vec<Symbol>>,
        body: Code,
    },
    App {
        func: Code,
        args: Rc<Vec<Code>>,
    },
    If {
        cond: Code,
        then_branch: Code,
        else_branch: Code,
    },
    Match {
        scrutinee: Code,
        arms: Rc<Vec<Arm>>,
    },
    Block {
        stmts: Rc<Vec<Stmt>>,
        tail: Option<Code>,
    },
    Record {
        fields: Rc<Vec<(Symbol, Code)>>,
    },
    Field {
        base: Code,
        field: Ident,
    },
    List {
        items: Rc<Vec<Code>>,
    },
    Perform {
        effect: QName,
        op: Symbol,
        resource: Option<Symbol>,
        args: Rc<Vec<Code>>,
    },
    Handle {
        body: Code,
        clauses: Rc<Vec<Clause>>,
        ret: Option<Rc<ReturnArm>>,
    },
    WithCell {
        resource: Symbol,
        init: Code,
        binder: Symbol,
        body: Code,
    },
}

pub struct Arm {
    pub pat: Pattern,
    pub guard: Option<Code>,
    pub body: Code,
    pub span: Span,
}

pub enum Stmt {
    Let {
        pat: Pattern,
        value: Code,
        span: Span,
    },
    Expr(Code),
}

pub struct Clause {
    pub effect: QName,
    pub op: Symbol,
    pub resource: Option<Symbol>,
    pub params: Rc<Vec<Symbol>>,
    /// The continuation binder of a general clause. `None` is the
    /// tail-resumptive form, whose body's value goes straight back to the
    /// perform site and which therefore needs no capture at all.
    pub resume: Option<Symbol>,
    pub body: Code,
    pub span: Span,
}

pub struct ReturnArm {
    pub binder: Symbol,
    pub body: Code,
    pub span: Span,
}

/// Grows the host stack rather than bounding the nesting: the parser, inference
/// and normalization all accept an expression of any depth by growing, and a
/// bound here would refuse — on the machine only — a program `ply check` and
/// `ply run` accept. That is an `E0503` divergence on every corpus with a long
/// operator chain in it.
pub fn lower(e: &Expr) -> Code {
    crate::limit::grow(|| lower_node(e))
}

fn lower_node(e: &Expr) -> Code {
    let kind = match &e.kind {
        ExprKind::Lit(lit) => NodeKind::Lit(lit.clone()),
        ExprKind::Var(q) => NodeKind::Var(q.clone()),
        ExprKind::Unary { op, operand } => NodeKind::Unary {
            op: *op,
            operand: lower(operand),
        },
        ExprKind::Binary { op, lhs, rhs } => NodeKind::Binary {
            op: *op,
            lhs: lower(lhs),
            rhs: lower(rhs),
        },
        ExprKind::Lambda { params, body } => NodeKind::Lambda {
            params: Rc::new(params.iter().map(|p| p.name.name.clone()).collect()),
            body: lower(body),
        },
        ExprKind::App { func, args } => NodeKind::App {
            func: lower(func),
            args: lower_all(args),
        },
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => NodeKind::If {
            cond: lower(cond),
            then_branch: lower(then_branch),
            else_branch: lower(else_branch),
        },
        ExprKind::Match { scrutinee, arms } => NodeKind::Match {
            scrutinee: lower(scrutinee),
            arms: Rc::new(arms.iter().map(lower_arm).collect()),
        },
        ExprKind::Block { stmts, tail } => NodeKind::Block {
            stmts: Rc::new(stmts.iter().map(lower_stmt).collect()),
            tail: tail.as_deref().map(lower),
        },
        ExprKind::Record { fields } => NodeKind::Record {
            fields: Rc::new(
                fields
                    .iter()
                    .map(|(name, value)| (name.name.clone(), lower(value)))
                    .collect(),
            ),
        },
        ExprKind::Field { base, field } => NodeKind::Field {
            base: lower(base),
            field: field.clone(),
        },
        ExprKind::List { items } => NodeKind::List {
            items: lower_all(items),
        },
        ExprKind::Perform {
            effect,
            op,
            resource,
            args,
        } => NodeKind::Perform {
            effect: effect.clone(),
            op: op.name.clone(),
            resource: resource.as_ref().map(|r| r.name.clone()),
            args: lower_all(args),
        },
        ExprKind::Handle {
            body,
            clauses,
            return_clause,
        } => NodeKind::Handle {
            body: lower(body),
            clauses: Rc::new(clauses.iter().map(lower_clause).collect()),
            ret: return_clause.as_deref().map(lower_return),
        },
        ExprKind::WithCell {
            resource,
            init,
            binder,
            body,
        } => NodeKind::WithCell {
            resource: resource.name.clone(),
            init: lower(init),
            binder: binder.name.clone(),
            body: lower(body),
        },
    };
    Rc::new(Node { kind, span: e.span })
}

fn lower_all(exprs: &[Expr]) -> Rc<Vec<Code>> {
    Rc::new(exprs.iter().map(lower).collect())
}

fn lower_arm(arm: &MatchArm) -> Arm {
    Arm {
        pat: arm.pat.clone(),
        guard: arm.guard.as_ref().map(lower),
        body: lower(&arm.body),
        span: arm.span,
    }
}

fn lower_stmt(stmt: &AstStmt) -> Stmt {
    match stmt {
        AstStmt::Let {
            pat, value, span, ..
        } => Stmt::Let {
            pat: pat.clone(),
            value: lower(value),
            span: *span,
        },
        AstStmt::Expr(e) => Stmt::Expr(lower(e)),
    }
}

fn lower_clause(c: &HandleClause) -> Clause {
    Clause {
        effect: c.effect.clone(),
        op: c.op.name.clone(),
        resource: c.resource.as_ref().map(|r| r.name.clone()),
        params: Rc::new(c.params.iter().map(|p| p.name.clone()).collect()),
        resume: c.resume.as_ref().map(|r| r.name.clone()),
        body: lower(&c.body),
        span: c.span,
    }
}

fn lower_return(rc: &ReturnClause) -> Rc<ReturnArm> {
    Rc::new(ReturnArm {
        binder: rc.binder.name.clone(),
        body: lower(&rc.body),
        span: rc.span,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::{at, bin, block, callv, discard, int, lam, record, spanned, var};

    #[test]
    fn lowering_preserves_spans() {
        let e = spanned(int(3), at(10, 11));
        assert_eq!(lower(&e).span, at(10, 11));
    }

    #[test]
    fn a_lambda_body_is_shared_rather_than_cloned_per_reference() {
        let e = lam(&["x"], bin(BinOp::Add, var("x"), int(1)));
        let code = lower(&e);
        let NodeKind::Lambda { body, .. } = &code.kind else {
            panic!("expected a lambda");
        };
        let held = body.clone();
        assert!(Rc::ptr_eq(body, &held));
    }

    #[test]
    fn a_block_lowers_its_statements_and_tail() {
        let e = block(vec![discard(int(1))], Some(callv("len", vec![var("xs")])));
        let NodeKind::Block { stmts, tail } = &lower(&e).kind else {
            panic!("expected a block");
        };
        assert_eq!(stmts.len(), 1);
        assert!(tail.is_some());
    }

    #[test]
    fn a_record_keeps_its_fields_in_source_order() {
        let e = record(vec![("b", int(2)), ("a", int(1))]);
        let NodeKind::Record { fields } = &lower(&e).kind else {
            panic!("expected a record");
        };
        let names: Vec<&str> = fields.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["b", "a"]);
    }
}
