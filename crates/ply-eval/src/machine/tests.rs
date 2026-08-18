use crate::build::*;
use crate::differential::compare_expr;
use crate::interp::Interp;
use crate::machine::{Machine, Progress};
use crate::task_regions::Fixture;
use crate::value::Value;
use ply_span::{Diagnostic, Span, codes};
use ply_syntax::ast::{BinOp, Expr, Item, Mode, UnOp};

fn eval_in(items: Vec<Item>, e: Expr) -> Result<Value, Diagnostic> {
    let (program, resolved) = standalone(items);
    Machine::for_program(&program, &resolved).eval_expr_for_test(&e)
}

fn eval(e: Expr) -> Result<Value, Diagnostic> {
    eval_in(Vec::new(), e)
}

fn eval_frames(items: Vec<Item>, e: Expr, max: usize) -> Result<Value, Diagnostic> {
    let (program, resolved) = standalone(items);
    Machine::for_program(&program, &resolved)
        .with_max_frames(max)
        .eval_expr_for_test(&e)
}

fn eval_calls(items: Vec<Item>, e: Expr, max: usize) -> Result<Value, Diagnostic> {
    let (program, resolved) = standalone(items);
    Machine::for_program(&program, &resolved)
        .with_max_calls(max)
        .eval_expr_for_test(&e)
}

#[track_caller]
fn ok_int(e: Expr) -> i64 {
    match eval(e) {
        Ok(Value::Int(i)) => i,
        other => panic!("expected an Int, got {other:?}"),
    }
}

#[track_caller]
fn rendered(items: Vec<Item>, e: Expr) -> String {
    match eval_in(items, e) {
        Ok(v) => v.render(),
        Err(d) => panic!("expected a value, got {d}"),
    }
}

#[track_caller]
fn err(e: Expr) -> Diagnostic {
    err_in(Vec::new(), e)
}

#[track_caller]
fn err_in(items: Vec<Item>, e: Expr) -> Diagnostic {
    match eval_in(items, e) {
        Err(d) => d,
        Ok(v) => panic!("expected a diagnostic, got {v}"),
    }
}

fn shapes() -> Item {
    type_def("Shape", &[("Circle", 1), ("Rect", 2), ("Empty", 0)])
}

fn state_effect() -> Item {
    effect_def(
        "state",
        &[("get", Mode::Read, false), ("put", Mode::Write, false)],
    )
}

/// `down` is tail-recursive: its recursive call is the whole of the branch it
/// sits in, so nothing is pending when it is made.
fn tail_recursive() -> Item {
    fn_def(
        "down",
        &["n"],
        if_(
            bin(BinOp::Le, var("n"), int(0)),
            int(0),
            callv("down", vec![bin(BinOp::Sub, var("n"), int(1))]),
        ),
    )
}

/// `sum` is not: the addition is waiting on the call, so every level has to be
/// kept.
fn non_tail_recursive() -> Item {
    fn_def(
        "sum",
        &["n"],
        if_(
            bin(BinOp::Le, var("n"), int(0)),
            int(0),
            bin(
                BinOp::Add,
                var("n"),
                callv("sum", vec![bin(BinOp::Sub, var("n"), int(1))]),
            ),
        ),
    )
}

#[test]
fn literals_evaluate_to_themselves() {
    assert_eq!(ok_int(int(42)), 42);
    assert!(matches!(eval(boolean(true)), Ok(Value::Bool(true))));
    assert!(matches!(eval(unit()), Ok(Value::Unit)));
    assert_eq!(rendered(Vec::new(), string("hi")), "\"hi\"");
}

#[test]
fn arithmetic_respects_operator_semantics() {
    assert_eq!(
        ok_int(bin(BinOp::Add, int(2), bin(BinOp::Mul, int(3), int(4)),)),
        14
    );
    assert_eq!(ok_int(bin(BinOp::Rem, int(7), int(3))), 1);
    assert_eq!(ok_int(bin(BinOp::Div, int(7), int(2))), 3);
}

#[test]
fn comparison_and_equality_produce_booleans() {
    assert!(matches!(
        eval(bin(BinOp::Lt, int(1), int(2))),
        Ok(Value::Bool(true))
    ));
    assert!(matches!(
        eval(bin(BinOp::Eq, list(vec![int(1)]), list(vec![int(1)]))),
        Ok(Value::Bool(true))
    ));
    assert!(matches!(
        eval(bin(BinOp::Ne, string("a"), string("b"))),
        Ok(Value::Bool(true))
    ));
}

#[test]
fn string_concatenation_joins_its_operands() {
    assert_eq!(
        rendered(Vec::new(), bin(BinOp::Concat, string("ab"), string("cd"))),
        "\"abcd\""
    );
}

