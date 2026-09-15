use crate::unit::build::*;
use ply_core::ty::Footprint;
use ply_eval::differential::*;
use ply_eval::{Arena, Fixture, Machine, Value};
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_syntax::ast::Expr;
use ply_syntax::ast::{BinOp, Item, Mode, Program};
use ply_syntax::resolve::Resolved;

/// A machine except where a test asks it not to be.
struct Perturbed<'a> {
    inner: Machine<'a>,
    /// Answers `eval_test` with this instead of running it.
    outcome: Option<Result<(), Diagnostic>>,
    /// Allocated into the arena after every test.
    extra_cell: Option<Value>,
    footprint: Option<Footprint>,
}

impl<'a> Perturbed<'a> {
    fn new(program: &'a Program, resolved: &'a Resolved) -> Perturbed<'a> {
        Perturbed {
            inner: Machine::for_program(program, resolved),
            outcome: None,
            extra_cell: None,
            footprint: None,
        }
    }
}

impl Evaluator for Perturbed<'_> {
    fn test_count(&self) -> usize {
        self.inner.test_count()
    }

    fn test_name(&self, index: usize) -> Option<&str> {
        self.inner.test_name(index)
    }

    fn eval_test(&mut self, index: usize) -> Result<(), Diagnostic> {
        let real = self.inner.eval_test(index);
        if let Some(extra) = self.extra_cell.clone() {
            self.inner.cells_mut().alloc(extra);
        }
        match &self.outcome {
            Some(forced) => forced.clone(),
            None => real,
        }
    }

    fn eval_test_in(&mut self, module: &Symbol, ordinal: usize) -> Result<(), Diagnostic> {
        self.inner.eval_test_in(module, ordinal)
    }

    fn eval_expr(&mut self, e: &Expr) -> Result<Value, Diagnostic> {
        self.inner.eval_expr_for_test(e)
    }

    fn cells(&self) -> &Arena {
        self.inner.cells()
    }

    fn cells_mut(&mut self) -> &mut Arena {
        self.inner.cells_mut()
    }

    fn set_fixture(&mut self, fixture: &Fixture) {
        let (regions, _) = fixture.open();
        self.inner.set_regions(regions);
    }

    fn observed_footprint(&self) -> Option<Footprint> {
        self.footprint.clone()
    }
}

fn corpus() -> Vec<Item> {
    vec![
        fn_def("two", &[], int(2)),
        test_def(
            "arithmetic agrees",
            callv("assert_eq", vec![callv("two", vec![]), int(2)]),
        ),
        test_def(
            "a cell survives the test",
            with_cell(
                "s",
                int(1),
                "c",
                block(
                    vec![discard(callv("cell_set", vec![var("c"), int(41)]))],
                    Some(callv(
                        "assert_eq",
                        vec![callv("cell_get", vec![var("c")]), int(41)],
                    )),
                ),
            ),
        ),
    ]
}

#[test]
fn two_honest_evaluators_over_one_corpus_report_nothing() {
    let (program, resolved) = standalone(corpus());
    let mut left = Machine::for_program(&program, &resolved);
    let mut right = Perturbed::new(&program, &resolved);
    let report = compare_tests(&mut left, &mut right, &Fixture::empty());
    assert_eq!(report.compared, 2);
    assert!(report.is_clean(), "{report}");
}

#[test]
fn a_divergence_only_in_a_label_span_is_caught() {
    let (program, resolved) = standalone(vec![test_def(
        "assertion",
        spanned(callv("assert_eq", vec![int(1), int(2)]), at(88, 100)),
    )]);
    let mut left = Machine::for_program(&program, &resolved);
    let mut right = Perturbed::new(&program, &resolved);

    let mut drifted = Machine::for_program(&program, &resolved)
        .eval_test(0)
        .unwrap_err();
    drifted.labels[0].span = at(1, 2);
    right.outcome = Some(Err(drifted));

    let report = compare_tests(&mut left, &mut right, &Fixture::empty());
    assert_eq!(
        report.divergences[0].detail,
        Detail::Diagnostic {
            field: "labels[0]".to_string()
        }
    );
    assert!(report.divergences[0].left.contains("88..100"));
}

