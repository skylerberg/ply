use crate::unit::build::*;
use ply_core::check_program;
use ply_eval::Value;
use ply_eval::compiled::*;
use ply_eval::evaluator::Machine;
use ply_eval::{Closure, ClosureKind};
use ply_span::Symbol;
use ply_span::{Diagnostic, codes};
use ply_syntax::ast::{BinOp, Expr, Item, Program};
use ply_syntax::resolve::Resolved;
use ply_ty::CheckOutput;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

type Reply = dyn Fn(&Symbol, &[Value], usize) -> Option<Value>;

#[derive(Clone, Debug, PartialEq)]
struct Offer {
    name: Symbol,
    args: Vec<Value>,
    budget: usize,
}

struct Double {
    /// Never dereferenced.
    program: *const Program,
    reply: Box<Reply>,
    offers: RefCell<Vec<Offer>>,
}

impl Double {
    fn over(
        program: &Program,
        reply: impl Fn(&Symbol, &[Value], usize) -> Option<Value> + 'static,
    ) -> Rc<Double> {
        Rc::new(Double {
            program: std::ptr::from_ref(program),
            reply: Box::new(reply),
            offers: RefCell::new(Vec::new()),
        })
    }

    fn declining(program: &Program) -> Rc<Double> {
        Double::over(program, |_, _, _| None)
    }

    fn answering(program: &Program, name: &str, value: Value) -> Rc<Double> {
        let wanted = Symbol::new(name);
        Double::over(program, move |asked, _, _| {
            (*asked == wanted).then(|| value.clone())
        })
    }

    fn offers(&self) -> Vec<Offer> {
        self.offers.borrow().clone()
    }

    fn names(&self) -> Vec<String> {
        self.offers
            .borrow()
            .iter()
            .map(|o| o.name.as_str().to_string())
            .collect()
    }
}

impl Compiled for Double {
    fn describes(&self, program: &Program) -> bool {
        std::ptr::eq(self.program, std::ptr::from_ref(program))
    }

    fn enter(&self, name: &Symbol, args: &[Value], budget: usize) -> Option<Value> {
        self.offers.borrow_mut().push(Offer {
            name: name.clone(),
            args: args.to_vec(),
            budget,
        });
        (self.reply)(name, args, budget)
    }
}

struct Checked {
    program: Program,
    resolved: Resolved,
    check: CheckOutput,
}

fn checked(items: Vec<Item>) -> Checked {
    let (program, resolved) = standalone(items);
    let check = match check_program(&program, &resolved) {
        Ok(check) => check,
        Err(ds) => panic!("the program under test does not check: {ds:#?}"),
    };
    Checked {
        program,
        resolved,
        check,
    }
}

impl Checked {
    fn machine(&self) -> Machine<'_> {
        Machine::new(&self.program, &self.resolved, &self.check)
    }

    fn types(&self) -> CarriedTypes {
        CarriedTypes::over(Some(&self.check))
    }
}

/// From source, because `build::fn_def` cannot write the declared types the argument gate reads.
fn checked_source(source: &str) -> Checked {
    let mut program = ply_syntax::parse_program(vec![(
        ply_span::SourceId(0),
        ply_syntax::ast::ModuleName::anonymous(),
        source,
    )])
    .expect("the fixture must parse");
    let resolved = ply_syntax::resolve::resolve(&mut program).expect("the fixture must resolve");
    let check = match check_program(&program, &resolved) {
        Ok(check) => check,
        Err(ds) => panic!("the program under test does not check: {ds:#?}"),
    };
    Checked {
        program,
        resolved,
        check,
    }
}

/// `Diagnostic` has no `PartialEq`, so a failing outcome cannot be compared with `assert_eq!`
#[track_caller]
fn ok(outcome: Result<Value, Diagnostic>) -> Value {
    match outcome {
        Ok(value) => value,
        Err(d) => panic!("expected a value, got {}: {}", d.code, d.message),
    }
}

fn double_def() -> Item {
    fn_def_sig(
        "double",
        &[("x", tcon("Int"))],
        tcon("Int"),
        bin(BinOp::Mul, var("x"), int(2)),
    )
}

#[test]
fn a_machine_with_no_backend_never_asks_and_never_counts() {
    let c = checked(vec![double_def()]);
    let mut machine = c.machine();
    assert_eq!(
        ok(machine.eval_expr_for_test(&callv("double", vec![int(21)]))),
        Value::Int(42)
    );
    assert_eq!(machine.compiled_counts(), (0, 0));
    assert_eq!(machine.compiled_refusals(), 0);
}

#[test]
fn a_backend_built_over_another_program_is_ignored() {
    let elsewhere = checked(vec![fn_def_sig(
        "double",
        &[("x", tcon("Int"))],
        tcon("Int"),
        int(1000),
    )]);
    let backend = Double::answering(&elsewhere.program, "double", Value::Int(84));

    let c = checked(vec![double_def()]);
    let mut machine = c.machine();
    machine.set_compiled(backend.clone());
    assert_eq!(
        ok(machine.eval_expr_for_test(&callv("double", vec![int(21)]))),
        Value::Int(42)
    );
    assert_eq!(machine.compiled_counts(), (0, 0));
    assert!(backend.offers().is_empty());
}

