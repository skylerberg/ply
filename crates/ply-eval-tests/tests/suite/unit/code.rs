use crate::unit::build::{at, bin, block, callv, discard, int, lam, record, spanned, var};
use ply_eval::code::*;
use ply_syntax::ast::BinOp;
use std::rc::Rc;

#[test]
fn lowering_preserves_spans() {
    let e = spanned(int(3), at(10, 11));
    assert_eq!(lower(&e).code.span, at(10, 11));
}

#[test]
fn a_lambda_body_is_shared_rather_than_cloned_per_reference() {
    let e = lam(&["x"], bin(BinOp::Add, var("x"), int(1)));
    let code = lower(&e).code;
    let NodeKind::Lambda { body, .. } = &code.kind else {
        panic!("expected a lambda");
    };
    let held = body.clone();
    assert!(Rc::ptr_eq(body, &held));
}

#[test]
fn a_block_lowers_its_statements_and_tail() {
    let e = block(vec![discard(int(1))], Some(callv("len", vec![var("xs")])));
    let NodeKind::Block { stmts, tail } = &lower(&e).code.kind else {
        panic!("expected a block");
    };
    assert_eq!(stmts.len(), 1);
    assert!(tail.is_some());
}

#[test]
fn a_record_keeps_its_fields_in_source_order() {
    let e = record(vec![("b", int(2)), ("a", int(1))]);
    let NodeKind::Record { fields, .. } = &lower(&e).code.kind else {
        panic!("expected a record");
    };
    let names: Vec<&str> = fields.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["b", "a"]);
}

#[test]
fn a_lambda_captures_its_free_variable_into_its_own_window() {
    let e = block(
        vec![crate::unit::build::letv("n", int(1))],
        Some(lam(&["y"], bin(BinOp::Add, var("y"), var("n")))),
    );
    let lowered = lower(&e);
    let NodeKind::Block { tail, .. } = &lowered.code.kind else {
        panic!("expected a block");
    };
    let NodeKind::Lambda { captures, size, .. } = &tail.as_ref().unwrap().kind else {
        panic!("expected a lambda");
    };
    assert_eq!(captures.len(), 1, "`n` is free in the lambda");
    assert_eq!(*size, 2, "the window holds the parameter and the capture");
}