#[test]
fn unary_operators_negate_and_invert() {
    assert_eq!(ok_int(un(UnOp::Neg, int(5))), -5);
    assert!(matches!(
        eval(un(UnOp::Not, boolean(false))),
        Ok(Value::Bool(true))
    ));
}

/// The right operand must not be evaluated at all, which `panic` proves by
/// being a diagnostic if it ever runs.
#[test]
fn logical_operators_short_circuit() {
    let boom = callv("panic", vec![string("evaluated")]);
    assert!(matches!(
        eval(bin(BinOp::And, boolean(false), boom.clone())),
        Ok(Value::Bool(false))
    ));
    assert!(matches!(
        eval(bin(BinOp::Or, boolean(true), boom)),
        Ok(Value::Bool(true))
    ));
    assert!(matches!(
        eval(bin(BinOp::And, boolean(true), boolean(false))),
        Ok(Value::Bool(false))
    ));
    assert!(matches!(
        eval(bin(BinOp::Or, boolean(false), boolean(true))),
        Ok(Value::Bool(true))
    ));
}

#[test]
fn a_non_boolean_operand_of_a_logical_operator_names_its_own_side() {
    let d = err(bin(BinOp::And, int(1), boolean(true)));
    assert_eq!(d.code, codes::RUNTIME_ERROR);
    assert!(d.message.contains("a logical operator"), "{}", d.message);

    let d = err(bin(BinOp::And, boolean(true), int(1)));
    assert!(d.message.contains("a logical operator"), "{}", d.message);
}

#[test]
fn let_bindings_are_visible_to_the_rest_of_the_block() {
    let e = block(
        vec![
            letv("x", int(2)),
            letv("y", bin(BinOp::Mul, var("x"), int(3))),
        ],
        Some(bin(BinOp::Add, var("x"), var("y"))),
    );
    assert_eq!(ok_int(e), 8);
}

#[test]
fn a_block_without_a_tail_evaluates_to_unit() {
    assert!(matches!(
        eval(block(vec![discard(int(1))], None)),
        Ok(Value::Unit)
    ));
}

#[test]
fn a_later_let_shadows_an_earlier_one() {
    let e = block(vec![letv("x", int(1)), letv("x", int(2))], Some(var("x")));
    assert_eq!(ok_int(e), 2);
}

#[test]
fn a_lambda_captures_the_environment_at_creation() {
    let e = block(
        vec![
            letv("n", int(10)),
            letv("f", lam(&["x"], bin(BinOp::Add, var("x"), var("n")))),
            letv("n", int(99)),
        ],
        Some(call(var("f"), vec![int(1)])),
    );
    assert_eq!(ok_int(e), 11);
}

#[test]
fn a_closure_returned_from_a_function_keeps_its_capture() {
    let items = vec![fn_def(
        "adder",
        &["n"],
        lam(&["x"], bin(BinOp::Add, var("x"), var("n"))),
    )];
    let e = call(callv("adder", vec![int(7)]), vec![int(5)]);
    assert!(matches!(eval_in(items, e), Ok(Value::Int(12))));
}

#[test]
fn arguments_are_evaluated_left_to_right() {
    let items = vec![fn_def(
        "note",
        &["c", "n"],
        block(
            vec![discard(callv(
                "cell_set",
                vec![
                    var("c"),
                    callv("push", vec![callv("cell_get", vec![var("c")]), var("n")]),
                ],
            ))],
            Some(var("n")),
        ),
    )];
    let e = with_cell(
        "order",
        list(Vec::new()),
        "c",
        block(
            vec![discard(callv(
                "string_concat",
                vec![
                    callv("int_to_string", vec![callv("note", vec![var("c"), int(1)])]),
                    callv("int_to_string", vec![callv("note", vec![var("c"), int(2)])]),
                ],
            ))],
            Some(callv("cell_get", vec![var("c")])),
        ),
    );
    assert_eq!(rendered(items, e), "[1, 2]");
}

#[test]
fn if_takes_the_branch_its_condition_selected() {
    assert_eq!(ok_int(if_(boolean(true), int(1), int(2))), 1);
    assert_eq!(ok_int(if_(boolean(false), int(1), int(2))), 2);
}

#[test]
fn a_non_boolean_condition_points_at_the_condition() {
    let d = err(if_(int(1), int(1), int(2)));
    assert!(d.message.contains("`if` condition"), "{}", d.message);
}

#[test]
fn records_build_and_project() {
    let e = field(record(vec![("a", int(1)), ("b", int(2))]), "b");
    assert_eq!(ok_int(e), 2);
    assert_eq!(rendered(Vec::new(), record(Vec::new())), "{}");
}