/// Built by hand, so [`admit`] can be asked about a body the machine would never hand it.
fn code_closure(name: Option<&str>, params: &[&str], body: Expr) -> Closure {
    let params: Vec<Symbol> = params.iter().copied().map(Symbol::new).collect();
    let lowered = ply_eval::code::lower_fn(&params, &body);
    Closure {
        name: name.map(Symbol::new),
        kind: ClosureKind::Code {
            params: Rc::new(params),
            size: lowered.size,
            body: lowered.code,
            captures: ply_eval::code::no_captures(),
            captured: Rc::from(Vec::new()),
            module: 0,
        },
    }
}

fn self_handled() -> Checked {
    checked(vec![
        effect_def("state", &[("get", ply_syntax::ast::Mode::Read, false)]),
        fn_def_sig(
            "touch",
            &[("x", tcon("Int"))],
            tcon("Int"),
            perform("state", "get", None, vec![var("x")]),
        ),
        fn_def_sig(
            "handled",
            &[("x", tcon("Int"))],
            tcon("Int"),
            handle(
                callv("touch", vec![var("x")]),
                vec![clause(
                    "state",
                    "get",
                    None,
                    &["n"],
                    bin(BinOp::Add, var("n"), int(1)),
                )],
            ),
        ),
        fn_def_sig(
            "wrapper",
            &[("x", tcon("Int"))],
            tcon("Int"),
            callv("handled", vec![var("x")]),
        ),
        fn_def_sig(
            "bump",
            &[("x", tcon("Int"))],
            tcon("Int"),
            bin(BinOp::Add, var("x"), int(0)),
        ),
    ])
}

#[test]
fn a_machine_with_no_check_output_offers_nothing() {
    let (program, resolved) = standalone(vec![double_def()]);
    let backend = Double::declining(&program);
    let mut machine = Machine::for_program(&program, &resolved);
    machine.set_compiled(backend.clone());
    assert_eq!(
        ok(machine.eval_expr_for_test(&callv("double", vec![int(21)]))),
        Value::Int(42)
    );
    assert!(backend.offers().is_empty());
    assert_eq!(machine.compiled_counts(), (0, 0));
}

#[test]
fn crossable_admits_every_leaf_kind_that_holds_no_handle_and_nothing_else() {
    assert!(crossable(&Value::Int(0)));
    assert!(crossable(&Value::Bool(false)));
    assert!(crossable(&Value::bytes(b"GET /orders HTTP/1.1")));
    assert!(
        crossable(&Value::bytes(b"")),
        "an empty `Bytes` is a `Bytes`"
    );
    assert!(crossable(&Value::str("s")));
    assert!(
        crossable(&Value::str("")),
        "an empty `String` is a `String`"
    );
    assert!(crossable(&Value::Unit));
    for refused in [
        Value::Float(0.0),
        Value::Decimal(Default::default()),
        Value::List(Default::default()),
        Value::Secret(Arc::new(Value::Int(1))),
        Value::Secret(Arc::new(Value::bytes(b"hunter2"))),
    ] {
        assert!(!crossable(&refused), "{refused:?} crossed the boundary");
    }
}

/// `Cell`, `Task` and `Secret` are `Type::Con`s just as `Option` is, so "any nominal type" would carry them.
#[test]
fn a_world_handle_typed_parameter_is_refused_though_it_is_a_nominal_type() {
    let c = checked_source(
        "fn holds_cell(c: Cell<Int>) -> Int = 1\n\
         fn holds_secret(s: Secret<Int>) -> Int = 1\n\
         fn holds_fn(g: (Int) -> Int) -> Int = g(1)\n\
         fn holds_int(n: Int) -> Int = n\n",
    );
    let types = c.types();
    for name in ["holds_cell", "holds_secret", "holds_fn"] {
        let ty = &c.check.defs[&Symbol::new(name)].scheme.ty;
        let ply_ty::Type::Fn { params, .. } = ty else {
            panic!("{name} publishes no function type");
        };
        assert!(
            !types.carries(&params[0], None),
            "{name}'s declared parameter type {:?} is carried",
            params[0]
        );
    }
    let ply_ty::Type::Fn { params, .. } = &c.check.defs[&Symbol::new("holds_int")].scheme.ty else {
        panic!("holds_int publishes no function type");
    };
    assert!(
        types.carries(&params[0], None),
        "the control failed: `Int` is not carried, so the loop above says nothing"
    );
}

