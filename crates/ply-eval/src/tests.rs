use crate::build::*;
use crate::{Interp, Value};
use ply_span::{Diagnostic, codes};
use ply_syntax::ast::{BinOp, Expr, Item, Mode, UnOp};

fn eval_in(items: Vec<Item>, e: Expr) -> Result<Value, Diagnostic> {
    let (program, resolved) = standalone(items);
    Interp::for_program(&program, &resolved).eval_expr_for_test(&e)
}

fn eval(e: Expr) -> Result<Value, Diagnostic> {
    eval_in(Vec::new(), e)
}

fn eval_depth(items: Vec<Item>, e: Expr, depth: usize) -> Result<Value, Diagnostic> {
    let (program, resolved) = standalone(items);
    Interp::for_program(&program, &resolved)
        .with_max_depth(depth)
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
fn err(e: Expr) -> Diagnostic {
    match eval(e) {
        Err(d) => d,
        Ok(v) => panic!("expected a diagnostic, got {v}"),
    }
}

#[track_caller]
fn err_in(items: Vec<Item>, e: Expr) -> Diagnostic {
    match eval_in(items, e) {
        Err(d) => d,
        Ok(v) => panic!("expected a diagnostic, got {v}"),
    }
}

fn state_effect() -> Item {
    effect_def(
        "state",
        &[("get", Mode::Read, false), ("put", Mode::Write, false)],
    )
}

fn db_effect() -> Item {
    effect_def(
        "db",
        &[("get", Mode::Read, true), ("put", Mode::Write, true)],
    )
}

#[test]
fn arithmetic_respects_operator_semantics() {
    assert_eq!(ok_int(bin(BinOp::Add, int(2), int(3))), 5);
    assert_eq!(ok_int(bin(BinOp::Sub, int(2), int(3))), -1);
    assert_eq!(ok_int(bin(BinOp::Mul, int(4), int(3))), 12);
    assert_eq!(ok_int(bin(BinOp::Div, int(7), int(2))), 3);
    assert_eq!(ok_int(bin(BinOp::Rem, int(7), int(2))), 1);
    assert_eq!(ok_int(bin(BinOp::Div, int(-7), int(2))), -3);
    assert_eq!(ok_int(un(UnOp::Neg, int(5))), -5);
}

#[test]
fn division_by_zero_is_a_diagnostic() {
    let d = err(bin(BinOp::Div, int(1), int(0)));
    assert_eq!(d.code, codes::RUNTIME_ERROR);
    assert!(d.message.contains("division by zero"), "{}", d.message);

    let d = err(bin(BinOp::Rem, int(1), int(0)));
    assert!(d.message.contains("remainder by zero"), "{}", d.message);
}

#[test]
fn integer_overflow_is_a_diagnostic_not_a_wrap() {
    let d = err(bin(BinOp::Add, int(i64::MAX), int(1)));
    assert_eq!(d.code, codes::RUNTIME_ERROR);
    assert!(d.message.contains("overflow"), "{}", d.message);

    let d = err(un(UnOp::Neg, int(i64::MIN)));
    assert!(d.message.contains("overflow"), "{}", d.message);
}

#[test]
fn logical_operators_short_circuit() {
    // The right operand would divide by zero if it were evaluated.
    let boom = bin(BinOp::Eq, bin(BinOp::Div, int(1), int(0)), int(0));
    assert_eq!(
        eval(bin(BinOp::And, boolean(false), boom.clone()))
            .unwrap()
            .render(),
        "false"
    );
    assert_eq!(
        eval(bin(BinOp::Or, boolean(true), boom)).unwrap().render(),
        "true"
    );
}

#[test]
fn equality_is_structural_across_shapes() {
    let a = list(vec![int(1), record(vec![("x", int(2))])]);
    let b = list(vec![int(1), record(vec![("x", int(2))])]);
    let c = list(vec![int(1), record(vec![("x", int(3))])]);
    assert_eq!(eval(bin(BinOp::Eq, a.clone(), b)).unwrap().render(), "true");
    assert_eq!(eval(bin(BinOp::Eq, a, c)).unwrap().render(), "false");
}

#[test]
fn comparing_functions_is_an_error() {
    let d = err(bin(BinOp::Eq, lam(&["x"], var("x")), lam(&["y"], var("y"))));
    assert_eq!(d.code, codes::RUNTIME_ERROR);
    assert!(d.message.contains("compare functions"), "{}", d.message);
}

#[test]
fn string_comparison_orders_lexicographically() {
    assert_eq!(
        eval(bin(BinOp::Lt, string("apple"), string("banana")))
            .unwrap()
            .render(),
        "true"
    );
    assert_eq!(
        eval(bin(BinOp::Ge, string("b"), string("a")))
            .unwrap()
            .render(),
        "true"
    );
    let d = err(bin(BinOp::Lt, int(1), string("a")));
    assert_eq!(d.code, codes::RUNTIME_ERROR);
}

#[test]
fn record_field_access_reports_the_available_fields() {
    let r = record(vec![("a", int(1)), ("b", int(2))]);
    assert_eq!(ok_int(field(r.clone(), "b")), 2);
    let d = err(field(r, "c"));
    assert_eq!(d.code, codes::UNKNOWN_NAME);
    assert!(
        d.notes[0].contains("`a`") && d.notes[0].contains("`b`"),
        "{:?}",
        d.notes
    );
}

#[test]
fn calling_a_non_function_is_reported() {
    let d = err(call(int(3), vec![]));
    assert_eq!(d.code, codes::NOT_A_FUNCTION);
}

#[test]
fn unknown_names_are_reported_with_their_span() {
    let d = err(var_at("nope", at(11, 15)));
    assert_eq!(d.code, codes::UNKNOWN_NAME);
    assert_eq!(d.primary_span().unwrap(), at(11, 15));
}

#[test]
fn a_closure_captures_the_environment_at_creation() {
    let e = block(
        vec![
            letv("x", int(1)),
            letv("f", lam(&["y"], bin(BinOp::Add, var("x"), var("y")))),
            letv("x", int(100)),
        ],
        Some(callv("f", vec![int(1)])),
    );
    assert_eq!(ok_int(e), 2);
}

#[test]
fn closures_returned_from_functions_keep_their_captures() {
    let items = vec![fn_def(
        "adder",
        &["n"],
        lam(&["m"], bin(BinOp::Add, var("n"), var("m"))),
    )];
    let e = block(
        vec![
            letv("add3", callv("adder", vec![int(3)])),
            letv("add10", callv("adder", vec![int(10)])),
        ],
        Some(bin(
            BinOp::Add,
            callv("add3", vec![int(1)]),
            callv("add10", vec![int(1)]),
        )),
    );
    match eval_in(items, e) {
        Ok(Value::Int(i)) => assert_eq!(i, 15),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_lambda_parameter_shadows_an_outer_binding() {
    let e = block(
        vec![letv("x", int(1))],
        Some(call(lam(&["x"], var("x")), vec![int(9)])),
    );
    assert_eq!(ok_int(e), 9);
}

#[test]
fn a_local_binding_shadows_a_top_level_definition() {
    let items = vec![fn_def("f", &[], int(1))];
    let e = block(vec![letv("f", lam(&[], int(2)))], Some(callv("f", vec![])));
    assert!(matches!(eval_in(items, e), Ok(Value::Int(2))));
}

#[test]
fn wrong_argument_count_is_an_arity_mismatch() {
    let items = vec![fn_def("f", &["a", "b"], var("a"))];
    let d = err_in(items, callv("f", vec![int(1)]));
    assert_eq!(d.code, codes::ARITY_MISMATCH);
    assert!(d.message.contains("2 arguments"), "{}", d.message);
}

#[test]
fn self_recursion_terminates_at_its_base_case() {
    let items = vec![fn_def(
        "fact",
        &["n"],
        if_(
            bin(BinOp::Le, var("n"), int(1)),
            int(1),
            bin(
                BinOp::Mul,
                var("n"),
                callv("fact", vec![bin(BinOp::Sub, var("n"), int(1))]),
            ),
        ),
    )];
    match eval_in(items, callv("fact", vec![int(10)])) {
        Ok(Value::Int(i)) => assert_eq!(i, 3_628_800),
        other => panic!("{other:?}"),
    }
}

#[test]
fn mutual_recursion_resolves_through_the_global_scope() {
    let items = vec![
        fn_def(
            "is_even",
            &["n"],
            if_(
                bin(BinOp::Eq, var("n"), int(0)),
                boolean(true),
                callv("is_odd", vec![bin(BinOp::Sub, var("n"), int(1))]),
            ),
        ),
        fn_def(
            "is_odd",
            &["n"],
            if_(
                bin(BinOp::Eq, var("n"), int(0)),
                boolean(false),
                callv("is_even", vec![bin(BinOp::Sub, var("n"), int(1))]),
            ),
        ),
    ];
    assert_eq!(
        eval_in(items, callv("is_even", vec![int(10)]))
            .unwrap()
            .render(),
        "true"
    );
}

#[test]
fn unbounded_recursion_becomes_a_diagnostic_not_a_stack_overflow() {
    let items = vec![fn_def(
        "spin",
        &["n"],
        callv("spin", vec![bin(BinOp::Add, var("n"), int(1))]),
    )];
    let d = match eval_depth(items, callv("spin", vec![int(0)]), 64) {
        Err(d) => d,
        Ok(v) => panic!("expected a recursion diagnostic, got {v}"),
    };
    assert_eq!(d.code, codes::RUNTIME_ERROR);
    assert!(d.message.contains("recursion limit"), "{}", d.message);
}

/// The depth limit is only a real bound if the native stack underneath it
/// never runs out first, on a stack far smaller than a worker's default.
#[test]
fn recursion_to_the_depth_limit_survives_a_one_mebibyte_thread_stack() {
    let handle = std::thread::Builder::new()
        .stack_size(1024 * 1024)
        .spawn(|| {
            let items = vec![fn_def(
                "down",
                &["n"],
                if_(
                    bin(BinOp::Le, var("n"), int(0)),
                    int(0),
                    bin(
                        BinOp::Add,
                        int(0),
                        callv("down", vec![bin(BinOp::Sub, var("n"), int(1))]),
                    ),
                ),
            )];
            let depth = crate::DEFAULT_MAX_DEPTH as i64;
            matches!(
                eval_in(items, callv("down", vec![int(depth - 1)])),
                Ok(Value::Int(0))
            )
        })
        .expect("failed to spawn");
    assert!(
        handle
            .join()
            .expect("the interpreter overflowed the thread stack")
    );
}

#[test]
fn the_depth_counter_unwinds_so_sequential_calls_are_not_penalized() {
    let items = vec![fn_def(
        "down",
        &["n"],
        if_(
            bin(BinOp::Le, var("n"), int(0)),
            int(0),
            callv("down", vec![bin(BinOp::Sub, var("n"), int(1))]),
        ),
    )];
    // Twenty separate 10-deep call chains under a limit of 12: only a leaking
    // counter would trip.
    let mut body = int(0);
    for _ in 0..20 {
        body = bin(BinOp::Add, body, callv("down", vec![int(10)]));
    }
    assert!(matches!(eval_depth(items, body, 12), Ok(Value::Int(0))));
}

#[test]
fn literal_patterns_match_by_value() {
    let e = match_(
        int(2),
        vec![
            arm(pint(1), string("one")),
            arm(pint(2), string("two")),
            arm(pwild(), string("many")),
        ],
    );
    assert_eq!(eval(e).unwrap().render(), "\"two\"");
}

#[test]
fn string_and_bool_literal_patterns_match() {
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
}

#[test]
fn a_unit_pattern_matches_only_unit() {
    let e = match_(unit(), vec![arm(pint(0), int(1)), arm(punit(), int(2))]);
    assert_eq!(ok_int(e), 2);
    let e = match_(int(0), vec![arm(punit(), int(1)), arm(pwild(), int(2))]);
    assert_eq!(ok_int(e), 2);
}

#[test]
fn nested_constructor_patterns_bind_inner_values() {
    let items = vec![type_def("Tree", &[("Leaf", 1), ("Node", 2)])];
    let tree = callv(
        "Node",
        vec![
            callv("Leaf", vec![int(1)]),
            callv(
                "Node",
                vec![callv("Leaf", vec![int(2)]), callv("Leaf", vec![int(3)])],
            ),
        ],
    );
    let e = match_(
        tree,
        vec![
            arm(
                pctor(
                    "Node",
                    vec![
                        pwild(),
                        pctor("Node", vec![pctor("Leaf", vec![pvar("x")]), pwild()]),
                    ],
                ),
                var("x"),
            ),
            arm(pwild(), int(-1)),
        ],
    );
    match eval_in(items, e) {
        Ok(Value::Int(i)) => assert_eq!(i, 2),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_nullary_constructor_pattern_tests_rather_than_binds() {
    let items = vec![type_def("Option", &[("Some", 1), ("None", 0)])];
    // The parser cannot distinguish `None` the binder from `None` the variant,
    // so a bare name that is a known nullary constructor must not match `Some`.
    let e = match_(
        callv("Some", vec![int(7)]),
        vec![
            arm(pvar("None"), int(0)),
            arm(pctor("Some", vec![pvar("v")]), var("v")),
        ],
    );
    match eval_in(items, e) {
        Ok(Value::Int(i)) => assert_eq!(i, 7),
        other => panic!("{other:?}"),
    }
}

#[test]
fn list_patterns_split_head_and_tail() {
    let e = match_(
        list(vec![int(1), int(2), int(3)]),
        vec![
            arm(plist(vec![], None), int(-1)),
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
        list(vec![int(1), int(2), int(3)]),
        vec![
            arm(plist(vec![pvar("a"), pvar("b")], None), int(2)),
            arm(pwild(), int(0)),
        ],
    );
    assert_eq!(ok_int(e), 0);
}

#[test]
fn record_patterns_honour_the_rest_flag() {
    let r = record(vec![("a", int(1)), ("b", int(2))]);
    let closed = match_(
        r.clone(),
        vec![
            arm(prec(vec![("a", pvar("x"))], false), var("x")),
            arm(pwild(), int(-1)),
        ],
    );
    assert_eq!(ok_int(closed), -1);

    let open = match_(
        r,
        vec![
            arm(prec(vec![("a", pvar("x"))], true), var("x")),
            arm(pwild(), int(-1)),
        ],
    );
    assert_eq!(ok_int(open), 1);
}

#[test]
fn guards_can_reject_an_otherwise_matching_arm() {
    let e = match_(
        int(4),
        vec![
            guarded(pvar("n"), bin(BinOp::Gt, var("n"), int(10)), string("big")),
            guarded(
                pvar("n"),
                bin(BinOp::Gt, var("n"), int(3)),
                string("medium"),
            ),
            arm(pwild(), string("small")),
        ],
    );
    assert_eq!(eval(e).unwrap().render(), "\"medium\"");
}

#[test]
fn bindings_from_a_rejected_arm_do_not_leak() {
    let e = block(
        vec![letv("x", int(1))],
        Some(match_(
            int(5),
            vec![
                guarded(pvar("x"), boolean(false), var("x")),
                arm(pwild(), var("x")),
            ],
        )),
    );
    assert_eq!(ok_int(e), 1);
}

#[test]
fn an_unmatched_scrutinee_is_a_non_exhaustive_match() {
    let e = match_(int(3), vec![arm(pint(1), int(1)), arm(pint(2), int(2))]);
    let d = err(e);
    assert_eq!(d.code, codes::NON_EXHAUSTIVE_MATCH);
    assert!(d.labels[0].message.contains('3'), "{:?}", d.labels);
}

#[test]
fn a_refutable_let_that_fails_is_a_diagnostic() {
    let e = block(
        vec![let_(pctor("Some", vec![pvar("x")]), int(1))],
        Some(var("x")),
    );
    let d = err(e);
    assert_eq!(d.code, codes::NON_EXHAUSTIVE_MATCH);
    assert!(d.message.contains("`let` pattern"), "{}", d.message);
}

#[test]
fn a_handler_clause_answers_the_perform_site_directly() {
    let items = vec![state_effect()];
    let e = handle(
        bin(BinOp::Add, perform("state", "get", None, vec![]), int(1)),
        vec![clause("state", "get", None, &[], int(41))],
    );
    match eval_in(items, e) {
        Ok(Value::Int(i)) => assert_eq!(i, 42),
        other => panic!("{other:?}"),
    }
}

#[test]
fn operation_arguments_are_bound_to_the_clause_parameters() {
    let items = vec![state_effect()];
    let e = handle(
        perform("state", "put", None, vec![int(20)]),
        vec![clause(
            "state",
            "put",
            None,
            &["v"],
            bin(BinOp::Mul, var("v"), int(2)),
        )],
    );
    match eval_in(items, e) {
        Ok(Value::Int(i)) => assert_eq!(i, 40),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_clause_body_sees_the_handlers_scope_not_the_perform_sites() {
    let items = vec![state_effect()];
    let e = block(
        vec![letv("x", int(1))],
        Some(handle(
            block(
                vec![letv("x", int(99))],
                Some(perform("state", "get", None, vec![])),
            ),
            vec![clause("state", "get", None, &[], var("x"))],
        )),
    );
    match eval_in(items, e) {
        Ok(Value::Int(i)) => assert_eq!(i, 1),
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_innermost_matching_handler_wins() {
    let items = vec![state_effect()];
    let e = handle(
        handle(
            perform("state", "get", None, vec![]),
            vec![clause("state", "get", None, &[], int(2))],
        ),
        vec![clause("state", "get", None, &[], int(1))],
    );
    match eval_in(items, e) {
        Ok(Value::Int(i)) => assert_eq!(i, 2),
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_operation_the_inner_handler_does_not_name_falls_through() {
    let items = vec![state_effect()];
    let e = handle(
        handle(
            bin(
                BinOp::Add,
                perform("state", "get", None, vec![]),
                perform("state", "put", None, vec![int(0)]),
            ),
            vec![clause("state", "get", None, &[], int(2))],
        ),
        vec![clause("state", "put", None, &["v"], int(30))],
    );
    match eval_in(items, e) {
        Ok(Value::Int(i)) => assert_eq!(i, 32),
        other => panic!("{other:?}"),
    }
}

#[test]
fn clauses_discriminate_on_the_resource_label() {
    let items = vec![db_effect()];
    let e = handle(
        bin(
            BinOp::Add,
            perform("db", "get", Some("users"), vec![int(0)]),
            perform("db", "get", Some("orders"), vec![int(0)]),
        ),
        vec![
            clause("db", "get", Some("users"), &["k"], int(1)),
            clause("db", "get", Some("orders"), &["k"], int(10)),
        ],
    );
    match eval_in(items, e) {
        Ok(Value::Int(i)) => assert_eq!(i, 11),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_resource_the_handler_does_not_name_falls_through_to_the_outer_handler() {
    let items = vec![db_effect()];
    let e = handle(
        handle(
            perform("db", "get", Some("orders"), vec![int(0)]),
            vec![clause("db", "get", Some("users"), &["k"], int(1))],
        ),
        vec![clause("db", "get", Some("orders"), &["k"], int(7))],
    );
    match eval_in(items, e) {
        Ok(Value::Int(i)) => assert_eq!(i, 7),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_handler_that_performs_the_operation_it_handles_reaches_the_next_handler_out() {
    let items = vec![state_effect()];
    let e = handle(
        handle(
            perform("state", "get", None, vec![]),
            vec![clause(
                "state",
                "get",
                None,
                &[],
                bin(BinOp::Add, perform("state", "get", None, vec![]), int(1)),
            )],
        ),
        vec![clause("state", "get", None, &[], int(10))],
    );
    match eval_depth(items, e, 32) {
        Ok(Value::Int(i)) => assert_eq!(i, 11),
        other => panic!("expected 11 (the clause must not catch itself), got {other:?}"),
    }
}

#[test]
fn a_self_performing_handler_with_nothing_outside_it_is_unhandled_not_looping() {
    let items = vec![state_effect()];
    let e = handle(
        perform("state", "get", None, vec![]),
        vec![clause(
            "state",
            "get",
            None,
            &[],
            perform("state", "get", None, vec![]),
        )],
    );
    let d = match eval_depth(items, e, 32) {
        Err(d) => d,
        Ok(v) => panic!("expected UNHANDLED_EFFECT, got {v}"),
    };
    assert_eq!(d.code, codes::UNHANDLED_EFFECT);
}

#[test]
fn the_handler_stack_is_restored_after_a_clause_returns() {
    let items = vec![state_effect()];
    // The clause escapes to the outer handler; the second perform must still
    // find the inner one.
    let e = handle(
        handle(
            bin(
                BinOp::Add,
                perform("state", "get", None, vec![]),
                perform("state", "get", None, vec![]),
            ),
            vec![clause(
                "state",
                "get",
                None,
                &[],
                bin(BinOp::Mul, perform("state", "get", None, vec![]), int(2)),
            )],
        ),
        vec![clause("state", "get", None, &[], int(5))],
    );
    match eval_depth(items, e, 32) {
        Ok(Value::Int(i)) => assert_eq!(i, 20),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_clause_written_without_a_resource_handles_every_resource() {
    let items = vec![db_effect()];
    let e = handle(
        bin(
            BinOp::Add,
            perform("db", "get", Some("users"), vec![int(0)]),
            perform("db", "get", Some("orders"), vec![int(0)]),
        ),
        vec![clause("db", "get", None, &["k"], int(3))],
    );
    match eval_in(items, e) {
        Ok(Value::Int(i)) => assert_eq!(i, 6),
        other => panic!("{other:?}"),
    }
}

#[test]
fn cells_compare_by_identity() {
    let e = with_cell(
        "s",
        int(1),
        "c",
        with_cell(
            "s",
            int(1),
            "d",
            list(vec![
                bin(BinOp::Eq, var("c"), var("c")),
                bin(BinOp::Eq, var("c"), var("d")),
            ]),
        ),
    );
    assert_eq!(eval(e).unwrap().render(), "[true, false]");
}

#[test]
fn a_return_clause_transforms_the_bodys_value() {
    let items = vec![state_effect()];
    let e = handle_ret(
        perform("state", "get", None, vec![]),
        vec![clause("state", "get", None, &[], int(4))],
        "x",
        bin(BinOp::Mul, var("x"), int(10)),
    );
    match eval_in(items, e) {
        Ok(Value::Int(i)) => assert_eq!(i, 40),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_return_clause_runs_outside_its_own_handler() {
    let items = vec![state_effect()];
    let e = handle(
        handle_ret(
            int(0),
            vec![clause("state", "get", None, &[], int(1))],
            "x",
            perform("state", "get", None, vec![]),
        ),
        vec![clause("state", "get", None, &[], int(99))],
    );
    match eval_depth(items, e, 32) {
        Ok(Value::Int(i)) => assert_eq!(i, 99),
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_unhandled_operation_names_the_atom_it_could_not_discharge() {
    let items = vec![db_effect()];
    let e = spanned(perform("db", "get", Some("users"), vec![int(1)]), at(4, 20));
    let d = err_in(items, e);
    assert_eq!(d.code, codes::UNHANDLED_EFFECT);
    assert!(d.message.contains("db.get[users]"), "{}", d.message);
    assert_eq!(d.primary_span().unwrap(), at(4, 20));
}

#[test]
fn a_clause_with_the_wrong_parameter_count_is_an_arity_mismatch() {
    let items = vec![state_effect()];
    let e = handle(
        perform("state", "put", None, vec![int(1)]),
        vec![clause("state", "put", None, &["a", "b"], int(0))],
    );
    let d = err_in(items, e);
    assert_eq!(d.code, codes::ARITY_MISMATCH);
}

#[test]
fn performing_an_operation_the_effect_does_not_declare_is_reported() {
    let items = vec![state_effect()];
    let d = err_in(items, perform("state", "nope", None, vec![]));
    assert_eq!(d.code, codes::UNKNOWN_OPERATION);
}

#[test]
fn a_resource_parameterized_operation_requires_a_label() {
    let items = vec![db_effect()];
    let d = err_in(items, perform("db", "get", None, vec![int(1)]));
    assert_eq!(d.code, codes::RESOURCE_REQUIRED);
}

#[test]
fn handlers_installed_inside_a_clause_body_are_popped_again() {
    let items = vec![state_effect()];
    let inner_handle = handle(
        perform("state", "put", None, vec![int(1)]),
        vec![clause("state", "put", None, &["v"], int(3))],
    );
    let e = handle(
        bin(
            BinOp::Add,
            perform("state", "get", None, vec![]),
            perform("state", "put", None, vec![int(0)]),
        ),
        vec![
            clause("state", "get", None, &[], inner_handle),
            clause("state", "put", None, &["v"], int(4)),
        ],
    );
    match eval_depth(items, e, 32) {
        Ok(Value::Int(i)) => assert_eq!(i, 7),
        other => panic!("{other:?}"),
    }
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
fn a_handler_backed_by_a_cell_implements_state() {
    let items = vec![state_effect()];
    let body = block(
        vec![
            discard(perform("state", "put", None, vec![int(5)])),
            discard(perform(
                "state",
                "put",
                None,
                vec![bin(
                    BinOp::Add,
                    perform("state", "get", None, vec![]),
                    int(2),
                )],
            )),
        ],
        Some(perform("state", "get", None, vec![])),
    );
    let e = with_cell(
        "s",
        int(0),
        "c",
        handle(
            body,
            vec![
                clause("state", "get", None, &[], callv("cell_get", vec![var("c")])),
                clause(
                    "state",
                    "put",
                    None,
                    &["v"],
                    callv("cell_set", vec![var("c"), var("v")]),
                ),
            ],
        ),
    );
    match eval_in(items, e) {
        Ok(Value::Int(i)) => assert_eq!(i, 7),
        other => panic!("{other:?}"),
    }
}

#[test]
fn cell_writes_from_a_clause_survive_across_performs() {
    let items = vec![state_effect()];
    let bump = clause(
        "state",
        "put",
        None,
        &["v"],
        callv(
            "cell_set",
            vec![
                var("c"),
                bin(BinOp::Add, callv("cell_get", vec![var("c")]), var("v")),
            ],
        ),
    );
    let body = block(
        vec![
            discard(perform("state", "put", None, vec![int(1)])),
            discard(perform("state", "put", None, vec![int(2)])),
            discard(perform("state", "put", None, vec![int(3)])),
        ],
        Some(perform("state", "get", None, vec![])),
    );
    let e = with_cell(
        "s",
        int(0),
        "c",
        handle(
            body,
            vec![
                bump,
                clause("state", "get", None, &[], callv("cell_get", vec![var("c")])),
            ],
        ),
    );
    match eval_in(items, e) {
        Ok(Value::Int(i)) => assert_eq!(i, 6),
        other => panic!("{other:?}"),
    }
}

#[test]
fn nested_regions_get_independent_cells() {
    let inner = with_cell(
        "s",
        int(10),
        "d",
        block(
            vec![discard(callv("cell_set", vec![var("d"), int(20)]))],
            Some(callv("cell_get", vec![var("d")])),
        ),
    );
    let e = with_cell(
        "s",
        int(1),
        "c",
        block(
            vec![letv("inner", inner)],
            Some(bin(
                BinOp::Add,
                var("inner"),
                callv("cell_get", vec![var("c")]),
            )),
        ),
    );
    assert_eq!(ok_int(e), 21);
}

#[test]
fn len_counts_list_elements_and_string_chars() {
    assert_eq!(ok_int(callv("len", vec![list(vec![int(1), int(2)])])), 2);
    assert_eq!(ok_int(callv("len", vec![string("héllo")])), 5);
    let d = err(callv("len", vec![int(3)]));
    assert_eq!(d.code, codes::RUNTIME_ERROR);
    assert!(d.message.contains("List or String"), "{}", d.message);
}

#[test]
fn push_appends_without_mutating_the_original() {
    let e = block(
        vec![
            letv("xs", list(vec![int(1)])),
            letv("ys", callv("push", vec![var("xs"), int(2)])),
        ],
        Some(bin(
            BinOp::Add,
            callv("len", vec![var("xs")]),
            callv("len", vec![var("ys")]),
        )),
    );
    assert_eq!(ok_int(e), 3);
    assert_eq!(
        err(callv("push", vec![int(1), int(2)])).code,
        codes::RUNTIME_ERROR
    );
}

#[test]
fn map_filter_and_fold_drive_user_closures() {
    let xs = list(vec![int(1), int(2), int(3), int(4)]);
    let doubled = callv(
        "map",
        vec![xs.clone(), lam(&["x"], bin(BinOp::Mul, var("x"), int(2)))],
    );
    assert_eq!(eval(doubled).unwrap().render(), "[2, 4, 6, 8]");

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
    assert_eq!(eval(evens).unwrap().render(), "[2, 4]");

    let sum = callv(
        "fold",
        vec![
            xs,
            int(0),
            lam(&["acc", "x"], bin(BinOp::Add, var("acc"), var("x"))),
        ],
    );
    assert_eq!(ok_int(sum), 10);
}

#[test]
fn a_closure_passed_to_map_may_perform_an_effect() {
    let items = vec![state_effect()];
    let e = handle(
        callv(
            "map",
            vec![
                list(vec![int(1), int(2)]),
                lam(
                    &["x"],
                    bin(BinOp::Add, var("x"), perform("state", "get", None, vec![])),
                ),
            ],
        ),
        vec![clause("state", "get", None, &[], int(10))],
    );
    assert_eq!(eval_in(items, e).unwrap().render(), "[11, 12]");
}

#[test]
fn a_higher_order_builtin_reports_a_non_function_argument() {
    let d = err(callv("map", vec![list(vec![int(1)]), int(3)]));
    assert_eq!(d.code, codes::NOT_A_FUNCTION);
    assert_eq!(
        err(callv("map", vec![int(1), lam(&["x"], var("x"))])).code,
        codes::RUNTIME_ERROR
    );
}

#[test]
fn filter_requires_a_boolean_predicate() {
    let d = err(callv(
        "filter",
        vec![list(vec![int(1)]), lam(&["x"], var("x"))],
    ));
    assert_eq!(d.code, codes::RUNTIME_ERROR);
    assert!(d.message.contains("Bool"), "{}", d.message);
}

#[test]
fn range_builds_half_open_intervals_and_refuses_runaways() {
    assert_eq!(
        eval(callv("range", vec![int(3)])).unwrap().render(),
        "[0, 1, 2]"
    );
    assert_eq!(
        eval(callv("range", vec![int(2), int(5)])).unwrap().render(),
        "[2, 3, 4]"
    );
    assert_eq!(
        eval(callv("range", vec![int(5), int(2)])).unwrap().render(),
        "[]"
    );
    assert_eq!(eval(callv("range", vec![int(0)])).unwrap().render(), "[]");
    let d = err(callv("range", vec![int(0), int(i64::MAX)]));
    assert_eq!(d.code, codes::RUNTIME_ERROR);
    assert!(d.message.contains("exceeds the limit"), "{}", d.message);
}

#[test]
fn string_builtins_reject_non_strings() {
    assert_eq!(
        eval(callv("int_to_string", vec![int(-12)]))
            .unwrap()
            .render(),
        "\"-12\""
    );
    assert_eq!(
        eval(callv("string_concat", vec![string("a"), string("b")]))
            .unwrap()
            .render(),
        "\"ab\""
    );
    assert_eq!(
        err(callv("int_to_string", vec![string("x")])).code,
        codes::RUNTIME_ERROR
    );
    assert_eq!(
        err(callv("string_concat", vec![string("a"), int(1)])).code,
        codes::RUNTIME_ERROR
    );
    assert_eq!(
        err(bin(BinOp::Concat, int(1), string("a"))).code,
        codes::RUNTIME_ERROR
    );
}

#[test]
fn cell_builtins_reject_non_cells() {
    assert_eq!(
        err(callv("cell_get", vec![int(1)])).code,
        codes::RUNTIME_ERROR
    );
    assert_eq!(
        err(callv("cell_set", vec![int(1), int(2)])).code,
        codes::RUNTIME_ERROR
    );
}

#[test]
fn panic_carries_its_message() {
    let d = err(callv("panic", vec![string("boom")]));
    assert_eq!(d.code, codes::RUNTIME_ERROR);
    assert!(d.message.contains("panic: boom"), "{}", d.message);
}

#[test]
fn every_builtin_checks_its_argument_count() {
    for b in crate::Builtin::all() {
        let (min, max) = b.arity();
        let too_few: Vec<Expr> = (0..min.saturating_sub(1)).map(|_| int(0)).collect();
        let d = err(callv(b.name(), too_few));
        assert_eq!(
            d.code,
            codes::ARITY_MISMATCH,
            "{} accepted too few arguments",
            b.name()
        );

        let too_many: Vec<Expr> = (0..max + 1).map(|_| int(0)).collect();
        let d = err(callv(b.name(), too_many));
        assert_eq!(
            d.code,
            codes::ARITY_MISMATCH,
            "{} accepted too many arguments",
            b.name()
        );
    }
}

#[test]
fn a_user_definition_shadows_a_builtin_of_the_same_name() {
    let items = vec![fn_def("len", &["x"], int(-1))];
    match eval_in(items, callv("len", vec![list(vec![int(1), int(2)])])) {
        Ok(Value::Int(i)) => assert_eq!(i, -1),
        other => panic!("{other:?}"),
    }
}

#[test]
fn assert_passes_on_true_and_reports_its_message_on_false() {
    assert!(eval(callv("assert", vec![boolean(true)])).is_ok());
    let d = err(callv(
        "assert",
        vec![boolean(false), string("balance must be positive")],
    ));
    assert_eq!(d.code, codes::ASSERTION_FAILED);
    assert!(
        d.notes
            .iter()
            .any(|n| n.contains("balance must be positive")),
        "{:?}",
        d.notes
    );
    assert_eq!(
        err(callv("assert", vec![int(1)])).code,
        codes::RUNTIME_ERROR
    );
}

#[test]
fn assert_eq_renders_expected_and_actual_at_the_call_site() {
    let e = spanned(callv("assert_eq", vec![int(-5), int(0)]), at(88, 100));
    let d = err(e);
    assert_eq!(d.code, codes::ASSERTION_FAILED);
    assert_eq!(d.message, "assertion failed: expected 0, found -5");
    assert!(
        d.notes.contains(&"expected: 0".to_string()),
        "{:?}",
        d.notes
    );
    assert!(
        d.notes.contains(&"actual:   -5".to_string()),
        "{:?}",
        d.notes
    );
    assert_eq!(d.primary_span().unwrap(), at(88, 100));
}

#[test]
fn assert_eq_points_at_the_first_structural_difference() {
    let actual = list(vec![
        record(vec![("id", int(1)), ("name", string("a"))]),
        record(vec![("id", int(2)), ("name", string("b"))]),
    ]);
    let expected = list(vec![
        record(vec![("id", int(1)), ("name", string("a"))]),
        record(vec![("id", int(2)), ("name", string("z"))]),
    ]);
    let d = err(callv("assert_eq", vec![actual, expected]));
    let note = d
        .notes
        .iter()
        .find(|n| n.starts_with("first difference"))
        .expect("no diff note");
    assert!(note.contains("[1].name"), "{note}");
    assert!(note.contains("expected \"z\""), "{note}");
    assert!(note.contains("found \"b\""), "{note}");
}

#[test]
fn assert_eq_on_differing_lengths_reports_the_whole_values() {
    let d = err(callv(
        "assert_eq",
        vec![list(vec![int(1)]), list(vec![int(1), int(2)])],
    ));
    assert_eq!(d.message, "assertion failed: expected [1, 2], found [1]");
    assert!(
        !d.notes.iter().any(|n| n.starts_with("first difference")),
        "{:?}",
        d.notes
    );
}

#[test]
fn assert_eq_distinguishes_constructors_with_equal_payloads() {
    let items = vec![type_def("Shape", &[("Circle", 1), ("Square", 1)])];
    let e = callv(
        "assert_eq",
        vec![callv("Circle", vec![int(2)]), callv("Square", vec![int(2)])],
    );
    let d = err_in(items, e);
    assert_eq!(
        d.message,
        "assertion failed: expected Square(2), found Circle(2)"
    );
}

#[test]
fn eval_test_runs_the_indexed_test_and_reports_a_failure() {
    let m = module(vec![
        fn_def("two", &[], int(2)),
        test_def(
            "passes",
            callv("assert_eq", vec![callv("two", vec![]), int(2)]),
        ),
        test_def(
            "fails",
            callv("assert_eq", vec![callv("two", vec![]), int(3)]),
        ),
    ]);
    let (program, resolved) = standalone_module(m);
    let mut interp = Interp::for_program(&program, &resolved);
    assert_eq!(interp.test_count(), 2);
    assert_eq!(interp.test_name(1), Some("fails"));
    assert!(interp.eval_test(0).is_ok());
    let d = interp.eval_test(1).unwrap_err();
    assert_eq!(d.code, codes::ASSERTION_FAILED);
    assert!(interp.eval_test(2).is_err());
}

#[test]
fn a_failed_test_does_not_poison_the_next_one() {
    let items = vec![state_effect()];
    let mut all = items;
    all.push(test_def(
        "leaves a handler installed",
        handle(
            callv("panic", vec![string("stop")]),
            vec![clause("state", "get", None, &[], int(1))],
        ),
    ));
    all.push(test_def(
        "must be unhandled",
        perform("state", "get", None, vec![]),
    ));
    let (program, resolved) = standalone(all);
    let mut interp = Interp::for_program(&program, &resolved);
    assert_eq!(interp.eval_test(0).unwrap_err().code, codes::RUNTIME_ERROR);
    assert_eq!(
        interp.eval_test(1).unwrap_err().code,
        codes::UNHANDLED_EFFECT
    );
}

/// There is no longer one `main` per program, so choosing an entry point is
/// the caller's job; the evaluator only answers to a program-wide name.
#[test]
fn call_invokes_a_definition_by_its_program_wide_name() {
    let (program, resolved) =
        standalone(vec![fn_def("main", &[], bin(BinOp::Add, int(1), int(2)))]);
    let mut interp = Interp::for_program(&program, &resolved);
    assert_eq!(interp.call("main", Vec::new(), sp()).unwrap().render(), "3");

    let (empty, resolved) = standalone(Vec::new());
    let mut interp = Interp::for_program(&empty, &resolved);
    let d = interp.call("main", Vec::new(), sp()).unwrap_err();
    assert_eq!(d.code, codes::UNKNOWN_NAME);
}

#[test]
fn values_render_readably_for_a_report_reader() {
    assert_eq!(eval(unit()).unwrap().render(), "()");
    assert_eq!(Value::str("a\"b\nc").render(), "\"a\\\"b\\nc\"");
    assert_eq!(
        eval(record(vec![("b", int(2)), ("a", int(1))]))
            .unwrap()
            .render(),
        "{a: 1, b: 2}"
    );
    let long = eval(callv("range", vec![int(40)])).unwrap().render();
    assert!(long.ends_with("… 8 more]"), "{long}");
}