#[test]
fn a_record_renders_its_fields_in_key_order() {
    let e = record(vec![("b", int(2)), ("a", int(1))]);
    assert_eq!(rendered(Vec::new(), e), "{a: 1, b: 2}");
}

#[test]
fn a_missing_field_lists_the_ones_that_exist() {
    let d = err(field(record(vec![("a", int(1))]), "z"));
    assert_eq!(d.code, codes::UNKNOWN_NAME);
    assert!(d.notes.iter().any(|n| n.contains("`a`")), "{:?}", d.notes);
}

#[test]
fn field_access_on_a_non_record_is_a_type_error() {
    let d = err(field(int(1), "a"));
    assert_eq!(d.code, codes::RUNTIME_ERROR);
    assert!(d.message.contains("field access"), "{}", d.message);
}

#[test]
fn lists_evaluate_every_item() {
    assert_eq!(
        rendered(
            Vec::new(),
            list(vec![int(1), bin(BinOp::Add, int(1), int(1)), int(3)])
        ),
        "[1, 2, 3]"
    );
    assert_eq!(rendered(Vec::new(), list(Vec::new())), "[]");
}

#[test]
fn a_constructor_applied_to_arguments_builds_a_variant() {
    assert_eq!(
        rendered(vec![shapes()], callv("Rect", vec![int(2), int(3)])),
        "Rect(2, 3)"
    );
    assert_eq!(rendered(vec![shapes()], var("Empty")), "Empty");
}

#[test]
fn calling_a_non_function_is_reported() {
    let d = err(call(int(1), vec![int(2)]));
    assert_eq!(d.code, codes::NOT_A_FUNCTION);
}

#[test]
fn an_unknown_name_is_reported_with_its_span() {
    let d = err(var_at("nope", at(4, 8)));
    assert_eq!(d.code, codes::UNKNOWN_NAME);
    assert_eq!(d.primary_span(), Some(at(4, 8)));
}

#[test]
fn the_wrong_argument_count_is_an_arity_mismatch() {
    let items = vec![fn_def("f", &["a", "b"], int(0))];
    let d = err_in(items, callv("f", vec![int(1)]));
    assert_eq!(d.code, codes::ARITY_MISMATCH);
}

#[test]
fn division_by_zero_and_overflow_are_diagnostics() {
    let d = err(bin(BinOp::Div, int(1), int(0)));
    assert_eq!(d.code, codes::RUNTIME_ERROR);
    let d = err(bin(BinOp::Mul, int(i64::MAX), int(2)));
    assert!(d.message.contains("overflow"), "{}", d.message);
}

#[test]
fn literal_patterns_match_by_value() {
    let e = match_(
        int(2),
        vec![
            arm(pint(1), int(10)),
            arm(pint(2), int(20)),
            arm(pwild(), int(30)),
        ],
    );
    assert_eq!(ok_int(e), 20);
}

#[test]
fn string_bool_and_unit_literal_patterns_match() {
    let e = match_(
        string("b"),
        vec![arm(pstr("a"), int(1)), arm(pstr("b"), int(2))],
    );
    assert_eq!(ok_int(e), 2);
    let e = match_(
        boolean(false),
        vec![arm(pbool(true), int(1)), arm(pbool(false), int(2))],
    );
    assert_eq!(ok_int(e), 2);
    let e = match_(unit(), vec![arm(punit(), int(7))]);
    assert_eq!(ok_int(e), 7);
}

#[test]
fn nested_constructor_patterns_bind_inner_values() {
    let items = vec![shapes()];
    let e = match_(
        callv("Rect", vec![int(3), int(4)]),
        vec![
            arm(pctor("Circle", vec![pvar("r")]), var("r")),
            arm(
                pctor("Rect", vec![pvar("w"), pvar("h")]),
                bin(BinOp::Mul, var("w"), var("h")),
            ),
        ],
    );
    assert!(matches!(eval_in(items, e), Ok(Value::Int(12))));
}

#[test]
fn a_constructor_pattern_nested_inside_a_list_pattern_binds_through_both() {
    let items = vec![shapes()];
    let e = match_(
        list(vec![callv("Circle", vec![int(9)])]),
        vec![
            arm(
                plist(vec![pctor("Circle", vec![pvar("r")])], None),
                var("r"),
            ),
            arm(pwild(), int(0)),
        ],
    );
    assert!(matches!(eval_in(items, e), Ok(Value::Int(9))));
}