#[test]
fn an_answer_whose_kind_is_not_its_declared_returns_is_refused_unless_it_is_childless() {
    let c = checked_source(
        "type Scan = { at: Int, tok: Bytes }\n\
         fn scan(i: Int) -> Scan = { at: i, tok: b\"x\" }\n",
    );
    let holding = Value::list(vec![Value::Closure(Arc::new(code_closure(
        None,
        &["y"],
        var("y"),
    )))]);
    let types = c.types();
    let scan = Symbol::new("scan");
    assert!(
        !types.answer_crosses(&scan, &holding),
        "a `List` holding a `Closure` came back under a declared `-> Scan`"
    );
    assert!(
        types.answer_crosses(
            &scan,
            &record_value(&[("at", Value::Int(0)), ("tok", Value::bytes(b""))])
        ),
        "the record the declaration denotes was refused, so the widening bought nothing"
    );
    assert!(
        types.answer_crosses(&scan, &Value::Int(0)),
        "the childless clause was lost: `Mutation::WrongType` and `Mutation::Answers` both \
         answer an `Int` for a definition that returns something else, and refusing it here \
         would police a wrong answer with a kind test"
    );
}

#[test]
fn a_closure_bearing_record_return_is_refused_however_ordinary_the_record_looks() {
    let c = checked_source(
        "type Box = { run: (Int) -> Int, tag: Int }\n\
         type Plain = { tag: Int }\n\
         fn make_box(n: Int) -> Box = { run: |y: Int| y, tag: n }\n\
         fn make_plain(n: Int) -> Plain = { tag: n }\n",
    );
    let types = c.types();
    // A record with no closure in it, under a declared type that can hold one.
    let innocent = record_value(&[("tag", Value::Int(1))]);
    assert!(
        !types.answer_crosses(&Symbol::new("make_box"), &innocent),
        "a record came back under a declared return type that can hold a `Closure`"
    );
    assert!(
        types.answer_crosses(&Symbol::new("make_plain"), &innocent),
        "the control failed: a record of `Int` was refused too"
    );
    assert!(
        !types.signature_carried(&Symbol::new("make_box")),
        "a backend's registry would hold a definition the machine will not hear from"
    );
    assert!(types.signature_carried(&Symbol::new("make_plain")));
}

#[test]
fn an_entered_subtree_is_refused_for_an_effect_two_hops_down_that_it_would_hide() {
    let c = self_handled();
    // `wrapper` calls `handled`, which discharges `state.get` under its own handler.
    let mut machine = c.machine();
    assert_eq!(
        ok(machine.eval_expr_for_test(&callv("wrapper", vec![int(1)]))),
        Value::Int(2)
    );
    assert_eq!(
        machine.trace().performs(),
        1,
        "the fixture is wrong: nothing was performed, so hiding the subtree would cost \
         nothing"
    );
    drop(machine);

    // And the machine offers it to nobody, so the subtree is never hidden.
    let backend = Double::declining(&c.program);
    let mut machine = c.machine();
    machine.set_compiled(backend.clone());
    assert_eq!(
        ok(machine.eval_expr_for_test(&callv("wrapper", vec![int(1)]))),
        Value::Int(2)
    );
    assert!(
        !backend.names().iter().any(|n| n == "wrapper"),
        "a definition whose subtree performs was offered: {:?}",
        backend.names()
    );
    assert_eq!(
        machine.trace().performs(),
        1,
        "the atoms the interpreter records were lost"
    );
}

struct Roots {
    /// Never dereferenced.
    program: *const Program,
    entered: Box<dyn Fn() -> Entered>,
}

impl Compiled for Roots {
    fn describes(&self, program: &Program) -> bool {
        std::ptr::eq(self.program, std::ptr::from_ref(program))
    }

    fn enter(&self, _: &Symbol, _: &[Value], _: usize) -> Option<Value> {
        None
    }

    fn enter_test(&self, _: &Symbol, _: usize) -> Entered {
        (self.entered)()
    }
}

fn first_test_under(
    c: &Checked,
    entered: impl Fn() -> Entered + 'static,
) -> (Result<(), Diagnostic>, (u64, u64)) {
    let mut machine = c.machine();
    machine.set_compiled(Rc::new(Roots {
        program: &c.program,
        entered: Box::new(entered),
    }));
    let outcome = machine.eval_test_in(c.program.modules[0].name.as_symbol(), 0);
    (outcome, machine.compiled_counts())
}

fn double_doubles(expected: i64) -> Vec<Item> {
    vec![
        double_def(),
        test_def(
            "double doubles",
            callv(
                "assert_eq",
                vec![callv("double", vec![int(21)]), int(expected)],
            ),
        ),
    ]
}

fn assertion_raised() -> Entered {
    Entered::Raised(Diagnostic::error(codes::RUNTIME_ERROR, "assertion failed"))
}

#[test]
fn a_test_root_the_backend_raised_in_keeps_the_machines_diagnostic_when_it_raises_too() {
    let c = checked(double_doubles(43));
    let (outcome, _) = first_test_under(&c, assertion_raised);
    let d = outcome.expect_err("the assertion fails in the machine");
    assert_ne!(d.code, codes::ENGINE_DIVERGENCE, "{}", d.message);
}

fn record_value(fields: &[(&str, Value)]) -> Value {
    let mut map = BTreeMap::new();
    for (name, value) in fields {
        map.insert(Symbol::new(name), value.clone());
    }
    Value::Record(Arc::new(map.into_iter().collect()))
}
