//! Effect handlers end to end, on the machine that runs them.

use super::*;
use crate::build;
use crate::code::{Node, NodeKind, Stmt};
use crate::machine::{Machine, Progress};
use ply_span::{SourceId, Symbol};
use ply_syntax::ast::{BinOp, Ident, Item, Lit, Mode, Pattern, PatternKind, QName};

// Building lowered code directly.

fn sp() -> Span {
    Span::new(SourceId(0), 0, 1)
}

fn node(kind: NodeKind) -> Code {
    Rc::new(Node {
        kind,
        span: sp(),
        own: crate::rc::Own::Borrowed,
    })
}

fn lit(l: Lit) -> Code {
    let value = crate::semantics::literal(&l);
    node(NodeKind::Lit(l, value))
}

fn int(i: i64) -> Code {
    lit(Lit::Int(i))
}

fn boolean(b: bool) -> Code {
    lit(Lit::Bool(b))
}

fn unit() -> Code {
    lit(Lit::Unit)
}

fn var(name: &str) -> Code {
    node(NodeKind::Var(QName::bare(Ident::new(name, sp()))))
}

fn bin(op: BinOp, lhs: Code, rhs: Code) -> Code {
    node(NodeKind::Binary { op, lhs, rhs })
}

fn if_(cond: Code, then_branch: Code, else_branch: Code) -> Code {
    node(NodeKind::If {
        cond,
        then_branch,
        else_branch,
    })
}

fn call(func: Code, args: Vec<Code>) -> Code {
    node(NodeKind::App {
        func,
        args: Rc::new(args),
        dead: Rc::new(Vec::new()),
    })
}

fn callv(name: &str, args: Vec<Code>) -> Code {
    call(var(name), args)
}

fn lam(params: &[&str], body: Code) -> Code {
    node(NodeKind::Lambda {
        params: Rc::new(params.iter().map(|p| Symbol::new(*p)).collect()),
        body,
    })
}

fn list(items: Vec<Code>) -> Code {
    node(NodeKind::List {
        items: Rc::new(items),
    })
}

fn block(stmts: Vec<Stmt>, tail: Option<Code>) -> Code {
    node(NodeKind::Block {
        stmts: Rc::new(stmts),
        tail,
    })
}

fn letv(name: &str, value: Code) -> Stmt {
    Stmt::Let {
        pat: Pattern {
            kind: PatternKind::Var(Ident::new(name, sp())),
            span: sp(),
        },
        value,
        span: sp(),
        dead: crate::rc::no_dead(),
    }
}

fn discard(value: Code) -> Stmt {
    Stmt::Expr {
        code: value,
        dead: crate::rc::no_dead(),
    }
}

fn perform_(effect: &str, op: &str, resource: Option<&str>, args: Vec<Code>) -> Code {
    node(NodeKind::Perform {
        effect: QName::bare(Ident::new(effect, sp())),
        op: Symbol::new(op),
        resource: resource.map(Symbol::new),
        args: Rc::new(args),
    })
}

fn clause_(
    effect: &str,
    op: &str,
    resource: Option<&str>,
    params: &[&str],
    resume: Option<&str>,
    body: Code,
) -> Clause {
    Clause {
        effect: QName::bare(Ident::new(effect, sp())),
        op: Symbol::new(op),
        resource: resource.map(Symbol::new),
        params: Rc::new(params.iter().map(|p| Symbol::new(*p)).collect()),
        resume: resume.map(Symbol::new),
        body,
        span: sp(),
    }
}

fn handle_(body: Code, clauses: Vec<Clause>, ret: Option<(&str, Code)>) -> Code {
    node(NodeKind::Handle {
        body,
        clauses: Rc::new(clauses),
        ret: ret.map(|(binder, body)| {
            Rc::new(ReturnArm {
                binder: Symbol::new(binder),
                body,
                span: sp(),
            })
        }),
    })
}

fn with_cell_(resource: &str, init: Code, binder: &str, body: Code) -> Code {
    node(NodeKind::WithCell {
        resource: Symbol::new(resource),
        init,
        binder: Symbol::new(binder),
        body,
    })
}

