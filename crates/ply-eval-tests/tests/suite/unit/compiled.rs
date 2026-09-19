use crate::fixture::port_front;
use ply_eval::Value;
use ply_eval::compiled::*;
use ply_eval::evaluator::Machine;
use ply_span::Symbol;
use ply_span::{Diagnostic, codes};
use ply_ty::{DefHash, Front};
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

struct Checked {
    front: Front,
}

impl Checked {
    fn machine(&self) -> Machine<'_> {
        Machine::new(&self.front)
    }

    fn types(&self) -> CarriedTypes {
        CarriedTypes::over(Some(&self.front.check))
    }
}

fn checked_source(source: &str) -> Checked {
    Checked {
        front: port_front(&[("", source)]),
    }
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
        let ty = &c.front.check.defs[&Symbol::new(name)].scheme.ty;
        let ply_ty::Type::Fn { params, .. } = ty else {
            panic!("{name} publishes no function type");
        };
        assert!(
            !types.carries(&params[0], None),
            "{name}'s declared parameter type {:?} is carried",
            params[0]
        );
    }
    let ply_ty::Type::Fn { params, .. } = &c.front.check.defs[&Symbol::new("holds_int")].scheme.ty
    else {
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
    let holding = Value::list(vec![Value::builtin(ply_eval::Builtin::IntToString)]);
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

struct Roots {
    program: DefHash,
    entered: Box<dyn Fn() -> Entered>,
}

impl Compiled for Roots {
    fn describes(&self, program: DefHash) -> bool {
        self.program == program
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
        program: c.front.hashes.digest(),
        entered: Box::new(entered),
    }));
    let outcome = machine.eval_test(0);
    (outcome, machine.compiled_counts())
}

/// A test whose assertion fails: `double(21)` is 42.
const DOUBLE_DOUBLES: &str =
    "fn double(x: Int) -> Int = x * 2\n\ntest \"double doubles\" { assert_eq(double(21), 43) }\n";

fn assertion_raised() -> Entered {
    Entered::Raised(Diagnostic::error(codes::RUNTIME_ERROR, "assertion failed"))
}

#[test]
fn a_test_root_the_backend_raised_in_keeps_the_machines_diagnostic_when_it_raises_too() {
    let c = checked_source(DOUBLE_DOUBLES);
    let (outcome, _) = first_test_under(&c, assertion_raised);
    let d = outcome.expect_err("the assertion fails in the machine");
    assert_eq!(d.code, codes::RUNTIME_ERROR);
    assert_eq!(d.message, "assertion failed");
}

fn record_value(fields: &[(&str, Value)]) -> Value {
    let mut map = BTreeMap::new();
    for (name, value) in fields {
        map.insert(Symbol::new(name), value.clone());
    }
    Value::Record(Arc::new(map.into_iter().collect()))
}
