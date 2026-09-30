//! Reading the world `proof.world` hands over: every shape it can take, and every one it must not.

use ply_eval::decode::At;
use ply_eval::{SourceId, Span, Symbol, Value};
use ply_prove::domain::Shape;
use ply_prove::{Obligation, ObligationKind, Points, Sort, Strategy, Unsettled, World};
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

fn none() -> Value {
    Value::ctor("None", Vec::new())
}

fn some(value: Value) -> Value {
    Value::ctor("Some", vec![value])
}

fn strategy(name: &str, args: Vec<Value>) -> Value {
    Value::ctor(format!("proof.world.{name}"), args)
}

fn sampled() -> Value {
    strategy(
        "Static",
        vec![strategy("Run", vec![strategy("Drawn", Vec::new())])],
    )
}

/// A clause of `m.pick`, over `binders` and a `result`, whose variables are `variables`.
fn obligation(
    key: &str,
    binders: Vec<Value>,
    result: Option<Value>,
    variables: &[&str],
    how: Value,
) -> Value {
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
        ("result", result.map_or_else(none, some)),
        (
            "variables",
            Value::list(variables.iter().map(|v| Value::str(*v)).collect()),
        ),
        ("guarded", Value::Bool(true)),
        ("host", Value::Bool(false)),
        ("footprint", none()),
        ("frame", Value::ctor("proof.obligation.Pure", Vec::new())),
        ("strategy", how),
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

/// The one obligation of a world holding only `obligation`.
fn read(obligation: Value) -> Obligation {
    let value = world(vec![], vec![], vec![obligation]);
    let (_, mut obligations) =
        World::decode(At::new("the world", &value)).expect("a well-formed world reads");
    obligations.remove(0)
}

fn bool_domain() -> Value {
    record(vec![
        (
            "shapes",
            Value::list(vec![ty("Scalar", vec![Value::str("Bool"), Value::Int(2)])]),
        ),
        ("points", Value::Int(2)),
    ])
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
    let value = world(
        vec![],
        vec![],
        vec![obligation("xyz", vec![], None, &[], sampled())],
    );
    let why = refusal_of(&value);
    assert_eq!(why.path, "the world.obligations[0].key");
}

/// A variable the claim does not name would reach a report under no name at all.
#[test]
fn a_binder_over_a_variable_the_claim_does_not_name_is_refused() {
    let value = world(
        vec![],
        vec![],
        vec![obligation(
            KEY,
            vec![binder("first", var(0), "a"), binder("second", var(1), "b")],
            None,
            &["a"],
            sampled(),
        )],
    );
    let why = refusal_of(&value);
    assert_eq!(why.path, "the world.obligations[0].binders[1].ty");
    assert!(
        why.message.contains("variable 1 of a claim that names 1"),
        "{why}"
    );
}

/// A claim over one `Bool`, discharged `how`.
fn flipped(how: Value) -> Value {
    obligation(KEY, vec![binder("b", con("Bool"), "Bool")], None, &[], how)
}

#[test]
fn every_strategy_reads_as_the_search_it_names() {
    let over = |how: Value| read(flipped(how)).strategy;
    let every = strategy("Every", vec![bool_domain(), Value::str("Bool")]);
    let Strategy::Interleave(Points::Every(finite)) =
        over(strategy("Interleave", vec![every.clone()]))
    else {
        panic!("an interleaving search over every point");
    };
    assert_eq!(finite.name.as_str(), "Bool");
    assert_eq!(finite.points, 2);
    assert_eq!(
        finite.shapes,
        [Shape::Scalar {
            name: "Bool".to_string(),
            size: 2
        }]
    );
    assert!(matches!(
        over(strategy("Interleave", vec![strategy("Drawn", Vec::new())])),
        Strategy::Interleave(Points::Drawn)
    ));
    assert!(matches!(
        over(strategy("Hosted", Vec::new())),
        Strategy::Hosted
    ));
    let Strategy::Static(Unsettled::Unhandled(row)) = over(strategy(
        "Static",
        vec![strategy("Unhandled", vec![Value::str("{m.store.read}")])],
    )) else {
        panic!("a static attempt, then the gap");
    };
    assert_eq!(row, "{m.store.read}");
    assert!(matches!(
        over(strategy("Static", vec![strategy("Run", vec![every])])),
        Strategy::Static(Unsettled::Run(Points::Every(_)))
    ));
    assert!(matches!(
        over(sampled()),
        Strategy::Static(Unsettled::Run(Points::Drawn))
    ));
}

#[test]
fn a_strategy_the_reader_does_not_know_is_refused() {
    let value = world(
        vec![],
        vec![],
        vec![obligation(
            KEY,
            vec![],
            None,
            &[],
            strategy("Guess", Vec::new()),
        )],
    );
    let why = refusal_of(&value);
    assert_eq!(why.path, "the world.obligations[0].strategy");
    assert!(why.message.contains("`Guess`"), "{why}");
}

/// A count the shapes do not multiply out to would walk a point twice or miss one.
#[test]
fn a_domain_its_shapes_do_not_count_is_refused() {
    let counted = |points: i64| {
        let domain = record(vec![
            (
                "shapes",
                Value::list(vec![ty("Scalar", vec![Value::str("Bool"), Value::Int(2)])]),
            ),
            ("points", Value::Int(points)),
        ]);
        let every = strategy("Every", vec![domain, Value::str("Bool")]);
        world(
            vec![],
            vec![],
            vec![flipped(strategy(
                "Static",
                vec![strategy("Run", vec![every])],
            ))],
        )
    };
    let why = refusal_of(&counted(3));
    assert!(
        why.message
            .contains("3 points, which the binders' shapes do not multiply out to"),
        "{why}"
    );
    assert!(refusal_of(&counted(0)).message.contains("no points"));
    assert!(World::decode(At::new("the world", &counted(2))).is_ok());
}

/// A point decodes one value per shape, and the body is entered with one per binder.
#[test]
fn a_domain_of_another_arity_than_its_claim_is_refused() {
    let every = strategy("Every", vec![bool_domain(), Value::str("Bool")]);
    let value = world(
        vec![],
        vec![],
        vec![obligation(
            KEY,
            vec![
                binder("b", con("Bool"), "Bool"),
                binder("c", con("Bool"), "Bool"),
            ],
            None,
            &[],
            strategy("Interleave", vec![every]),
        )],
    );
    let why = refusal_of(&value);
    assert_eq!(why.path, "the world.obligations[0].strategy");
    assert!(
        why.message
            .contains("a domain over 1 binder(s), for a claim of 2"),
        "{why}"
    );
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
            vec![binder("first", var(0), "a"), binder("second", var(1), "b")],
            Some(binder("result", var(0), "a")),
            &["a", "b"],
            sampled(),
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
    assert_eq!(
        pick.sort,
        Sort::func(vec![Sort::Var(0), Sort::Var(1)], Sort::Var(0), true)
    );

    assert_eq!(obligations.len(), 1);
    let o = &obligations[0];
    assert_eq!(o.key.to_hex(), KEY);
    assert_eq!(o.owner.as_str(), "m.pick");
    assert_eq!(o.kind, ObligationKind::Ensures { index: 1 });
    assert_eq!(o.span, Span::new(SourceId(0), 12, 30));
    assert!(o.footprint.is_none());
    assert_eq!(
        o.all_binders()
            .iter()
            .map(|b| (b.name.as_str(), b.sort.clone(), b.text.as_str()))
            .collect::<Vec<_>>(),
        [
            ("first", Sort::Var(0), "a"),
            ("second", Sort::Var(1), "b"),
            ("result", Sort::Var(0), "a"),
        ]
    );
    // What a point assigns: a clause's `result` is its owner's answer, and not drawn.
    assert_eq!(o.binders.len(), 2);
    assert_eq!(o.result.as_ref().map(|b| b.name.as_str()), Some("result"));
    assert_eq!(o.variables, [Symbol::new("a"), Symbol::new("b")]);
    assert!(matches!(
        o.strategy,
        Strategy::Static(Unsettled::Run(Points::Drawn))
    ));
}
