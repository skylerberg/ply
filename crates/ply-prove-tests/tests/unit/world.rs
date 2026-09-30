//! Reading the world `proof.world` hands over: every shape it can take, and every one it must not.

use ply_eval::Value;
use ply_eval::decode::At;
use ply_prove::sort::var_name;
use ply_prove::{ObligationKind, Sort, World};
use ply_span::{SourceId, Span, Symbol};
use std::sync::Arc;

#[allow(clippy::arc_with_non_send_sync)]
fn record(fields: Vec<(&str, Value)>) -> Value {
    Value::Record(Arc::new(
        fields
            .into_iter()
            .map(|(name, value)| (Symbol::new(name), value))
            .collect(),
    ))
}

fn ty(name: &str, args: Vec<Value>) -> Value {
    Value::ctor(format!("proof.domain.{name}"), args)
}

fn con(name: &str) -> Value {
    ty("Con", vec![Value::str(name), Value::list(Vec::new())])
}

fn var(i: i64) -> Value {
    ty("Var", vec![Value::Int(i)])
}

fn decl(name: &str, params: i64, variants: Vec<(&str, Vec<Value>)>) -> Value {
    record(vec![
        ("name", Value::str(name)),
        ("params", Value::Int(params)),
        (
            "variants",
            Value::list(
                variants
                    .into_iter()
                    .map(|(variant, fields)| {
                        record(vec![
                            ("name", Value::str(variant)),
                            ("fields", Value::list(fields)),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

fn world(decls: Vec<Value>, signatures: Vec<Value>, obligations: Vec<Value>) -> Value {
    record(vec![
        ("decls", Value::list(decls)),
        ("signatures", Value::list(signatures)),
        ("obligations", Value::list(obligations)),
    ])
}

fn obligation(key: &str, binders: Vec<Value>) -> Value {
    record(vec![
        ("key", Value::str(key)),
        ("owner", Value::str("m.pick")),
        (
            "kind",
            Value::ctor("proof.obligation.Ensures", vec![Value::Int(1)]),
        ),
        (
            "at",
            record(vec![
                ("module", Value::Int(0)),
                ("start", Value::Int(12)),
                ("end", Value::Int(30)),
            ]),
        ),
        ("binders", Value::list(binders)),
        ("guarded", Value::Bool(true)),
        ("host", Value::Bool(false)),
        ("footprint", Value::ctor("None", Vec::new())),
    ])
}

fn binder(name: &str, sort: Value, text: &str) -> Value {
    record(vec![
        ("name", Value::str(name)),
        ("ty", sort),
        ("text", Value::str(text)),
    ])
}

const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn refusal_of(value: &Value) -> ply_eval::decode::Error {
    match World::decode(At::new("the world", value)) {
        Ok(_) => panic!("a malformed world was read"),
        Err(e) => e,
    }
}

#[test]
fn a_sort_reads_every_shape_a_type_takes() {
    let value = ty(
        "Fn",
        vec![
            Value::list(vec![
                var(0),
                ty(
                    "Record",
                    vec![Value::list(vec![
                        record(vec![("name", Value::str("y")), ("ty", con("Bool"))]),
                        record(vec![("name", Value::str("x")), ("ty", var(1))]),
                    ])],
                ),
            ]),
            ty("Con", vec![Value::str("List"), Value::list(vec![var(0)])]),
            Value::Bool(false),
        ],
    );
    let read = Sort::decode(At::new("the sort", &value)).expect("a well-formed sort reads");
    assert_eq!(
        read,
        Sort::func(
            vec![
                Sort::Var(0),
                // Ascending by name, whatever order they were listed in.
                Sort::record([
                    (Symbol::new("x"), Sort::Var(1)),
                    (Symbol::new("y"), Sort::bool()),
                ]),
            ],
            Sort::list(Sort::Var(0)),
            false,
        )
    );
}

#[test]
fn a_variable_prints_as_the_letter_the_compiler_gives_its_place() {
    let named: Vec<String> = [0, 1, 2, 3, 4, 22, 23, 24, 46].map(var_name).to_vec();
    // `e`, `f` and `t` are the rows' letters, so the types' skip them.
    assert_eq!(named, ["a", "b", "c", "d", "g", "z", "a1", "b1", "a2"]);
    assert_eq!(
        Sort::func(
            vec![Sort::Var(0)],
            Sort::Con(Symbol::new("Pair"), vec![Sort::Var(0), Sort::Var(1)]),
            true
        )
        .to_string(),
        "(a) -> Pair<a, b>"
    );
    assert_eq!(
        Sort::record([
            (Symbol::new("_0"), Sort::int()),
            (Symbol::new("_1"), Sort::bool()),
        ])
        .to_string(),
        "(Int, Bool)"
    );
    assert_eq!(
        Sort::record([(Symbol::new("n"), Sort::int())]).to_string(),
        "{n: Int}"
    );
}

#[test]
fn a_type_the_reader_does_not_know_is_refused_where_it_is() {
    let value = ty("Arrow", vec![]);
    let why = Sort::decode(At::new("the sort", &value)).expect_err("no sort is an `Arrow`");
    assert_eq!(why.path, "the sort");
    assert!(why.message.contains("`Arrow`"), "{why}");
}

#[test]
fn a_variable_numbered_below_zero_is_refused() {
    let value = var(-1);
    let why = Sort::decode(At::new("the sort", &value)).expect_err("no variable is numbered -1");
    assert_eq!(why.path, "the sort.Var");
}

#[test]
fn a_record_naming_a_field_twice_is_refused() {
    let value = ty(
        "Record",
        vec![Value::list(vec![
            record(vec![("name", Value::str("x")), ("ty", con("Int"))]),
            record(vec![("name", Value::str("x")), ("ty", con("Bool"))]),
        ])],
    );
    let why = Sort::decode(At::new("the sort", &value)).expect_err("a field is named once");
    assert_eq!(why.path, "the sort.Record[1]");
    assert!(why.message.contains("a second field named `x`"), "{why}");
}

#[test]
fn a_field_over_a_parameter_its_type_does_not_take_is_refused() {
    let value = world(
        vec![decl(
            "m.Box",
            1,
            vec![("m.Empty", vec![]), ("m.Full", vec![var(0), var(1)])],
        )],
        vec![],
        vec![],
    );
    let why = refusal_of(&value);
    assert_eq!(why.path, "the world.decls[0].variants[1].fields[1]");
    assert!(
        why.message
            .contains("a field over parameter 1 of a type that takes 1"),
        "{why}"
    );
}

#[test]
fn a_constructor_of_two_types_is_refused() {
    let value = world(
        vec![
            decl("m.A", 0, vec![("m.Same", vec![])]),
            decl("m.B", 0, vec![("m.Same", vec![])]),
        ],
        vec![],
        vec![],
    );
    let why = refusal_of(&value);
    assert_eq!(why.path, "the world.decls[1]");
    assert!(why.message.contains("`m.Same`"), "{why}");
}

#[test]
fn an_obligation_keyed_by_something_not_a_hash_is_refused() {
    let value = world(vec![], vec![], vec![obligation("xyz", vec![])]);
    let why = refusal_of(&value);
    assert_eq!(why.path, "the world.obligations[0].key");
}

#[test]
fn a_world_reads_its_types_its_signatures_and_its_obligations() {
    let value = world(
        vec![decl(
            "m.Pair",
            1,
            vec![("m.Both", vec![var(0), var(0)]), ("m.Neither", vec![])],
        )],
        vec![record(vec![
            ("name", Value::str("m.pick")),
            (
                "ty",
                ty(
                    "Fn",
                    vec![Value::list(vec![var(0), var(1)]), var(0), Value::Bool(true)],
                ),
            ),
            ("pure", Value::Bool(true)),
        ])],
        vec![obligation(
            KEY,
            vec![
                binder("first", var(0), "a"),
                binder("second", var(1), "b"),
                binder("result", var(0), "a"),
            ],
        )],
    );
    let (world, obligations) =
        World::decode(At::new("the world", &value)).expect("a well-formed world reads");

    let both = world
        .ctor(&Symbol::new("m.Both"))
        .expect("a declared constructor");
    assert_eq!(both.decl.name.as_str(), "m.Pair");
    assert_eq!(both.decl.params, 1);
    assert_eq!(both.variant.index, 0);
    assert_eq!(
        world.fields(both.variant, &[Sort::bool()]),
        [Sort::bool(), Sort::bool()]
    );
    assert_eq!(
        world
            .ctor(&Symbol::new("m.Neither"))
            .map(|c| c.variant.index),
        Some(1)
    );
    let pick = world
        .signature(&Symbol::new("m.pick"))
        .expect("a signature");
    assert!(pick.pure);
    assert_eq!(pick.sort.to_string(), "(a, b) -> a");

    assert_eq!(obligations.len(), 1);
    let o = &obligations[0];
    assert_eq!(o.key.to_hex(), KEY);
    assert_eq!(o.owner.as_str(), "m.pick");
    assert_eq!(o.kind, ObligationKind::Ensures { index: 1 });
    assert_eq!(o.span, Span::new(SourceId(0), 12, 30));
    assert!(o.guarded && !o.host && o.footprint.is_none());
    assert_eq!(
        o.binders
            .iter()
            .map(|b| (b.name.as_str(), b.sort.clone(), b.text.as_str()))
            .collect::<Vec<_>>(),
        [
            ("first", Sort::Var(0), "a"),
            ("second", Sort::Var(1), "b"),
            ("result", Sort::Var(0), "a"),
        ]
    );
    // What a point assigns: a clause's `result` is its owner's answer.
    assert_eq!(o.generated().len(), 2);
}
