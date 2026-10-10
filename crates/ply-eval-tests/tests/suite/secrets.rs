// `Value`'s `Arc` payloads are deliberately not `Send`.
#![allow(clippy::arc_with_non_send_sync)]

use ply_eval::{Plain, Symbol, Value, codes, constant_time_eq, values_equal};
use std::sync::Arc;

#[test]
fn a_secret_is_copied_out_with_no_payload_whatever_it_holds() {
    for secret in [
        Value::secret_text("hunter2"),
        Value::secret_text(""),
        Value::secret_bytes(b"\x00\xff"),
        Value::secret_bytes(b"hunter2"),
    ] {
        assert_eq!(Plain::of(&secret), Plain::Secret);
        assert_eq!(Plain::shown(&secret), Plain::Secret);
        assert!(!format!("{secret:?}").contains("hunter2"));
    }
}

#[test]
fn a_nested_secret_is_copied_out_with_no_payload() {
    let inner = Value::secret_text("hunter2");
    let outer = Value::list(vec![
        Value::ctor("Some", vec![inner.clone()]),
        Value::map([(Value::str("password"), inner.clone())]),
        inner,
    ]);
    assert_eq!(
        Plain::of(&outer),
        Plain::List(vec![
            Plain::Ctor("Some".to_string(), vec![Plain::Secret]),
            Plain::Map(vec![(Plain::Str("password".to_string()), Plain::Secret)]),
            Plain::Secret,
        ])
    );
}

/// `type_error` interpolates the offending value, so this is a route the redaction has to cover.
#[test]
fn a_runtime_type_error_over_a_secret_prints_no_payload() {
    let d = values_equal(
        &Value::secret_text("hunter2"),
        &Value::builtin(ply_eval::Builtin::Len),
        ply_eval::Span::DUMMY,
    )
    .expect_err("a function has no equality");
    let text = format!("{d:#?}");
    assert!(!text.contains("hunter2"), "{text}");
}

#[test]
fn a_secret_is_never_equal_to_its_payload() {
    let span = ply_eval::Span::DUMMY;
    let secret = Value::secret_text("hunter2");
    let plain = Value::str("hunter2");
    assert!(!values_equal(&secret, &plain, span).unwrap());
    assert!(!values_equal(&plain, &secret, span).unwrap());
    assert!(values_equal(&secret, &secret.clone(), span).unwrap());
}

/// Both operands are scanned in full whatever the answer, so timing is not an oracle.
#[test]
fn the_comparison_is_over_the_whole_of_both_operands() {
    assert!(constant_time_eq(b"", b""));
    assert!(constant_time_eq(b"hunter2", b"hunter2"));
    assert!(!constant_time_eq(b"hunter2", b"hunter3"));
    assert!(!constant_time_eq(b"aunter2", b"hunter2"));
    assert!(!constant_time_eq(b"hunter2", b"hunter"));
    assert!(!constant_time_eq(b"", b"\0"));
    assert!(!constant_time_eq(b"\0", b""));
}

/// The runtime backstop under `derivable(ord, ·)`.
#[test]
fn compare_values_refuses_a_secret_at_run_time() {
    let d = ply_eval::builtins::call(
        ply_eval::Builtin::CompareValues,
        vec![Value::secret_text("a"), Value::secret_text("b")],
        ply_eval::Span::DUMMY,
    )
    .expect_err("a credential has no order");
    assert_eq!(d.code, codes::RUNTIME_ERROR);
    assert!(
        d.notes.iter().any(|n| n.contains("secret_verify")),
        "{d:#?}"
    );
    assert!(!format!("{d:#?}").contains("\"a\""), "{d:#?}");
}

#[test]
fn a_secret_key_is_refused_by_every_map_operation_that_takes_one() {
    let key = Value::secret_text("a");
    for (builtin, args) in [
        (
            ply_eval::Builtin::MapInsert,
            vec![Value::empty_map(), key.clone(), Value::Int(1)],
        ),
        (
            ply_eval::Builtin::MapGet,
            vec![Value::empty_map(), key.clone()],
        ),
        (
            ply_eval::Builtin::MapContains,
            vec![Value::empty_map(), key.clone()],
        ),
        (
            ply_eval::Builtin::MapRemove,
            vec![Value::empty_map(), key.clone()],
        ),
    ] {
        let d = ply_eval::builtins::call(builtin, args, ply_eval::Span::DUMMY)
            .err()
            .unwrap_or_else(|| panic!("{} accepted a Secret key", builtin.name()));
        assert_eq!(d.code, codes::RUNTIME_ERROR, "{}", builtin.name());
    }
}

/// A credential reaching a handler that does not declare it may receive one is refused before the
/// handler is entered, wherever in the argument it sits, and the refusal names the operation, the
/// argument and the handler, and never the payload.
#[test]
fn a_secret_reaching_a_handler_that_does_not_declare_one_is_e0439() {
    let secret = Value::secret_text("hunter2");
    let nested = Value::Record(Arc::new(ply_eval::Fields::from_unsorted(vec![
        (Symbol::new("user"), Value::str("ada")),
        (Symbol::new("password"), secret.clone()),
    ])));
    assert!(ply_eval::carries_secret(&secret));
    assert!(ply_eval::carries_secret(&nested));
    assert!(!ply_eval::carries_secret(&Value::Int(1)));
    let d = ply_eval::err_secret_to_host(ply_eval::Span::DUMMY, "net.send[socket]", 0, "test::send");
    assert_eq!(d.code, codes::SECRET_TO_HOST);
    assert!(d.message.contains("net.send[socket]"), "{}", d.message);
    assert!(d.message.contains("argument 1"), "{}", d.message);
    assert!(
        d.notes.iter().any(|n| n.contains("test::send")),
        "{:#?}",
        d.notes
    );
    assert!(!format!("{d:#?}").contains("hunter2"), "{d:#?}");
}

