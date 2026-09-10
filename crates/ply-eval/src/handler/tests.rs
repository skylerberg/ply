//! Effect handlers end to end, on the machine that runs them.

use super::*;
use crate::build;
use crate::build::{bin, block, callv, int, letv, list, var};
use crate::evaluator::Machine;
use ply_span::{SourceId, Symbol};
use ply_syntax::ast::{BinOp, Expr, HandleClause, Item, Mode};

// The programs are built as AST and lowered by the real lowering, so every variable occurrence
// gets the slot the machine will read.

fn sp() -> Span {
    Span::new(SourceId(0), 0, 1)
}

fn perform_(effect: &str, op: &str, resource: Option<&str>, args: Vec<Expr>) -> Expr {
    build::perform(effect, op, resource, args)
}

fn clause_(
    effect: &str,
    op: &str,
    resource: Option<&str>,
    params: &[&str],
    resume: Option<&str>,
    body: Expr,
) -> HandleClause {
    match resume {
        Some(k) => build::general_clause(effect, op, resource, params, k, body),
        None => build::clause(effect, op, resource, params, body),
    }
}

fn handle_(body: Expr, clauses: Vec<HandleClause>, ret: Option<(&str, Expr)>) -> Expr {
    match ret {
        Some((binder, ret)) => build::handle_ret(body, clauses, binder, ret),
        None => build::handle(body, clauses),
    }
}

fn with_cell_(resource: &str, init: Expr, binder: &str, body: Expr) -> Expr {
    build::with_cell(resource, init, binder, body)
}

fn cell_get(cell: Expr) -> Expr {
    callv("cell_get", vec![cell])
}

/// Every test below runs on the real machine.
struct Outcome {
    result: Result<Value, Diagnostic>,
    /// The run's cells, ascending by slot index — the order `Arena::slots` hands them out, so two
    /// runs of one program compare byte for byte.
    cells: Vec<Value>,
}

fn run(e: &Expr) -> Outcome {
    run_in(Vec::new(), e)
}

fn run_in(items: Vec<Item>, e: &Expr) -> Outcome {
    let (program, resolved) = build::standalone(items);
    let mut machine = Machine::for_program(&program, &resolved);
    let result = machine.eval_expr_for_test(e);
    Outcome {
        result,
        cells: machine.cells().slots().map(|(_, v)| v.clone()).collect(),
    }
}

impl Outcome {
    #[track_caller]
    fn value(&self) -> &Value {
        match &self.result {
            Ok(v) => v,
            Err(d) => panic!("expected a value, got [{}] {}", d.code, d.message),
        }
    }

    #[track_caller]
    fn int(&self) -> i64 {
        match self.value() {
            Value::Int(i) => *i,
            other => panic!("expected an Int, got {other}"),
        }
    }

    #[track_caller]
    fn rendered(&self) -> String {
        self.value().render()
    }

    #[track_caller]
    fn cell(&self, index: u32) -> i64 {
        match self.cells.get(index as usize) {
            Some(Value::Int(i)) => *i,
            other => panic!("cell {index} holds {other:?}"),
        }
    }
}

/// An expression in a program of its own, evaluated to whatever it answers.
#[track_caller]
fn standalone(items: Vec<Item>, e: &ply_syntax::ast::Expr) -> Result<Value, Diagnostic> {
    let (program, resolved) = build::standalone(items);
    Machine::for_program(&program, &resolved).eval_expr_for_test(e)
}

#[test]
fn a_tail_resumptive_clause_that_performs_its_own_operation_reaches_the_next_handler_out() {
    let e = handle_(
        handle_(
            perform_("state", "get", None, vec![]),
            vec![clause_(
                "state",
                "get",
                None,
                &[],
                None,
                bin(BinOp::Add, perform_("state", "get", None, vec![]), int(1)),
            )],
            None,
        ),
        vec![clause_("state", "get", None, &[], None, int(10))],
        None,
    );
    assert_eq!(run(&e).int(), 11);
}

#[test]
fn an_inner_handler_that_does_not_name_the_operation_falls_through() {
    let e = handle_(
        handle_(
            perform_("db", "get", Some("orders"), vec![int(0)]),
            vec![clause_(
                "db",
                "get",
                Some("users"),
                &["k"],
                Some("r"),
                int(1),
            )],
            None,
        ),
        vec![clause_("db", "get", Some("orders"), &["k"], None, int(7))],
        None,
    );
    assert_eq!(run(&e).int(), 7);
}

#[test]
fn each_region_allocates_its_own_cell_and_the_world_keeps_both() {
    let e = block(
        vec![
            letv("a", with_cell_("s", int(1), "c", cell_get(var("c")))),
            letv("b", with_cell_("s", int(2), "c", cell_get(var("c")))),
        ],
        Some(list(vec![var("a"), var("b")])),
    );

    let run = run(&e);
    assert_eq!(run.rendered(), "[1, 2]");
    assert_eq!(
        run.cells.len(),
        2,
        "a shared region's slots outlive its close, so both are still there"
    );
    assert_eq!(run.cell(0), 1);
    assert_eq!(run.cell(1), 2);
}

#[test]
fn a_clause_arity_mismatch_is_an_arity_mismatch() {
    let e = build::handle(
        build::perform("state", "get", None, vec![]),
        vec![build::clause("state", "get", None, &["k"], build::int(0))],
    );
    let d = standalone(
        vec![build::effect_def("state", &[("get", Mode::Read, false)])],
        &e,
    )
    .expect_err("the clause wants one argument and the perform gives none");
    assert_eq!(d.code, codes::ARITY_MISMATCH);
}

#[test]
fn a_handler_that_reads_and_writes_a_cell_answers_through_its_return_clause() {
    let e = build::with_cell(
        "s",
        build::int(1),
        "c",
        build::handle_ret(
            build::bin(
                BinOp::Add,
                build::perform("state", "get", None, vec![]),
                build::perform("state", "put", None, vec![build::int(4)]),
            ),
            vec![
                build::clause(
                    "state",
                    "get",
                    None,
                    &[],
                    build::callv("cell_get", vec![build::var("c")]),
                ),
                build::clause(
                    "state",
                    "put",
                    None,
                    &["v"],
                    build::block(
                        vec![build::discard(build::callv(
                            "cell_set",
                            vec![build::var("c"), build::var("v")],
                        ))],
                        Some(build::int(100)),
                    ),
                ),
            ],
            "x",
            build::bin(BinOp::Mul, build::var("x"), build::int(2)),
        ),
    );
    let v = standalone(
        vec![build::effect_def(
            "state",
            &[("get", Mode::Read, false), ("put", Mode::Write, false)],
        )],
        &e,
    )
    .expect("the program has a handler for everything it performs");
    assert_eq!(v.render(), "202");
}

#[test]
fn check_operation_accepts_an_effect_no_module_declares() {
    let effect = Symbol::new("mystery");
    let op = Symbol::new("go");
    assert!(check_operation(OpDecl::UnknownEffect, &effect, &op, false, sp()).is_ok());
}