#[test]
fn a_nullary_constructor_pattern_tests_rather_than_binds() {
    let items = vec![shapes()];
    let e = match_(
        callv("Circle", vec![int(1)]),
        vec![arm(pvar("Empty"), int(100)), arm(pwild(), int(200))],
    );
    assert!(matches!(eval_in(items, e), Ok(Value::Int(200))));
}

#[test]
fn list_patterns_split_head_and_tail() {
    let e = match_(
        list(vec![int(1), int(2), int(3)]),
        vec![
            arm(plist(Vec::new(), None), int(0)),
            arm(
                plist(vec![pvar("h")], Some(pvar("t"))),
                bin(BinOp::Add, var("h"), callv("len", vec![var("t")])),
            ),
        ],
    );
    assert_eq!(ok_int(e), 3);
}

#[test]
fn a_list_pattern_without_a_rest_requires_an_exact_length() {
    let e = match_(
        list(vec![int(1), int(2)]),
        vec![
            arm(plist(vec![pvar("a")], None), int(1)),
            arm(pwild(), int(2)),
        ],
    );
    assert_eq!(ok_int(e), 2);
}

#[test]
fn record_patterns_honour_the_rest_flag() {
    let value = record(vec![("a", int(1)), ("b", int(2))]);
    let exact = match_(
        value.clone(),
        vec![
            arm(prec(vec![("a", pvar("x"))], false), var("x")),
            arm(pwild(), int(0)),
        ],
    );
    assert_eq!(ok_int(exact), 0);

    let with_rest = match_(
        value,
        vec![
            arm(prec(vec![("a", pvar("x"))], true), var("x")),
            arm(pwild(), int(0)),
        ],
    );
    assert_eq!(ok_int(with_rest), 1);
}

#[test]
fn a_guard_rejects_an_otherwise_matching_arm_and_a_later_arm_takes_it() {
    let e = match_(
        int(5),
        vec![
            guarded(pvar("n"), bin(BinOp::Gt, var("n"), int(10)), int(1)),
            guarded(pvar("n"), bin(BinOp::Gt, var("n"), int(3)), int(2)),
            arm(pwild(), int(3)),
        ],
    );
    assert_eq!(ok_int(e), 2);
}

#[test]
fn bindings_from_a_rejected_arm_do_not_leak() {
    let e = block(
        vec![letv("x", int(1))],
        Some(match_(
            int(9),
            vec![
                guarded(pvar("x"), boolean(false), int(0)),
                arm(pwild(), var("x")),
            ],
        )),
    );
    assert_eq!(ok_int(e), 1);
}

#[test]
fn an_unmatched_scrutinee_is_a_non_exhaustive_match() {
    let d = err(match_(int(3), vec![arm(pint(1), int(1))]));
    assert_eq!(d.code, codes::NON_EXHAUSTIVE_MATCH);
}

#[test]
fn a_refutable_let_that_fails_is_a_diagnostic() {
    let e = block(vec![let_(pint(1), int(2))], Some(int(0)));
    let d = err(e);
    assert_eq!(d.code, codes::NON_EXHAUSTIVE_MATCH);
}

#[test]
fn deep_non_tail_recursion_is_a_diagnostic_not_a_crash() {
    let d = match eval_frames(
        vec![non_tail_recursive()],
        callv("sum", vec![int(100_000)]),
        64,
    ) {
        Err(d) => d,
        Ok(v) => panic!("expected a recursion diagnostic, got {v}"),
    };
    assert_eq!(d.code, codes::RUNTIME_ERROR);
    assert!(d.message.contains("recursion limit"), "{}", d.message);
}

/// At the bound's real order of magnitude, where reporting the failure means
/// unwinding a stack of a quarter of a million frames rather than sixty.
#[test]
fn the_frame_bound_holds_at_the_scale_it_is_set_for() {
    let out = eval_frames(
        vec![non_tail_recursive()],
        callv("sum", vec![int(1_000_000)]),
        250_000,
    );
    let d = out.expect_err("a million levels needs more than 250,000 frames");
    assert!(d.message.contains("recursion limit"), "{}", d.message);
}

#[test]
fn the_recursion_diagnostic_names_the_innermost_calls() {
    let d = eval_frames(
        vec![non_tail_recursive()],
        callv("sum", vec![int(100_000)]),
        64,
    )
    .expect_err("the limit is exceeded");
    let named = d
        .notes
        .iter()
        .find(|n| n.starts_with("innermost calls:"))
        .unwrap_or_else(|| panic!("no innermost-call note in {:?}", d.notes));
    assert!(named.contains("`sum`"), "{named}");
}