/// The case a verdict comparison alone would miss entirely: both sides pass, and one of them
/// left its cells somewhere else.
#[test]
fn an_arena_that_differs_after_a_passing_test_is_caught_at_the_cell() {
    let (program, resolved) = standalone(corpus());
    let mut left = Machine::for_program(&program, &resolved);
    let mut right = Perturbed::new(&program, &resolved);
    right.extra_cell = Some(Value::Int(99));

    let report = compare_tests(&mut left, &mut right, &Fixture::empty());
    let d = report
        .divergences
        .first()
        .expect("an extra cell is a divergence");
    assert!(
        matches!(&d.detail, Detail::Cells { at } if at == "0"),
        "{:?}",
        d.detail
    );
    assert_eq!(d.left, "no such cell");
    assert_eq!(d.right, "99");
}

#[test]
fn an_arena_whose_contents_differ_names_the_cell_and_both_values() {
    let (program, resolved) = standalone(corpus());
    let mut left = Machine::for_program(&program, &resolved);
    let mut right = Perturbed::new(&program, &resolved);

    let seeded = Fixture::build(|r| Value::Cell(r.alloc_cell(Value::Int(0))));

    // `compare_tests` re-seeds both from its own base, so the divergence has to be injected
    // through the engine rather than through the fixture.
    right.extra_cell = Some(Value::Int(7));
    let report = compare_tests(&mut left, &mut right, &seeded);
    let d = &report.divergences[0];
    assert!(matches!(&d.detail, Detail::Cells { .. }), "{:?}", d.detail);
}

#[test]
fn footprints_are_compared_only_when_both_sides_traced_one() {
    let (program, resolved) = standalone(corpus());
    let mut left = Machine::for_program(&program, &resolved);
    let mut right = Perturbed::new(&program, &resolved);

    let report = compare_tests(&mut left, &mut right, &Fixture::empty());
    assert!(report.is_clean(), "{report}");
    assert_eq!(report.footprints_compared, 0);
}

#[test]
fn a_corpus_the_two_sides_disagree_on_the_size_of_stops_immediately() {
    let (program, resolved) = standalone(corpus());
    let (smaller, smaller_resolved) = standalone(vec![test_def("only one", int(1))]);
    let mut left = Machine::for_program(&program, &resolved);
    let mut right = Perturbed::new(&smaller, &smaller_resolved);

    let report = compare_tests(&mut left, &mut right, &Fixture::empty());
    assert_eq!(report.compared, 0);
    assert_eq!(report.divergences.len(), 1);
    assert_eq!(report.divergences[0].left, "2 tests");
}

#[test]
fn an_expression_comparison_reports_the_two_values() {
    let (program, resolved) = standalone(Vec::new());
    let mut left = Machine::for_program(&program, &resolved);
    let mut right = Perturbed::new(&program, &resolved);

    let agree = bin(BinOp::Add, int(1), int(2));
    assert!(compare_expr(&mut left, &mut right, "sum", &agree).is_none());
}

#[test]
fn a_divergence_becomes_a_failing_diagnostic_naming_both_sides() {
    let (program, resolved) = standalone(vec![test_def("t", int(1))]);
    let mut left = Machine::for_program(&program, &resolved);
    let mut right = Perturbed::new(&program, &resolved);
    right.outcome = Some(Err(Diagnostic::error(codes::RUNTIME_ERROR, "boom")));

    let report = compare_tests(&mut left, &mut right, &Fixture::empty());
    let d = report.into_result().unwrap_err();
    assert_eq!(d.code, codes::ENGINE_DIVERGENCE);
    assert!(d.message.contains("backend"), "{}", d.message);
    assert!(d.message.contains("machine"), "{}", d.message);
    assert!(d.notes.iter().any(|n| n.contains("boom")), "{:?}", d.notes);
}