/// At the builtin, independent of whether any source program can still reach it.
#[test]
fn map_of_entries_refuses_a_secret_key() {
    let refused = ply_eval::builtins::call(
        ply_eval::Builtin::MapOfEntries,
        vec![Value::list(vec![
            entry(Value::secret_text("hunter2"), Value::Int(1)),
            entry(Value::secret_text("hunter1"), Value::Int(0)),
        ])],
        ply_eval::Span::DUMMY,
    );
    let d = refused.expect_err("`map_of_entries` refuses a `Secret` key");
    assert_eq!(d.code, codes::RUNTIME_ERROR, "{d:#?}");
    assert!(
        d.message.contains("cannot order a `Secret`"),
        "the backstop names the credential: {}",
        d.message
    );
}

#[test]
fn map_merge_refuses_a_secret_key() {
    // No map builtin builds this right-hand side, so it is assembled directly.
    let right = Value::map([(Value::secret_text("hunter2"), Value::Int(1))]);
    let refused = ply_eval::builtins::call(
        ply_eval::Builtin::MapMerge,
        vec![Value::empty_map(), right],
        ply_eval::Span::DUMMY,
    );
    let d = refused.expect_err("`map_merge` refuses a `Secret` key");
    assert_eq!(d.code, codes::RUNTIME_ERROR, "{d:#?}");
    assert!(d.message.contains("cannot order a `Secret`"), "{d:#?}");
}

#[test]
fn every_map_operation_that_orders_a_key_refuses_a_secret() {
    let secret = Value::secret_text("hunter2");
    let cases: Vec<(ply_eval::Builtin, Vec<Value>)> = vec![
        (
            ply_eval::Builtin::MapInsert,
            vec![Value::empty_map(), secret.clone(), Value::Int(0)],
        ),
        (
            ply_eval::Builtin::MapGet,
            vec![Value::empty_map(), secret.clone()],
        ),
        (
            ply_eval::Builtin::MapContains,
            vec![Value::empty_map(), secret.clone()],
        ),
        (
            ply_eval::Builtin::MapRemove,
            vec![Value::empty_map(), secret.clone()],
        ),
        (
            ply_eval::Builtin::MapOfEntries,
            vec![Value::list(vec![entry(secret.clone(), Value::Int(0))])],
        ),
        (
            ply_eval::Builtin::MapMerge,
            vec![
                Value::empty_map(),
                Value::map([(secret.clone(), Value::Int(0))]),
            ],
        ),
        (
            ply_eval::Builtin::CompareValues,
            vec![secret.clone(), secret.clone()],
        ),
        (
            ply_eval::Builtin::MapFloor,
            vec![Value::empty_map(), secret.clone()],
        ),
        (
            ply_eval::Builtin::MapCeiling,
            vec![Value::empty_map(), secret.clone()],
        ),
        (
            ply_eval::Builtin::MapBelow,
            vec![Value::empty_map(), secret.clone()],
        ),
        (
            ply_eval::Builtin::MapAbove,
            vec![Value::empty_map(), secret.clone()],
        ),
        (
            ply_eval::Builtin::MapSplit,
            vec![Value::empty_map(), secret.clone()],
        ),
        (
            ply_eval::Builtin::MapRange,
            vec![
                Value::empty_map(),
                Value::ctor("None", Vec::new()),
                Value::Bool(true),
                Value::ctor("Some", vec![secret.clone()]),
                Value::Bool(true),
                Value::Int(1),
            ],
        ),
    ];
    for (builtin, args) in cases {
        let refused = ply_eval::builtins::call(builtin, args, ply_eval::Span::DUMMY);
        let d = refused
            .err()
            .unwrap_or_else(|| panic!("{builtin:?} accepted a `Secret` key"));
        assert_eq!(d.code, codes::RUNTIME_ERROR, "{builtin:?}: {d:#?}");
        assert!(
            d.message.contains("cannot order a `Secret`"),
            "{builtin:?}: {}",
            d.message
        );
    }
    // No `Secret` is a native key, so `map_update` always reaches its key through `take`.
    let d = ply_eval::map::take(Value::empty_map(), &secret, ply_eval::Span::DUMMY)
        .expect_err("`map_update` refuses a `Secret` key");
    assert_eq!(d.code, codes::RUNTIME_ERROR, "{d:#?}");
    assert!(d.message.contains("cannot order a `Secret`"), "{d:#?}");
}

/// A `{key, value}` record, as `map_of_entries` reads one.
fn entry(k: Value, v: Value) -> Value {
    use std::collections::BTreeMap;
    let mut fields = BTreeMap::new();
    fields.insert(Symbol::new("key"), k);
    fields.insert(Symbol::new("value"), v);
    Value::Record(Arc::new(fields.into_iter().collect()))
}
