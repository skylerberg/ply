// `Value`'s `Arc` payloads are deliberately not `Send`.
#![allow(clippy::arc_with_non_send_sync)]

use crate::fixture::Compiled;
use ply_eval::host::{
    Determinism, HostAnswer, HostBinding, HostHandler, HostOp, HostRegistry, HostRequest,
    HostResource, HostRuntime, Linearity,
};
use ply_eval::{Diagnostic, Plain, Symbol, Value, codes, constant_time_eq, values_equal};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Runs test 0 and answers its diagnostic, if any.
fn run(source: &str) -> Result<(), Diagnostic> {
    let compiled = Compiled::named("t", source);
    let mut machine = compiled.machine_on_backend();
    machine.eval_test(0).into_parts().0
}

#[track_caller]
fn passes(source: &str) {
    if let Err(d) = run(source) {
        panic!("expected this to pass:\n{source}\ngot {d:#?}");
    }
}

#[track_caller]
fn fails(source: &str) -> Diagnostic {
    run(source).expect_err("the program was expected to fail")
}

#[test]
fn a_secret_is_copied_out_with_no_payload_whatever_it_holds() {
    for payload in [
        Value::str("hunter2"),
        Value::str(""),
        Value::bytes(b"\x00\xff"),
        Value::Int(1),
        Value::list(vec![Value::str("a"), Value::str("b")]),
    ] {
        let secret = Value::secret(payload);
        assert_eq!(Plain::of(&secret), Plain::Secret);
        assert_eq!(Plain::shown(&secret), Plain::Secret);
        assert!(!format!("{secret:?}").contains("hunter2"));
    }
}