/// Exercises the shapes a backend is most likely to get wrong: a handler answering
/// a perform, a cell written from a clause, all three higher-order builtins driving a closure
/// that itself performs, and four distinct failures whose diagnostics must match label for
/// label.
fn mixed_corpus() -> Vec<Item> {
    let state = effect_def("state", &[("get", Mode::Read, false)]);
    let handled =
        |body: Expr, answer: Expr| handle(body, vec![clause("state", "get", None, &[], answer)]);
    vec![
        state,
        fn_def("twice", &["x"], bin(BinOp::Mul, var("x"), int(2))),
        test_def(
            "arithmetic and calls",
            callv("assert_eq", vec![callv("twice", vec![int(21)]), int(42)]),
        ),
        test_def(
            "map over a performing closure",
            handled(
                callv(
                    "assert_eq",
                    vec![
                        callv(
                            "map",
                            vec![
                                list(vec![int(1), int(2)]),
                                lam(
                                    &["x"],
                                    bin(
                                        BinOp::Add,
                                        var("x"),
                                        perform("state", "get", None, vec![]),
                                    ),
                                ),
                            ],
                        ),
                        list(vec![int(11), int(12)]),
                    ],
                ),
                int(10),
            ),
        ),
        test_def(
            "filter and fold agree",
            callv(
                "assert_eq",
                vec![
                    callv(
                        "fold",
                        vec![
                            callv(
                                "filter",
                                vec![
                                    callv("range", vec![int(0), int(6)]),
                                    lam(
                                        &["x"],
                                        bin(BinOp::Eq, bin(BinOp::Rem, var("x"), int(2)), int(0)),
                                    ),
                                ],
                            ),
                            int(0),
                            lam(&["acc", "x"], bin(BinOp::Add, var("acc"), var("x"))),
                        ],
                    ),
                    int(6),
                ],
            ),
        ),
        test_def(
            "a cell written from a clause",
            with_cell(
                "s",
                int(0),
                "c",
                block(
                    vec![discard(handle(
                        block(
                            vec![
                                discard(perform("state", "get", None, vec![])),
                                discard(perform("state", "get", None, vec![])),
                            ],
                            None,
                        ),
                        vec![clause(
                            "state",
                            "get",
                            None,
                            &[],
                            callv(
                                "cell_set",
                                vec![
                                    var("c"),
                                    bin(BinOp::Add, callv("cell_get", vec![var("c")]), int(1)),
                                ],
                            ),
                        )],
                    ))],
                    Some(callv(
                        "assert_eq",
                        vec![callv("cell_get", vec![var("c")]), int(2)],
                    )),
                ),
            ),
        ),
        test_def(
            "a failing assertion",
            spanned(
                callv(
                    "assert_eq",
                    vec![list(vec![int(1), int(2)]), list(vec![int(1), int(3)])],
                ),
                at(88, 100),
            ),
        ),
        test_def("an unhandled effect", perform("state", "get", None, vec![])),
        test_def("a panic", callv("panic", vec![string("boom")])),
        test_def("an arity mismatch", callv("twice", vec![int(1), int(2)])),
    ]
}

#[test]
fn two_real_machines_agree_over_a_mixed_corpus() {
    let (program, resolved) = standalone(mixed_corpus());
    let mut plain = Machine::for_program(&program, &resolved);
    let mut machine = Machine::for_program(&program, &resolved);

    let report = compare_tests(&mut plain, &mut machine, &Fixture::empty());
    assert_eq!(report.compared, 8);
    assert!(report.is_clean(), "{report}");
}

#[test]
fn a_seeded_fixture_reaches_both_sides() {
    let (program, resolved) = standalone(vec![test_def("t", int(1))]);
    let mut left = Machine::for_program(&program, &resolved);
    let mut right = Perturbed::new(&program, &resolved);

    let seeded = Fixture::build(|r| Value::Cell(r.alloc_cell(Value::str("fixture"))));
    let cell = seeded
        .handle()
        .as_cell(Span::DUMMY, "the fixture handle")
        .expect("a cell");

    let report = compare_tests(&mut left, &mut right, &seeded);
    assert!(report.is_clean(), "{report}");
    assert_eq!(left.cells().get(cell).unwrap().render(), "\"fixture\"");
    assert_eq!(right.cells().get(cell).unwrap().render(), "\"fixture\"");
}