fn cell_get(cell: Code) -> Code {
    callv("cell_get", vec![cell])
}

fn cell_set(cell: Code, value: Code) -> Code {
    callv("cell_set", vec![cell, value])
}

/// Every test below runs on the real machine.
struct Outcome {
    result: Result<Value, Diagnostic>,
    /// The run's cells, ascending by slot index — the order `Arena::slots` hands them out, so two
    /// runs of one program compare byte for byte.
    cells: Vec<Value>,
}

fn run(code: &Code) -> Outcome {
    run_in(Vec::new(), code)
}

fn run_in(items: Vec<Item>, code: &Code) -> Outcome {
    let (program, resolved) = build::standalone(items);
    let mut machine = Machine::for_program(&program, &resolved);
    machine.go_eval(code.clone(), Env::empty(), 0);
    let result = loop {
        match machine.step() {
            Ok(Progress::Running) => {}
            Ok(Progress::Halted(v)) => break Ok(v),
            Err(d) => break Err(d),
        }
    };
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
    fn diagnostic(&self) -> &Diagnostic {
        match &self.result {
            Err(d) => d,
            Ok(v) => panic!("expected a diagnostic, got {v}"),
        }
    }

    #[track_caller]
    fn cell(&self, index: u32) -> i64 {
        match self.cells.get(index as usize) {
            Some(Value::Int(i)) => *i,
            other => panic!("cell {index} holds {other:?}"),
        }
    }

    fn cells(&self) -> Vec<String> {
        self.cells.iter().map(Value::render).collect()
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
fn a_multi_shot_clause_that_performs_its_own_operation_reaches_the_next_handler_out() {
    // The inner clause performs `pick` twice and resumes twice.
    let e = handle_(
        handle_(
            bin(BinOp::Mul, perform_("nd", "pick", None, vec![]), int(10)),
            vec![clause_(
                "nd",
                "pick",
                None,
                &[],
                Some("k"),
                bin(
                    BinOp::Add,
                    call(var("k"), vec![perform_("nd", "pick", None, vec![])]),
                    call(var("k"), vec![perform_("nd", "pick", None, vec![])]),
                ),
            )],
            None,
        ),
        vec![clause_("nd", "pick", None, &[], None, int(1))],
        None,
    );
    assert_eq!(run(&e).int(), 20);
}

#[test]
fn a_self_performing_handler_with_nothing_outside_it_is_unhandled_under_both_clause_forms() {
    for resume_binder in [None, Some("k")] {
        let inner = match resume_binder {
            Some(k) => call(var(k), vec![perform_("state", "get", None, vec![])]),
            None => perform_("state", "get", None, vec![]),
        };
        let e = handle_(
            perform_("state", "get", None, vec![]),
            vec![clause_("state", "get", None, &[], resume_binder, inner)],
            None,
        );
        let out = run(&e);
        let d = out.diagnostic();
        assert_eq!(d.code, codes::UNHANDLED_EFFECT, "{resume_binder:?}");
    }
}

/// ADR 0005 §3.2, "resumes zero times".
#[test]
fn a_clause_that_drops_its_continuation_keeps_the_writes_made_before_the_perform() {
    let e = with_cell_(
        "log",
        int(0),
        "c",
        handle_(
            block(
                vec![
                    discard(cell_set(var("c"), int(1))),
                    letv("b", perform_("amb", "flip", Some("coin"), vec![])),
                    discard(cell_set(var("c"), int(2))),
                ],
                Some(if_(var("b"), int(10), int(20))),
            ),
            vec![clause_(
                "amb",
                "flip",
                Some("coin"),
                &[],
                Some("k"),
                cell_get(var("c")),
            )],
            Some(("x", var("x"))),
        ),
    );

    let run = run(&e);
    assert_eq!(run.int(), 1);
    assert_eq!(run.cell(0), 1, "the write after the perform never ran");
}

/// ADR 0005 §3.2, "resumes once" — the case that decides the design.
#[test]
fn a_resumption_sees_the_write_the_clause_made_before_calling_it() {
    let e = with_cell_(
        "s",
        int(0),
        "c",
        handle_(
            block(
                vec![discard(perform_("state", "put", Some("s"), vec![int(5)]))],
                Some(perform_("state", "get", Some("s"), vec![])),
            ),
            vec![
                clause_(
                    "state",
                    "get",
                    Some("s"),
                    &[],
                    Some("k"),
                    call(var("k"), vec![cell_get(var("c"))]),
                ),
                clause_(
                    "state",
                    "put",
                    Some("s"),
                    &["v"],
                    Some("k"),
                    block(
                        vec![discard(cell_set(var("c"), var("v")))],
                        Some(call(var("k"), vec![unit()])),
                    ),
                ),
            ],
            Some(("x", var("x"))),
        ),
    );

    let run = run(&e);
    assert_eq!(run.int(), 5);
    assert_eq!(run.cell(0), 5);
}

/// ADR 0005 §3.2, "resumes twice".
#[test]
fn two_resumptions_run_against_one_threaded_world() {
    let e = with_cell_(
        "trace",
        int(0),
        "c",
        handle_(
            block(
                vec![
                    letv("b", perform_("amb", "flip", Some("coin"), vec![])),
                    discard(cell_set(
                        var("c"),
                        bin(BinOp::Add, cell_get(var("c")), int(1)),
                    )),
                ],
                Some(if_(var("b"), int(10), int(20))),
            ),
            vec![clause_(
                "amb",
                "flip",
                Some("coin"),
                &[],
                Some("k"),
                bin(
                    BinOp::Add,
                    call(var("k"), vec![boolean(true)]),
                    call(var("k"), vec![boolean(false)]),
                ),
            )],
            Some(("x", var("x"))),
        ),
    );

    let run = run(&e);
    assert_eq!(run.int(), 30);
    assert_eq!(
        run.cell(0),
        2,
        "each resumption incremented the one world; a snapshot would leave 1"
    );
}

/// ADR 0005 §3.3: per-branch state is the handler's job, and four lines of it.
#[test]
fn a_handler_that_restores_the_cell_gives_each_branch_the_same_starting_state() {
    let e = with_cell_(
        "s",
        int(0),
        "c",
        handle_(
            block(
                vec![
                    letv("b", perform_("nd", "pick", None, vec![])),
                    discard(cell_set(
                        var("c"),
                        bin(BinOp::Add, cell_get(var("c")), int(1)),
                    )),
                ],
                Some(if_(bin(BinOp::Eq, var("b"), int(1)), int(10), int(20))),
            ),
            vec![clause_(
                "nd",
                "pick",
                None,
                &[],
                Some("k"),
                block(
                    vec![
                        letv("before", cell_get(var("c"))),
                        letv("first", call(var("k"), vec![int(1)])),
                        discard(cell_set(var("c"), var("before"))),
                        letv("second", call(var("k"), vec![int(2)])),
                    ],
                    Some(bin(BinOp::Add, var("first"), var("second"))),
                ),
            )],
            None,
        ),
    );

    let run = run(&e);
    assert_eq!(run.int(), 30);
    assert_eq!(run.cell(0), 1, "each branch started from 0 and added one");
}

#[test]
fn the_return_clause_runs_once_per_resumption_and_not_on_the_clause_s_own_value() {
    let e = handle_(
        if_(perform_("amb", "flip", None, vec![]), int(1), int(2)),
        vec![clause_(
            "amb",
            "flip",
            None,
            &[],
            Some("k"),
            bin(
                BinOp::Add,
                call(var("k"), vec![boolean(true)]),
                call(var("k"), vec![boolean(false)]),
            ),
        )],
        Some(("x", bin(BinOp::Add, var("x"), int(100)))),
    );
    // (1 + 100) + (2 + 100); the clause's own 203 is not passed through again.
    assert_eq!(run(&e).int(), 203);
}

#[test]
fn a_nondeterministic_operation_delivers_one_value_to_both_resumptions() {
    // `flip` is performed once.
    let e = with_cell_(
        "draws",
        int(0),
        "c",
        handle_(
            block(
                vec![letv("b", perform_("clock", "now", None, vec![]))],
                Some(var("b")),
            ),
            vec![clause_(
                "clock",
                "now",
                None,
                &[],
                Some("k"),
                block(
                    vec![
                        discard(cell_set(
                            var("c"),
                            bin(BinOp::Add, cell_get(var("c")), int(1)),
                        )),
                        letv("drawn", cell_get(var("c"))),
                        letv("first", call(var("k"), vec![var("drawn")])),
                        letv("second", call(var("k"), vec![var("drawn")])),
                    ],
                    Some(bin(BinOp::Add, var("first"), var("second"))),
                ),
            )],
            None,
        ),
    );

    let run = run(&e);
    assert_eq!(run.int(), 2);
    assert_eq!(run.cell(0), 1, "the operation was performed once");
}

#[test]
fn the_tail_resumptive_form_agrees_with_its_general_expansion() {
    let program = |resume_binder: Option<&str>| {
        let body = match resume_binder {
            Some(k) => call(var(k), vec![cell_get(var("c"))]),
            None => cell_get(var("c")),
        };
        with_cell_(
            "s",
            int(7),
            "c",
            handle_(
                block(
                    vec![discard(cell_set(
                        var("c"),
                        bin(BinOp::Add, cell_get(var("c")), int(1)),
                    ))],
                    Some(bin(
                        BinOp::Add,
                        perform_("state", "get", None, vec![]),
                        perform_("state", "get", None, vec![]),
                    )),
                ),
                vec![clause_("state", "get", None, &[], resume_binder, body)],
                Some(("x", bin(BinOp::Mul, var("x"), int(3)))),
            ),
        )
    };

    let tail = run(&program(None));
    let general = run(&program(Some("k")));
    assert_eq!(tail.int(), 48);
    assert_eq!(general.int(), 48);
    assert_eq!(tail.cells(), general.cells());
}

#[test]
fn a_resumed_computation_reaches_the_handler_outside_the_one_that_resumed_it() {
    let e = handle_(
        handle_(
            if_(
                perform_("amb", "flip", None, vec![]),
                perform_("ask", "get", None, vec![]),
                bin(BinOp::Mul, perform_("ask", "get", None, vec![]), int(2)),
            ),
            vec![clause_(
                "amb",
                "flip",
                None,
                &[],
                Some("k"),
                bin(
                    BinOp::Add,
                    call(var("k"), vec![boolean(true)]),
                    call(var("k"), vec![boolean(false)]),
                ),
            )],
            None,
        ),
        vec![clause_("ask", "get", None, &[], None, int(7))],
        None,
    );
    assert_eq!(run(&e).int(), 21);
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

/// ADR 0005's escape case, and required test 6: this is a success, not an error.
#[test]
fn a_continuation_captured_in_a_cell_region_still_reads_the_cell_after_the_region_returned() {
    let region = with_cell_(
        "s",
        int(0),
        "c",
        handle_(
            block(
                vec![
                    discard(perform_("esc", "grab", None, vec![])),
                    discard(cell_set(
                        var("c"),
                        bin(BinOp::Add, cell_get(var("c")), int(1)),
                    )),
                ],
                Some(cell_get(var("c"))),
            ),
            vec![clause_("esc", "grab", None, &[], Some("k"), var("k"))],
            None,
        ),
    );
    let e = block(
        vec![
            letv("k", region),
            letv("first", call(var("k"), vec![unit()])),
            letv("second", call(var("k"), vec![unit()])),
        ],
        Some(list(vec![var("first"), var("second")])),
    );

    let run = run(&e);
    assert_eq!(
        run.rendered(),
        "[1, 2]",
        "the cell was read outside its region"
    );
    assert_eq!(run.cell(0), 2);
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
fn a_continuation_applied_to_the_wrong_number_of_arguments_is_an_arity_mismatch() {
    for args in [vec![], vec![int(1), int(2)]] {
        let count = args.len();
        let e = handle_(
            perform_("amb", "flip", None, vec![]),
            vec![clause_(
                "amb",
                "flip",
                None,
                &[],
                Some("k"),
                call(var("k"), args),
            )],
            None,
        );
        let out = run(&e);
        let d = out.diagnostic();
        assert_eq!(d.code, codes::ARITY_MISMATCH);
        assert!(
            d.message.contains(&format!(
                "a continuation takes 1 argument, but {count} were given"
            )),
            "{}",
            d.message
        );
    }
}

#[test]
fn an_unhandled_operation_is_an_unhandled_effect() {
    let e = build::handle(
        build::perform("state", "get", None, vec![]),
        vec![build::clause("state", "put", None, &["v"], build::int(0))],
    );
    let d = standalone(
        vec![build::effect_def(
            "state",
            &[("get", Mode::Read, false), ("put", Mode::Write, false)],
        )],
        &e,
    )
    .expect_err("nothing handles `state.get`");
    assert_eq!(d.code, codes::UNHANDLED_EFFECT);
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
fn a_missing_resource_label_is_a_resource_required() {
    let e = build::handle(
        build::perform("db", "get", None, vec![build::int(0)]),
        vec![build::clause("db", "get", None, &["k"], build::int(0))],
    );
    let d = standalone(
        vec![build::effect_def("db", &[("get", Mode::Read, true)])],
        &e,
    )
    .expect_err("`db.get` is resource-parameterized");
    assert_eq!(d.code, codes::RESOURCE_REQUIRED);
}

#[test]
fn an_operation_the_effect_does_not_declare_is_an_unknown_operation() {
    let e = build::handle(
        build::perform("state", "peek", None, vec![]),
        vec![build::clause("state", "peek", None, &[], build::int(0))],
    );
    let d = standalone(
        vec![build::effect_def("state", &[("get", Mode::Read, false)])],
        &e,
    )
    .expect_err("`state` has no `peek`");
    assert_eq!(d.code, codes::UNKNOWN_OPERATION);
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
fn a_capture_costs_one_segment_per_enclosing_handler_and_not_one_per_frame() {
    for frames in [1, 64] {
        let mut body = perform_("amb", "flip", None, vec![]);
        for _ in 0..frames {
            body = bin(BinOp::Add, body, int(0));
        }
        let e = handle_(
            body,
            vec![clause_("amb", "flip", None, &[], Some("k"), var("k"))],
            None,
        );
        match run(&e).value() {
            Value::Continuation(k) => {
                assert_eq!(k.segments(), 1);
                assert_eq!(k.frames(), frames);
            }
            other => panic!("expected a continuation, got {other}"),
        }
    }
}

/// `map`'s loop is frames rather than host recursion precisely so this works: a continuation
/// captured inside the callback has somewhere to return to on the second resumption.
#[test]
fn a_continuation_captured_inside_a_map_callback_produces_a_complete_list_per_resumption() {
    let e = handle_(
        callv(
            "map",
            vec![
                list(vec![int(1), int(2), int(3)]),
                lam(
                    &["x"],
                    bin(BinOp::Add, var("x"), perform_("amb", "flip", None, vec![])),
                ),
            ],
        ),
        vec![clause_(
            "amb",
            "flip",
            None,
            &[],
            Some("k"),
            list(vec![
                call(var("k"), vec![int(10)]),
                call(var("k"), vec![int(20)]),
            ]),
        )],
        None,
    );

    // The first `flip` is performed at element 0, so each resumption rebuilds the whole list from
    // there with its own answer for that element and its own further performs answered by the same
    // clause.
    let got = run(&e).rendered();
    assert_eq!(
        got,
        "[[[[11, 12, 13], [11, 12, 23]], [[11, 22, 13], [11, 22, 23]]], \
[[[21, 12, 13], [21, 12, 23]], [[21, 22, 13], [21, 22, 23]]]]"
    );
}

#[test]
fn check_operation_accepts_an_effect_no_module_declares() {
    let effect = Symbol::new("mystery");
    let op = Symbol::new("go");
    assert!(check_operation(OpDecl::UnknownEffect, &effect, &op, false, sp()).is_ok());
}
