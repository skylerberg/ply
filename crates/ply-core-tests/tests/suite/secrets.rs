// `Value`'s `Arc` payloads are deliberately not `Send`.
#![allow(clippy::arc_with_non_send_sync)]

use ply_core::{Plain, Symbol, Value, codes, constant_time_eq, values_equal};
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
        &Value::builtin(ply_core::Builtin::Digest),
        ply_core::Span::DUMMY,
    )
    .expect_err("a function has no equality");
    let text = format!("{d:#?}");
    assert!(!text.contains("hunter2"), "{text}");
}

#[test]
fn a_secret_is_never_equal_to_its_payload() {
    let span = ply_core::Span::DUMMY;
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
    let d = ply_core::builtins::call(
        ply_core::Builtin::CompareValues,
        vec![Value::secret_text("a"), Value::secret_text("b")],
        ply_core::Span::DUMMY,
    )
    .expect_err("a credential has no order");
    assert_eq!(d.code, codes::RUNTIME_ERROR);
    assert!(
        d.notes.iter().any(|n| n.contains("Secret::verify")),
        "{d:#?}"
    );
    assert!(!format!("{d:#?}").contains("\"a\""), "{d:#?}");
}

/// A credential reaching a handler that does not declare it may receive one is refused before the
/// handler is entered, wherever in the argument it sits, and the refusal names the operation, the
/// argument and the handler, and never the payload.
#[test]
fn a_secret_reaching_a_handler_that_does_not_declare_one_is_e0439() {
    let secret = Value::secret_text("hunter2");
    let nested = Value::Record(Arc::new(ply_core::Fields::from_unsorted(vec![
        (Symbol::new("user"), Value::str("ada")),
        (Symbol::new("password"), secret.clone()),
    ])));
    assert!(ply_core::host::carries_secret(&secret));
    assert!(ply_core::host::carries_secret(&nested));
    assert!(!ply_core::host::carries_secret(&Value::Int(1)));
    let d = ply_core::host::err_secret_to_host(
        ply_core::Span::DUMMY,
        "net.send[socket]",
        0,
        "test::send",
    );
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