/// A tail call is charged against the call budget exactly once, like every
/// other call.
///
/// It used to reuse the frame it would have returned through, which made a
/// tail-recursive loop run in constant space — and made a tail-recursive
/// *runaway* unbounded, where the tree-walker diagnosed it in milliseconds.
/// Two engines that ship together answer one program one way, so the elision
/// went. Charged once and not twice is what is left to assert: `down(n)` fits
/// in `n + 1` calls and does not fit in `n`.
#[test]
fn a_tail_call_costs_exactly_one_call() {
    let out = eval_calls(vec![tail_recursive()], callv("down", vec![int(200)]), 201);
    assert!(matches!(out, Ok(Value::Int(0))), "{out:?}");

    let out = eval_calls(vec![tail_recursive()], callv("down", vec![int(200)]), 200);
    let d = out.expect_err("two hundred calls do not fit in a budget of two hundred");
    assert!(d.message.contains("nested calls"), "{}", d.message);
}

#[test]
fn a_tail_call_from_a_block_tail_and_a_match_arm_is_charged_the_same() {
    let items = vec![fn_def(
        "walk",
        &["n"],
        match_(
            bin(BinOp::Le, var("n"), int(0)),
            vec![
                arm(pbool(true), int(0)),
                arm(
                    pwild(),
                    block(
                        vec![letv("m", bin(BinOp::Sub, var("n"), int(1)))],
                        Some(callv("walk", vec![var("m")])),
                    ),
                ),
            ],
        ),
    )];
    let out = eval_calls(items.clone(), callv("walk", vec![int(200)]), 201);
    assert!(matches!(out, Ok(Value::Int(0))), "{out:?}");

    let out = eval_calls(items, callv("walk", vec![int(200)]), 200);
    assert!(out.is_err(), "{out:?}");
}

#[test]
fn sequential_calls_do_not_accumulate_frames() {
    let mut body = int(0);
    for _ in 0..20 {
        body = bin(BinOp::Add, body, callv("sum", vec![int(10)]));
    }
    let out = eval_frames(vec![non_tail_recursive()], body, 64);
    assert!(matches!(out, Ok(Value::Int(1100))), "{out:?}");
}

/// Nothing about a Ply call touches the native stack, so a recursion far deeper
/// than any host frame budget still returns a value on a thread whose stack is
/// a quarter of a mebibyte. The call budget is raised for it because this is
/// about the native stack and not about the semantic bound.
#[test]
fn deep_recursion_does_not_touch_the_native_stack() {
    let total = std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(|| {
            let (program, resolved) = standalone(vec![non_tail_recursive()]);
            let out = Machine::for_program(&program, &resolved)
                .with_max_calls(50_000)
                .eval_expr_for_test(&callv("sum", vec![int(20_000)]));
            match out {
                Ok(Value::Int(n)) => n,
                other => panic!("expected a sum, got {other:?}"),
            }
        })
        .expect("failed to spawn")
        .join()
        .expect("the machine overflowed the thread stack");
    assert_eq!(total, 200_010_000);
}

#[test]
fn a_failed_evaluation_leaves_nothing_behind_for_the_next_one() {
    let (program, resolved) = standalone(vec![non_tail_recursive()]);
    let mut machine = Machine::for_program(&program, &resolved).with_max_frames(64);
    assert!(
        machine
            .eval_expr_for_test(&callv("sum", vec![int(100_000)]))
            .is_err()
    );
    assert!(matches!(
        machine.eval_expr_for_test(&callv("sum", vec![int(3)])),
        Ok(Value::Int(6))
    ));
}

#[test]
fn stepping_reports_halted_once_the_program_is_done_and_stays_there() {
    let (program, resolved) = standalone(Vec::new());
    let mut machine = Machine::for_program(&program, &resolved);
    assert!(matches!(
        machine.eval_expr_for_test(&int(1)),
        Ok(Value::Int(1))
    ));
    assert!(matches!(
        machine.step(),
        Ok(Progress::Halted(Value::Int(1)))
    ));
    assert!(matches!(
        machine.step(),
        Ok(Progress::Halted(Value::Int(1)))
    ));
}

#[test]
fn a_cell_round_trips_through_get_and_set() {
    let e = with_cell(
        "s",
        int(1),
        "c",
        block(
            vec![discard(callv("cell_set", vec![var("c"), int(9)]))],
            Some(callv("cell_get", vec![var("c")])),
        ),
    );
    assert_eq!(ok_int(e), 9);
}