#[test]
fn a_nested_secret_is_copied_out_with_no_payload() {
    let inner = Value::secret(Value::str("hunter2"));
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

#[test]
fn a_failing_assertion_prints_no_payload() {
    let d = fails(
        r#"
test "two logins differ" {
  assert_eq(
    {user: "ada", password: secret_of_string("hunter2")},
    {user: "ada", password: secret_of_string("correct-horse")})
}
"#,
    );
    let text = format!("{d:#?}");
    assert!(!text.contains("hunter2"), "{text}");
    assert!(!text.contains("correct-horse"), "{text}");
    assert!(
        d.values.iter().any(|v| format!("{v:?}").contains("Secret")),
        "{text}"
    );
}

/// `type_error` interpolates the offending value, so this is a route the redaction has to cover.
#[test]
fn a_runtime_type_error_over_a_secret_prints_no_payload() {
    let d = values_equal(
        &Value::secret(Value::str("hunter2")),
        &Value::builtin(ply_eval::Builtin::Len),
        ply_eval::Span::DUMMY,
    )
    .expect_err("a function has no equality");
    let text = format!("{d:#?}");
    assert!(!text.contains("hunter2"), "{text}");
}

#[test]
fn two_secrets_are_equal_exactly_when_their_payloads_are() {
    passes(
        r#"
test "equality works and prints nothing" {
  let a = secret_of_string("hunter2");
  let b = secret_of_string("hunter2");
  let c = secret_of_string("hunter3");
  assert(a == b);
  assert(a != c);
  assert_eq({k: a}, {k: b})
}
"#,
    );
}

#[test]
fn a_secret_is_never_equal_to_its_payload() {
    let span = ply_eval::Span::DUMMY;
    let secret = Value::secret(Value::str("hunter2"));
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
        vec![
            Value::secret(Value::str("a")),
            Value::secret(Value::str("b")),
        ],
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
    let key = Value::secret(Value::str("a"));
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

#[test]
fn verify_answers_one_bit_and_is_empty_answers_presence() {
    passes(
        r#"
test "the three builtins" {
  let s = secret_of_string("hunter2");
  assert(secret_verify(s, "hunter2"));
  assert(!secret_verify(s, "hunter3"));
  assert(!secret_verify(s, ""));
  assert(!secret_is_empty(s));
  assert(secret_is_empty(secret_of_string("")))
}
"#,
    );
}

#[test]
fn a_secret_of_bytes_is_made_joined_measured_and_compared_and_never_read() {
    passes(
        r#"
test "bytes behind a secret" {
  let a = secret_of_bytes(b"hunter2");
  assert(a == secret_of_bytes(b"hunter2"));
  assert(a != secret_of_bytes(b"hunter3"));
  assert(a != secret_of_bytes(b"hunter"));
  assert(a != secret_of_bytes(b"hunter22"));
  assert(a == secret_bytes(secret_of_string("hunter2")));
  assert_eq(secret_len(a), 7);
  assert(secret_concat(secret_of_bytes(b"hun"), secret_of_bytes(b"ter2")) == a);
  assert(secret_decode("hex", secret_of_string("68756e74657232")) == Some(a));
  assert(secret_decode("hex", secret_of_string("68756e7465723")) == None);
  assert(!secret_is_empty(a));
  assert(secret_is_empty(secret_of_bytes(b"")));
  assert_eq({k: a}, {k: secret_of_bytes(b"hunter2")})
}
"#,
    );
}

#[test]
fn a_failing_assertion_over_bytes_prints_no_payload() {
    let d = fails(
        r#"
test "two keys differ" {
  assert_eq(secret_of_bytes(b"hunter2"), secret_concat(secret_of_bytes(b"correct-"), secret_of_bytes(b"horse")))
}
"#,
    );
    let text = format!("{d:#?}");
    assert!(!text.contains("hunter2"), "{text}");
    assert!(!text.contains("horse"), "{text}");
}

#[test]
fn a_secret_of_bytes_is_no_more_shown_ordered_or_matched_than_one_of_a_string() {
    for source in [
        "fn f(s: Secret<Bytes>) -> String = f\"{s}\"",
        "fn f(a: Secret<Bytes>, b: Secret<Bytes>) -> Ordering = compare(a, b)",
        "fn f(m: Map<Secret<Bytes>, Int>) -> Int = map_len(m)",
        "fn f(s: Secret<Bytes>) -> Bytes = digest(s)",
    ] {
        let diagnostics = Compiled::rejected(source);
        assert!(
            diagnostics.iter().any(|d| d.code == codes::NOT_DERIVABLE),
            "{source}: {diagnostics:#?}"
        );
    }
}

/// A route secrets do not close: the plaintext is still in scope as a `String`.
#[test]
fn the_plaintext_the_secret_was_built_from_is_not_consumed() {
    passes(
        r#"
test "the source string survives" {
  let plain = "hunter2";
  let s = secret_of_string(plain);
  assert(secret_verify(s, plain));
  assert_eq(string_len(plain), 7)
}
"#,
    );
}

const SEND: &str = r#"
nondet effect net {
  write send[s](payload: Secret<String>) -> Int
}

test/nondet "the credential goes out" {
  assert_eq(net.send[socket](secret_of_string("hunter2")), 1)
}
"#;

const SEND_NESTED: &str = r#"
nondet effect net {
  write send[s](payload: {user: String, password: Secret<String>}) -> Int
}

test/nondet "the credential goes out inside a record" {
  assert_eq(net.send[socket]({user: "ada", password: secret_of_string("hunter2")}), 1)
}
"#;

const SEND_PLAIN: &str = r#"
nondet effect net {
  write send[s](payload: Int) -> Int
}

test/nondet "nothing sensitive goes out" {
  assert_eq(net.send[socket](1), 1)
}
"#;

#[derive(Default)]
struct Counter {
    calls: AtomicU64,
    seen: std::sync::Mutex<Vec<Plain>>,
}

impl HostHandler for Counter {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.seen
            .lock()
            .unwrap()
            .extend(req.args.iter().map(Plain::of));
        Ok(HostAnswer::Value(Value::Int(1)))
    }
}

fn op(secrets: bool) -> HostOp {
    HostOp {
        effect: Symbol::new("net"),
        op: Symbol::new("send"),
        resource: HostResource::Any,
        determinism: Determinism::Nondeterministic,
        linearity: Linearity::AtMostOnce,
        blocking: false,
        secrets,
        path: "test::send",
    }
}

fn bound(compiled: &Compiled, handler: Arc<Counter>, secrets: bool) -> HostBinding {
    let mut registry = HostRegistry::new();
    registry.register(op(secrets), handler);
    registry.bind(&compiled.front.check).expect("binds")
}

#[test]
fn a_secret_reaching_a_handler_that_does_not_declare_one_is_e0439() {
    let compiled = Compiled::named("t", SEND);
    let handler = Arc::new(Counter::default());
    let mut machine = compiled.machine_on_backend();
    machine.set_host_binding(Arc::new(bound(&compiled, handler.clone(), false)));

    let d = machine.eval_test(0).into_parts().0.expect_err("E0439");
    assert_eq!(d.code, codes::SECRET_TO_HOST);
    assert!(d.message.contains("net.send[socket]"), "{}", d.message);
    assert!(d.message.contains("argument 1"), "{}", d.message);
    assert!(
        d.notes.iter().any(|n| n.contains("test::send")),
        "{:#?}",
        d.notes
    );
    assert_eq!(
        handler.calls.load(Ordering::SeqCst),
        0,
        "the handler was entered before the check"
    );
    assert!(!format!("{d:#?}").contains("hunter2"), "{d:#?}");
}

#[test]
fn a_secret_nested_in_an_argument_is_found() {
    let compiled = Compiled::named("t", SEND_NESTED);
    let handler = Arc::new(Counter::default());
    let mut machine = compiled.machine_on_backend();
    machine.set_host_binding(Arc::new(bound(&compiled, handler.clone(), false)));

    let d = machine.eval_test(0).into_parts().0.expect_err("E0439");
    assert_eq!(d.code, codes::SECRET_TO_HOST);
    assert_eq!(handler.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn an_operation_that_declares_secrets_receives_one() {
    let compiled = Compiled::named("t", SEND);
    let handler = Arc::new(Counter::default());
    let mut machine = compiled.machine_on_backend();
    machine.set_host_binding(Arc::new(bound(&compiled, handler.clone(), true)));

    machine
        .eval_test(0)
        .into_parts()
        .0
        .expect("the handler answers");
    assert_eq!(handler.calls.load(Ordering::SeqCst), 1);
    // Even there, the value still refuses to be copied out.
    assert_eq!(handler.seen.lock().unwrap().as_slice(), [Plain::Secret]);
}

#[test]
fn an_argument_with_no_secret_reaches_the_handler_as_before() {
    let compiled = Compiled::named("t", SEND_PLAIN);
    let handler = Arc::new(Counter::default());
    let mut machine = compiled.machine_on_backend();
    machine.set_host_binding(Arc::new(bound(&compiled, handler.clone(), false)));

    machine
        .eval_test(0)
        .into_parts()
        .0
        .expect("the handler answers");
    assert_eq!(handler.calls.load(Ordering::SeqCst), 1);
}

/// A handler that quietly became able to receive credentials moves the digest CI pins.
#[test]
fn the_secrets_column_moves_the_listing_digest() {
    let compiled = Compiled::named("t", SEND);
    let listing = |secrets| {
        let mut registry = HostRegistry::new();
        registry.register(op(secrets), Arc::new(Counter::default()));
        registry.preview(&compiled.front.check).expect("resolves")
    };
    assert_ne!(listing(false).digest(), listing(true).digest());
    assert!(listing(true).rows.iter().all(|r| r.secrets));
}

#[test]
fn a_clause_may_not_answer_a_concrete_type_for_a_polymorphic_operation() {
    let source = r#"
effect vault { read fetch[k]<a>(s: Secret<String>) -> a }
fn launder(s: Secret<String>) -> String / {vault.read[k]} = vault.fetch[k](s)
test "launder" {
  handle {
    assert_eq(string_len(launder(secret_of_string("hunter2"))), 7)
  } with {
    vault.fetch[k](s) -> s,
  }
}
"#;
    let diagnostics = Compiled::rejected_in("t", source);
    assert!(
        diagnostics.iter().any(|d| d.code == codes::TYPE_MISMATCH),
        "the refusal is a type mismatch at the clause: {diagnostics:#?}"
    );
    assert!(
        diagnostics.iter().any(|d| d
            .notes
            .iter()
            .any(|n| n.contains("every type a perform site could ask for"))),
        "the diagnostic says why a clause cannot pick: {diagnostics:#?}"
    );
}

#[test]
fn a_clause_may_not_answer_the_wrong_type_for_a_polymorphic_operation() {
    let source = r#"
effect box { read take[k]<a>() -> a }
fn as_string() -> String / {box.read[k]} = box.take[k]()
test "confuse" {
  handle {
    assert_eq(string_len(as_string()), 1)
  } with {
    box.take[k]() -> 7,
  }
}
"#;
    let diagnostics = Compiled::rejected_in("t", source);
    assert!(
        diagnostics.iter().any(|d| d.code == codes::TYPE_MISMATCH),
        "{diagnostics:#?}"
    );
}

/// At the builtin, independent of whether any source program can still reach it.
#[test]
fn map_of_entries_refuses_a_secret_key() {
    let refused = ply_eval::builtins::call(
        ply_eval::Builtin::MapOfEntries,
        vec![Value::list(vec![
            entry(Value::secret(Value::str("hunter2")), Value::Int(1)),
            entry(Value::secret(Value::str("hunter1")), Value::Int(0)),
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
    let right = Value::map([(Value::secret(Value::str("hunter2")), Value::Int(1))]);
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
    let secret = Value::secret(Value::str("hunter2"));
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
