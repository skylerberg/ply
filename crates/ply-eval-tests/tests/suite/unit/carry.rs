use ply_eval::{Carry, Closure, ClosureKind, CtorCarries, Fixed, IntTy, Symbol, Synth, Value};
use std::sync::Arc;

fn fixed(ty: IntTy, n: i128) -> Value {
    Value::Fixed(Fixed::of(ty, n).expect("a value of the width"))
}

fn list(c: Carry) -> Carry {
    Carry::List(Box::new(c))
}

/// `Option`'s constructors, over its one parameter.
fn options() -> CtorCarries {
    CtorCarries::from([
        (Symbol::new("Some"), vec![Carry::Var(0)]),
        (Symbol::new("None"), vec![]),
    ])
}

fn bound(params: &[Carry], args: &[Value], ctors: &CtorCarries) -> Vec<Carry> {
    let mut vars = Vec::new();
    for (param, arg) in params.iter().zip(args) {
        param.bind(arg, &mut vars, ctors);
    }
    vars
}

#[test]
fn a_variable_reads_as_the_first_value_that_shows_it() {
    let ctors = options();
    // The first inner list shows nothing; the second shows a `U8`.
    let nested = Value::list(vec![
        Value::list(vec![]),
        Value::list(vec![fixed(IntTy::U8, 1)]),
    ]);
    assert_eq!(
        bound(&[list(list(Carry::Var(0)))], &[nested], &ctors),
        [Carry::Width(IntTy::U8)]
    );
    // An `Int` shows a type with no width in it.
    assert_eq!(
        bound(
            &[list(Carry::Var(0))],
            &[Value::list(vec![Value::Int(3)])],
            &ctors
        ),
        [Carry::Plain]
    );
    // An empty list shows nothing, so the variable stays open.
    let open = bound(&[list(Carry::Var(0))], &[Value::list(vec![])], &ctors);
    assert_eq!(Carry::Var(0).instantiate(&open), Carry::Open);
}

#[test]
fn a_constructor_shows_its_parameters_through_its_declaration() {
    let ctors = options();
    let option = Carry::Sum(vec![Carry::Var(0)]);
    let some = Value::ctor("Some", vec![fixed(IntTy::I16, -3)]);
    assert_eq!(
        bound(
            std::slice::from_ref(&option),
            std::slice::from_ref(&some),
            &ctors
        ),
        [Carry::Width(IntTy::I16)]
    );
    // A variable holding an `Option` reads as the sum at what its payload shows.
    assert_eq!(
        bound(&[Carry::Var(0)], &[some], &ctors),
        [Carry::Sum(vec![Carry::Width(IntTy::I16)])]
    );
    let none = Value::ctor("None", vec![]);
    let open = bound(&[option], &[none], &ctors);
    assert_eq!(Carry::Var(0).instantiate(&open), Carry::Open);
}

#[test]
fn a_generated_function_shows_its_answers_and_its_argument() {
    let ctors = options();
    let table = Value::Closure(Arc::new(Closure {
        name: None,
        kind: ClosureKind::Synth {
            arity: 1,
            rule: Synth::Table {
                entries: vec![(Value::Int(1), fixed(IntTy::U32, 7))],
                default: fixed(IntTy::U32, 0),
            },
        },
    }));
    let function = Carry::Fn(vec![Carry::Var(0)], Box::new(Carry::Var(1)));
    assert_eq!(
        bound(&[function], &[table], &ctors),
        [Carry::Plain, Carry::Width(IntTy::U32)]
    );
}

#[test]
fn a_constructor_field_reads_at_the_parameters_its_type_is_at() {
    let ctors = options();
    let some = Symbol::new("Some");
    let at_u8 = Carry::Sum(vec![Carry::Width(IntTy::U8)]);
    assert_eq!(at_u8.ctor_field(&some, 0, &ctors), Carry::Width(IntTy::U8));
    // Nothing below a plain type is a width, and an open one leaves its parameters open.
    assert_eq!(Carry::Plain.ctor_field(&some, 0, &ctors), Carry::Plain);
    assert_eq!(Carry::Open.ctor_field(&some, 0, &ctors), Carry::Open);
}