#[test]
fn every_entry_point_resets_to_the_fixture() {
    let (program, resolved) = standalone(Vec::new());
    let fixture = Fixture::build(|r| Value::Cell(r.alloc_cell(Value::Int(7))));
    let (regions, handle) = fixture.open();
    let seeded = handle
        .as_cell(ply_span::Span::DUMMY, "the fixture handle")
        .expect("a cell");

    let mut machine = Machine::for_program(&program, &resolved);
    machine.set_regions(regions);

    let read = with_cell("s", int(0), "c", callv("cell_get", vec![var("c")]));
    assert!(matches!(
        machine.eval_expr_for_test(&read),
        Ok(Value::Int(0))
    ));
    assert!(matches!(machine.cells().get(seeded), Some(Value::Int(7))));

    // The second entry point must not see the first one's allocation.
    let before = machine.cells().live();
    assert!(matches!(
        machine.eval_expr_for_test(&read),
        Ok(Value::Int(0))
    ));
    assert_eq!(machine.cells().live(), before);
}

#[test]
fn a_handler_clause_answers_the_perform_site() {
    let items = vec![state_effect()];
    let e = handle(
        bin(
            BinOp::Add,
            perform("state", "get", None, vec![int(0)]),
            int(1),
        ),
        vec![clause("state", "get", None, &["k"], int(41))],
    );
    assert!(matches!(eval_in(items, e), Ok(Value::Int(42))));
}

#[test]
fn a_handler_that_performs_the_operation_it_handles_reaches_the_next_handler_out() {
    let items = vec![state_effect()];
    let inner = handle(
        perform("state", "get", None, vec![int(0)]),
        vec![clause(
            "state",
            "get",
            None,
            &["k"],
            bin(
                BinOp::Add,
                perform("state", "get", None, vec![int(0)]),
                int(1),
            ),
        )],
    );
    let e = handle(inner, vec![clause("state", "get", None, &["k"], int(10))]);
    assert!(matches!(eval_in(items, e), Ok(Value::Int(11))));
}

#[test]
fn a_return_clause_transforms_the_bodys_value() {
    let items = vec![state_effect()];
    let e = handle_ret(
        perform("state", "get", None, vec![int(0)]),
        vec![clause("state", "get", None, &["k"], int(5))],
        "x",
        bin(BinOp::Mul, var("x"), int(2)),
    );
    assert!(matches!(eval_in(items, e), Ok(Value::Int(10))));
}

#[test]
fn an_unhandled_operation_names_the_atom_it_could_not_discharge() {
    let items = vec![state_effect()];
    let d = err_in(items, perform("state", "get", None, vec![int(0)]));
    assert_eq!(d.code, codes::UNHANDLED_EFFECT);
    assert!(d.message.contains("state.get"), "{}", d.message);
}

#[test]
fn map_filter_and_fold_drive_user_closures() {
    let xs = list(vec![int(1), int(2), int(3), int(4)]);
    let doubled = callv(
        "map",
        vec![xs.clone(), lam(&["x"], bin(BinOp::Mul, var("x"), int(2)))],
    );
    assert_eq!(rendered(Vec::new(), doubled), "[2, 4, 6, 8]");

    let evens = callv(
        "filter",
        vec![
            xs.clone(),
            lam(
                &["x"],
                bin(BinOp::Eq, bin(BinOp::Rem, var("x"), int(2)), int(0)),
            ),
        ],
    );
    assert_eq!(rendered(Vec::new(), evens), "[2, 4]");

    let total = callv(
        "fold",
        vec![
            xs,
            int(0),
            lam(&["a", "x"], bin(BinOp::Add, var("a"), var("x"))),
        ],
    );
    assert_eq!(ok_int(total), 10);
}

/// `map`'s loop is a frame rather than host recursion, so the callback can
/// perform an effect and the clause that answers it is reached through the
/// stack like any other.
#[test]
fn a_closure_passed_to_map_may_perform_an_effect() {
    let items = vec![state_effect()];
    let e = handle(
        callv(
            "map",
            vec![
                list(vec![int(1), int(2)]),
                lam(&["x"], perform("state", "get", None, vec![var("x")])),
            ],
        ),
        vec![clause(
            "state",
            "get",
            None,
            &["k"],
            bin(BinOp::Mul, var("k"), int(10)),
        )],
    );
    assert_eq!(rendered(items, e), "[10, 20]");
}

#[test]
fn a_test_is_reachable_by_index_and_by_module_ordinal() {
    let items = vec![
        test_def("passes", callv("assert", vec![boolean(true)])),
        test_def("fails", callv("assert", vec![boolean(false)])),
    ];
    let (program, resolved) = standalone(items);
    let mut machine = Machine::for_program(&program, &resolved);

    assert_eq!(machine.test_count(), 2);
    assert_eq!(machine.test_name(1), Some("fails"));
    assert!(machine.eval_test(0).is_ok());

    let d = machine.eval_test(1).expect_err("the second test fails");
    assert_eq!(d.code, codes::ASSERTION_FAILED);

    let anonymous = program.modules[0].name.as_symbol().clone();
    assert!(machine.eval_test_in(&anonymous, 0).is_ok());
    assert!(machine.eval_test_in(&anonymous, 5).is_err());
}

