use crate::unit::build::{
    at, bin, block, callv, discard, fn_def, int, lam, record, spanned, standalone, var,
};
use ply_eval::Own;
use ply_eval::code::*;
use ply_span::Symbol;
use ply_syntax::ast::Item;
use ply_syntax::ast::{BinOp, Expr};
use std::rc::Rc;
use std::sync::Arc;

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

fn body_of<'a>(program: &'a ply_syntax::ast::Program, name: &str) -> &'a Expr {
    program.modules[0]
        .items
        .iter()
        .find_map(|item| match item {
            Item::Fn(f) if f.name.name.as_str() == name => Some(&f.body),
            _ => None,
        })
        .expect("the program declares it")
}

#[test]
fn a_body_lowered_twice_is_lowered_once() {
    let (program, _) = standalone(vec![fn_def("f", &["x"], bin(BinOp::Add, var("x"), int(1)))]);
    let lowering = Lowering::for_program(&program);
    let params: Params = Rc::new(vec![Symbol::new("x")]);
    let first = lowering.of(&params, body_of(&program, "f"));
    let second = lowering.of(&params, body_of(&program, "f"));
    assert!(
        Rc::ptr_eq(&first.code, &second.code),
        "the same body lowered twice produced two different trees"
    );
    assert_eq!(lowering.len(), 1, "one body, {} entries", lowering.len());
}

/// A cache that answered for one parameter list from the other would hand out the wrong liveness.
#[test]
fn one_body_under_two_parameter_lists_is_lowered_twice() {
    let (program, _) = standalone(vec![fn_def("f", &["x"], var("x"))]);
    let lowering = Lowering::for_program(&program);
    let body = body_of(&program, "f");
    let owned = lowering.of(&Rc::new(vec![Symbol::new("x")]), body);
    let borrowed = lowering.body(body);
    assert!(matches!(owned.code.own, Own::Owned));
    assert!(
        matches!(borrowed.code.own, Own::Borrowed),
        "a body lowered under no parameters was answered from the entry that had one"
    );
}

#[test]
fn a_cache_taken_over_another_program_does_not_describe_this_one() {
    let (one, _) = standalone(vec![fn_def("f", &[], int(1))]);
    let (two, _) = standalone(vec![fn_def("f", &[], int(2))]);
    let lowering = Lowering::for_program(&one);
    assert!(lowering.describes(&one));
    assert!(
        !lowering.describes(&two),
        "a cache over one program claimed to describe another, so a bisection's \
         rebuilt body could be answered from the body it replaced"
    );
}

#[test]
fn the_last_closure_body_is_lowered_once_however_often_it_is_applied() {
    let body = Arc::new(bin(BinOp::Add, var("x"), int(1)));
    let params = [Symbol::new("x")];
    let mut cache = ClosureCode::default();
    let first = cache.of(&[], &params, &body);
    let second = cache.of(&[], &params, &body);
    assert!(Rc::ptr_eq(&first.code, &second.code));

    let other = Arc::new(bin(BinOp::Sub, var("x"), int(1)));
    let third = cache.of(&[], &params, &other);
    assert!(
        !Rc::ptr_eq(&first.code, &third.code),
        "a different body was answered from the previous one's entry"
    );
    assert!(Rc::ptr_eq(
        &third.code,
        &cache.of(&[], &params, &other).code
    ));
}