#[test]
fn call_invokes_a_definition_by_its_program_wide_name() {
    let items = vec![fn_def("twice", &["n"], bin(BinOp::Mul, var("n"), int(2)))];
    let (program, resolved) = standalone(items);
    let mut machine = Machine::for_program(&program, &resolved);
    let out = machine.call("twice", vec![Value::Int(21)], Span::DUMMY);
    assert!(matches!(out, Ok(Value::Int(42))), "{out:?}");
    assert!(machine.call("nope", Vec::new(), Span::DUMMY).is_err());
}

/// Every expression form, run on both engines and compared by the same
/// full-equality rule `--engine both` uses. A divergence here is the defect the
/// differential harness exists to catch, caught at unit-test cost.
#[test]
fn the_two_engines_agree_on_every_expression_form() {
    let items = vec![
        shapes(),
        state_effect(),
        non_tail_recursive(),
        fn_def(
            "adder",
            &["n"],
            lam(&["x"], bin(BinOp::Add, var("x"), var("n"))),
        ),
    ];
    let (program, resolved) = standalone(items);

    let subjects: Vec<(&str, Expr)> = vec![
        ("literal", int(1)),
        ("unary", un(UnOp::Neg, int(3))),
        (
            "binary",
            bin(BinOp::Add, int(1), bin(BinOp::Mul, int(2), int(3))),
        ),
        ("divide by zero", bin(BinOp::Div, int(1), int(0))),
        ("overflow", bin(BinOp::Add, int(i64::MAX), int(1))),
        ("short circuit", bin(BinOp::Or, boolean(true), int(1))),
        (
            "bad logical operand",
            bin(BinOp::And, int(1), boolean(true)),
        ),
        (
            "comparison of functions",
            bin(BinOp::Eq, var("adder"), var("adder")),
        ),
        ("block", block(vec![letv("x", int(2))], Some(var("x")))),
        (
            "refutable let",
            block(vec![let_(pint(1), int(2))], Some(int(0))),
        ),
        (
            "lambda application",
            call(callv("adder", vec![int(1)]), vec![int(2)]),
        ),
        ("arity mismatch", callv("adder", Vec::new())),
        ("not a function", call(int(1), vec![int(1)])),
        ("unknown name", var("missing")),
        ("if", if_(bin(BinOp::Lt, int(1), int(2)), int(10), int(20))),
        ("bad condition", if_(int(1), int(0), int(0))),
        (
            "match",
            match_(
                callv("Rect", vec![int(2), int(3)]),
                vec![
                    arm(pctor("Circle", vec![pvar("r")]), var("r")),
                    guarded(
                        pctor("Rect", vec![pvar("w"), pvar("h")]),
                        bin(BinOp::Gt, var("w"), int(5)),
                        int(0),
                    ),
                    arm(
                        pctor("Rect", vec![pvar("w"), pvar("h")]),
                        bin(BinOp::Mul, var("w"), var("h")),
                    ),
                ],
            ),
        ),
        ("non exhaustive", match_(int(3), vec![arm(pint(1), int(1))])),
        (
            "nullary ctor pattern",
            match_(
                var("Empty"),
                vec![arm(pvar("Empty"), int(1)), arm(pwild(), int(2))],
            ),
        ),
        (
            "list pattern",
            match_(
                list(vec![int(1), int(2)]),
                vec![
                    arm(
                        plist(vec![pvar("h")], Some(pvar("t"))),
                        callv("len", vec![var("t")]),
                    ),
                    arm(pwild(), int(0)),
                ],
            ),
        ),
        (
            "record pattern",
            match_(
                record(vec![("a", int(1)), ("b", int(2))]),
                vec![
                    arm(prec(vec![("a", pvar("x"))], true), var("x")),
                    arm(pwild(), int(0)),
                ],
            ),
        ),
        ("record", field(record(vec![("a", int(1))]), "a")),
        ("missing field", field(record(vec![("a", int(1))]), "z")),
        ("field of a non record", field(int(1), "a")),
        ("list", list(vec![int(1), int(2)])),
        (
            "map",
            callv(
                "map",
                vec![
                    list(vec![int(1), int(2)]),
                    lam(&["x"], bin(BinOp::Add, var("x"), int(1))),
                ],
            ),
        ),
        (
            "filter",
            callv(
                "filter",
                vec![
                    list(vec![int(1), int(2)]),
                    lam(&["x"], bin(BinOp::Gt, var("x"), int(1))),
                ],
            ),
        ),
        (
            "fold",
            callv(
                "fold",
                vec![
                    list(vec![int(1), int(2)]),
                    int(0),
                    lam(&["a", "x"], bin(BinOp::Add, var("a"), var("x"))),
                ],
            ),
        ),
        (
            "assert_eq failure",
            callv("assert_eq", vec![int(1), int(2)]),
        ),
        ("panic", callv("panic", vec![string("boom")])),
        ("recursion", callv("sum", vec![int(50)])),
        (
            "cells",
            with_cell(
                "s",
                int(1),
                "c",
                block(
                    vec![discard(callv("cell_set", vec![var("c"), int(4)]))],
                    Some(callv("cell_get", vec![var("c")])),
                ),
            ),
        ),
        (
            "handler",
            handle_ret(
                bin(
                    BinOp::Add,
                    perform("state", "get", None, vec![int(2)]),
                    int(1),
                ),
                vec![clause(
                    "state",
                    "get",
                    None,
                    &["k"],
                    bin(BinOp::Mul, var("k"), int(3)),
                )],
                "x",
                bin(BinOp::Add, var("x"), int(100)),
            ),
        ),
        ("unhandled", perform("state", "get", None, vec![int(0)])),
    ];

    let mut treewalk = Interp::for_program(&program, &resolved);
    let mut machine = Machine::for_program(&program, &resolved);
    let mut found = Vec::new();
    for (name, e) in &subjects {
        if let Some(d) = compare_expr(&mut treewalk, &mut machine, name, e) {
            found.push(d.to_string());
        }
    }
    assert!(found.is_empty(), "{}", found.join("\n"));
}

// ------------------------------------------------------- lowering, once each

#[track_caller]
fn answer(machine: &mut Machine<'_>) -> i64 {
    match machine.call("f", Vec::new(), sp()) {
        Ok(Value::Int(i)) => i,
        other => panic!("expected an Int from `f`, got {other:?}"),
    }
}

/// Lowering is a property of the syntax, so a machine built next over the same
/// program reads what this one lowered. A search builds one per interleaving and
/// `ply test` one per pool thread per group, which is what this is worth.
#[test]
fn a_second_machine_over_one_program_lowers_nothing_the_first_already_did() {
    let (program, resolved) = standalone(vec![
        fn_def("g", &["x"], bin(BinOp::Add, var("x"), int(1))),
        fn_def("f", &[], callv("g", vec![int(6)])),
    ]);

    let mut first = Machine::for_program(&program, &resolved);
    assert_eq!(answer(&mut first), 7);
    let shared = first.share_lowering();
    let lowered = shared.len();
    assert_eq!(lowered, 2, "`f` and `g`, and nothing else, were lowered");

    let mut second = Machine::for_program(&program, &resolved);
    second.set_lowering(std::rc::Rc::clone(&shared));
    assert_eq!(answer(&mut second), 7);
    assert_eq!(
        shared.len(),
        lowered,
        "the second machine lowered {} more bodies the first had already lowered",
        shared.len() - lowered
    );
}

/// A bisection rebuilds a program whose definitions carry the names of the ones
/// they replace, so a cache keyed on a body's address must be refused rather
/// than consulted across two programs. The answer here is the running program's.
#[test]
fn a_machine_refuses_a_lowering_taken_over_a_different_program() {
    let (one, one_resolved) = standalone(vec![fn_def("f", &[], int(1))]);
    let (two, two_resolved) = standalone(vec![fn_def("f", &[], int(2))]);

    let mut first = Machine::for_program(&one, &one_resolved);
    assert_eq!(answer(&mut first), 1);

    let mut second = Machine::for_program(&two, &two_resolved);
    second.set_lowering(first.share_lowering());
    assert!(
        !std::rc::Rc::ptr_eq(&second.share_lowering(), &first.share_lowering()),
        "a machine took a cache built over another program"
    );
    assert_eq!(
        answer(&mut second),
        2,
        "the second program's `f` answered with the first program's body"
    );
}

/// `eval_test` lowered the body it was about to run on every call and cached
/// nothing, so a worker re-running a test paid the traversal again.
#[test]
fn a_test_run_twice_is_lowered_once() {
    let (program, resolved) = standalone(vec![test_def("t", bin(BinOp::Add, int(1), int(2)))]);
    let mut machine = Machine::for_program(&program, &resolved);
    machine.eval_test(0).expect("the test passes");
    machine.eval_test(0).expect("the test passes again");
    assert_eq!(machine.share_lowering().len(), 1);
}
