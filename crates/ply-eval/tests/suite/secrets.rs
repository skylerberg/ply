//! ADR 0015 §2 at the evaluator: what a credential renders as, what comparing
//! two of them does, and what happens when one reaches the host boundary.
//!
//! `ply-core/tests/suite/secrets.rs` covers the routes the type checker closes. These
//! are the two that no type can close, because they take *any* value —
//! `Value::render` and `values_equal` — plus the one that leaves the language
//! entirely.

// A `Value::Record` holds `Arc<BTreeMap<Symbol, Value>>` and a `Value` is not
// `Send`; that is `ply-eval`'s design and this is the same allow, for the same
// reason, that `ply-host` carries.
#![allow(clippy::arc_with_non_send_sync)]

use ply_core::{CheckOutput, check_program};
use ply_eval::host::{
    Determinism, HostAnswer, HostBinding, HostHandler, HostOp, HostRegistry, HostRequest,
    HostResource, HostRuntime, Linearity,
};
use ply_eval::{Machine, SECRET_REDACTED, Value, constant_time_eq, values_equal};
use ply_span::{Diagnostic, SourceId, Symbol, codes};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::resolve::{Resolved, resolve};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

struct Compiled {
    program: Program,
    resolved: Resolved,
    check: CheckOutput,
}

fn compile(source: &str) -> Compiled {
    let mut program =
        ply_syntax::parse_program(vec![(SourceId(0), ModuleName::from_dotted("t"), source)])
            .expect("the fixture parses");
    let resolved = resolve(&mut program).expect("the fixture resolves");
    let check = check_program(&program, &resolved).expect("the fixture typechecks");
    Compiled {
        program,
        resolved,
        check,
    }
}

impl Compiled {
    fn machine(&self) -> Machine<'_> {
        Machine::new(&self.program, &self.resolved, &self.check)
    }
}

/// Runs test 0 and answers its diagnostic, if any.
fn run(source: &str) -> Result<(), Diagnostic> {
    let compiled = compile(source);
    let mut machine = compiled.machine();
    machine.eval_test(0)
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

// --- rendering --------------------------------------------------------------

/// The line that closes the assertion diff, the panic payload, `ply run`'s
/// result line, M5's failure JSON, `--json`, the result cache and every
/// `Diagnostic` that interpolates a value. It has no payload-dependent branch,
/// so there is nothing for a later change to get wrong per payload.
#[test]
fn a_secret_renders_redacted_whatever_it_holds() {
    for payload in [
        Value::str("hunter2"),
        Value::str(""),
        Value::bytes(b"\x00\xff"),
        Value::Int(1),
        Value::list(vec![Value::str("a"), Value::str("b")]),
    ] {
        let secret = Value::secret(payload);
        assert_eq!(secret.render(), SECRET_REDACTED);
        assert_eq!(format!("{secret}"), SECRET_REDACTED);
        assert_eq!(format!("{secret:?}"), SECRET_REDACTED);
        assert!(!secret.render().contains("hunter2"));
    }
}

/// Nesting does not reach it either: a variant, a list and a map all render
/// through the same walk, and the redaction is the leaf rather than a guard the
/// walk applies at the top.
#[test]
fn a_nested_secret_renders_redacted() {
    let inner = Value::secret(Value::str("hunter2"));
    let rendered = Value::list(vec![
        Value::ctor("Some", vec![inner.clone()]),
        Value::map([(Value::str("password"), inner.clone())]),
        inner,
    ])
    .render();
    assert!(!rendered.contains("hunter2"), "{rendered}");
    assert_eq!(rendered.matches(SECRET_REDACTED).count(), 3, "{rendered}");
}

/// A failing `assert_eq` over two records holding credentials prints the diff
/// and prints neither credential. This is the route that leaks in every other
/// language's test runner.
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
    assert!(text.contains(SECRET_REDACTED), "{text}");
}

/// `type_error` interpolates the offending value, so a builtin handed a
/// `Secret` is a diagnostic that names the type and prints nothing else.
#[test]
fn a_runtime_type_error_over_a_secret_prints_no_payload() {
    let d = values_equal(
        &Value::secret(Value::str("hunter2")),
        &Value::builtin(ply_eval::Builtin::Len),
        ply_span::Span::DUMMY,
    )
    .expect_err("a function has no equality");
    let text = format!("{d:#?}");
    assert!(!text.contains("hunter2"), "{text}");
}

// --- equality ---------------------------------------------------------------

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

/// A `Secret` is never equal to a non-`Secret`. Checked at the Rust level
/// because the type checker refuses to write the comparison in Ply, which is
/// the point — but the evaluator must not be the reason it holds.
#[test]
fn a_secret_is_never_equal_to_its_payload() {
    let span = ply_span::Span::DUMMY;
    let secret = Value::secret(Value::str("hunter2"));
    let plain = Value::str("hunter2");
    assert!(!values_equal(&secret, &plain, span).unwrap());
    assert!(!values_equal(&plain, &secret, span).unwrap());
    assert!(values_equal(&secret, &secret.clone(), span).unwrap());
}

/// The comparison scans both operands to their full length whatever the answer,
/// so the *comparison* is not the oracle. What a caller does with the `Bool` is
/// (ADR 0015 §2.5 (4)), and nothing here claims otherwise.
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

// --- ordering ---------------------------------------------------------------

/// The runtime backstop under `derivable(ord, ·)`. A well-typed program cannot
/// arrive here — `compare` and `compare_values` both carry the constraint — so
/// this is what a defect in either walk meets instead of an ordering oracle.
#[test]
fn compare_values_refuses_a_secret_at_run_time() {
    let mut regions = ply_eval::TaskRegions::new();
    let d = ply_eval::builtins::call(
        ply_eval::Builtin::CompareValues,
        vec![
            Value::secret(Value::str("a")),
            Value::secret(Value::str("b")),
        ],
        &mut regions,
        ply_span::Span::DUMMY,
    )
    .expect_err("a credential has no order");
    assert_eq!(d.code, codes::RUNTIME_ERROR);
    assert!(
        d.notes.iter().any(|n| n.contains("secret_verify")),
        "{d:#?}"
    );
    assert!(!format!("{d:#?}").contains("\"a\""), "{d:#?}");
}

/// And the same for the map operations, which are the other consumer of the
/// order. `Map<Secret<a>, v>` is `E0206`, so this too is a backstop.
#[test]
fn a_secret_key_is_refused_by_every_map_operation_that_takes_one() {
    let mut regions = ply_eval::TaskRegions::new();
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
        let d = ply_eval::builtins::call(builtin, args, &mut regions, ply_span::Span::DUMMY)
            .err()
            .unwrap_or_else(|| panic!("{} accepted a Secret key", builtin.name()));
        assert_eq!(d.code, codes::RUNTIME_ERROR, "{}", builtin.name());
    }
}

// --- the builtins -----------------------------------------------------------

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

/// §2.5 (2), stated as a test rather than only as prose: the plaintext the
/// secret was built from is still in scope and is still a `String`. Containment
/// starts where the `Secret` starts, and pretending otherwise would be the
/// unstated hole the ADR refuses to leave.
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

// --- the host boundary ------------------------------------------------------

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
    seen: std::sync::Mutex<Vec<String>>,
}

impl HostHandler for Counter {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.seen
            .lock()
            .unwrap()
            .extend(req.args.iter().map(Value::render));
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
    registry.bind(&compiled.check).expect("binds")
}

/// The tripwire. No W5 operation declares `secrets: true`, so this is the
/// answer every host operation gives today — and the handler is never entered,
/// which is the whole point: the credential has not crossed when this fires.
#[test]
fn a_secret_reaching_a_handler_that_does_not_declare_one_is_e0439() {
    let compiled = compile(SEND);
    let handler = Arc::new(Counter::default());
    let mut machine = compiled.machine();
    machine.set_host_binding(Arc::new(bound(&compiled, handler.clone(), false)));

    let d = machine.eval_test(0).expect_err("E0439");
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

/// A credential is almost never the whole argument — it is a field of the
/// record a request is built from — so the check is a walk rather than a
/// top-level test.
#[test]
fn a_secret_nested_in_an_argument_is_found() {
    let compiled = compile(SEND_NESTED);
    let handler = Arc::new(Counter::default());
    let mut machine = compiled.machine();
    machine.set_host_binding(Arc::new(bound(&compiled, handler.clone(), false)));

    let d = machine.eval_test(0).expect_err("E0439");
    assert_eq!(d.code, codes::SECRET_TO_HOST);
    assert_eq!(handler.calls.load(Ordering::SeqCst), 0);
}

/// The check is a gate rather than a ban: an operation that declares it may
/// receive a credential does, and becomes a reviewed member of the trusted
/// computing base. Nothing in W5 declares it, so this fixture is the only
/// caller.
#[test]
fn an_operation_that_declares_secrets_receives_one() {
    let compiled = compile(SEND);
    let handler = Arc::new(Counter::default());
    let mut machine = compiled.machine();
    machine.set_host_binding(Arc::new(bound(&compiled, handler.clone(), true)));

    machine.eval_test(0).expect("the handler answers");
    assert_eq!(handler.calls.load(Ordering::SeqCst), 1);
    // Even there, what the handler can *render* is redacted: the boundary hands
    // over the value, and the value still refuses to print itself.
    assert_eq!(handler.seen.lock().unwrap().as_slice(), [SECRET_REDACTED]);
}

/// An ordinary operation is untouched. A check that made every host operation
/// pay a walk over a `Secret`-free argument would be a cost with no user.
#[test]
fn an_argument_with_no_secret_reaches_the_handler_as_before() {
    let compiled = compile(SEND_PLAIN);
    let handler = Arc::new(Counter::default());
    let mut machine = compiled.machine();
    machine.set_host_binding(Arc::new(bound(&compiled, handler.clone(), false)));

    machine.eval_test(0).expect("the handler answers");
    assert_eq!(handler.calls.load(Ordering::SeqCst), 1);
}

/// The listing's digest covers the column, so a handler that quietly became
/// able to receive credentials moves the one line CI pins.
#[test]
fn the_secrets_column_moves_the_listing_digest() {
    let compiled = compile(SEND);
    let listing = |secrets| {
        let mut registry = HostRegistry::new();
        registry.register(op(secrets), Arc::new(Counter::default()));
        registry.preview(&compiled.check).expect("resolves")
    };
    assert_ne!(listing(false).digest(), listing(true).digest());
    assert!(listing(true).rows.iter().all(|r| r.secrets));
}

// --- the ordering-oracle backstop, and the hole it was reached through ------
//
// ADR 0015 §2.2 installs a runtime backstop as "what a defect in either walk
// meets instead of an ordering oracle over a credential". A W5 audit found it
// guarded `compare`, `compare_values`, `map_insert`, `map_get`, `map_contains`
// and `map_remove` but NOT `map_of_entries` or `map_merge`, both of which order
// their keys through `Value::cmp` — which, for two `Secret`s, compared the
// payloads. That was a total ordering oracle over a plaintext, recovering it in
// comparisons proportional to its length.
//
// Two things were wrong and both are fixed here. The backstop now lives under
// `crate::map`'s single key gate rather than at six call sites, so there is one
// place a key reaches `Value::cmp` from; and the vehicle that reached it — an
// effect operation with a polymorphic return, whose handler clause was checked
// against a *fresh* instantiation never unified with the perform site's — is a
// type error, because a clause is now checked with the operation's own type
// variables rigid.

/// The vehicle, closed at the type checker. An operation declared `-> a` is
/// answered by one clause for every perform site there will ever be, so the
/// clause may not choose the type; `vault.fetch[k](s) -> s` answering a
/// `Secret<String>` where the caller read a `String` is `E0201`.
///
/// This is general type soundness rather than anything about secrets — a clause
/// answering a `String` for an `-> Int` caller was accepted the same way — but
/// it is the route §2.3's "Map key ordering" row was reached through, so it is
/// pinned here beside the leak it enabled.
#[test]
fn a_clause_may_not_answer_a_concrete_type_for_a_polymorphic_operation() {
    let source = r#"
effect vault { read fetch[k](s: Secret<String>) -> a }
fn launder(s: Secret<String>) -> String / {vault.read[k]} = vault.fetch[k](s)
test "launder" {
  handle {
    assert_eq(string_len(launder(secret_of_string("hunter2"))), 7)
  } with {
    vault.fetch[k](s) -> s,
  }
}
"#;
    let mut program =
        ply_syntax::parse_program(vec![(SourceId(0), ModuleName::from_dotted("t"), source)])
            .expect("the fixture parses");
    let resolved = resolve(&mut program).expect("the fixture resolves");
    let diagnostics = check_program(&program, &resolved)
        .expect_err("a clause that laundered a `Secret` into a `String` is refused");
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

/// The same hole with no `Secret` in sight, so that closing it is recorded as
/// what it is: a clause answering an `Int` where the caller unified `String`
/// was accepted and failed at run time with `E0502`.
#[test]
fn a_clause_may_not_answer_the_wrong_type_for_a_polymorphic_operation() {
    let source = r#"
effect box { read take[k]() -> a }
fn as_string() -> String / {box.read[k]} = box.take[k]()
test "confuse" {
  handle {
    assert_eq(string_len(as_string()), 1)
  } with {
    box.take[k]() -> 7,
  }
}
"#;
    let mut program =
        ply_syntax::parse_program(vec![(SourceId(0), ModuleName::from_dotted("t"), source)])
            .expect("the fixture parses");
    let resolved = resolve(&mut program).expect("the fixture resolves");
    let diagnostics = check_program(&program, &resolved)
        .expect_err("a clause answering an `Int` for a `String` caller is refused");
    assert!(
        diagnostics.iter().any(|d| d.code == codes::TYPE_MISMATCH),
        "{diagnostics:#?}"
    );
}

/// The builtin-level check that the gap itself is gone, independent of whether
/// any source program can still reach it. `map_of_entries` handed two `Secret`
/// keys refuses rather than building a map ordered by their plaintexts.
///
/// At this level because there is no longer a well-typed program that reaches
/// it: a `Secret` typed honestly as a map key is `E0206`, and the effect route
/// that laundered one is now `E0201`. A backstop is exactly the thing that has
/// to hold when both of those have been defeated, so it is tested where a defect
/// in either would arrive.
#[test]
fn map_of_entries_refuses_a_secret_key() {
    let mut regions = ply_eval::TaskRegions::new();
    let refused = ply_eval::builtins::call(
        ply_eval::Builtin::MapOfEntries,
        vec![Value::list(vec![
            entry(Value::secret(Value::str("hunter2")), Value::Int(1)),
            entry(Value::secret(Value::str("hunter1")), Value::Int(0)),
        ])],
        &mut regions,
        ply_span::Span::DUMMY,
    );
    let d = refused.expect_err("`map_of_entries` refuses a `Secret` key");
    assert_eq!(d.code, codes::RUNTIME_ERROR, "{d:#?}");
    assert!(
        d.message.contains("cannot order a `Secret`"),
        "the backstop names the credential: {}",
        d.message
    );
}

/// `map_merge` shares the gate, because it inserts the right map's keys into
/// the left through the same one place a key enters a `Map`.
#[test]
fn map_merge_refuses_a_secret_key() {
    let mut regions = ply_eval::TaskRegions::new();
    // No map builtin will build the right-hand side, so it is assembled
    // directly: the gate is what `merge` has to apply, and this is the value a
    // defect elsewhere would hand it.
    let right = Value::map([(Value::secret(Value::str("hunter2")), Value::Int(1))]);
    let refused = ply_eval::builtins::call(
        ply_eval::Builtin::MapMerge,
        vec![Value::empty_map(), right],
        &mut regions,
        ply_span::Span::DUMMY,
    );
    let d = refused.expect_err("`map_merge` refuses a `Secret` key");
    assert_eq!(d.code, codes::RUNTIME_ERROR, "{d:#?}");
    assert!(d.message.contains("cannot order a `Secret`"), "{d:#?}");
}

/// Every map builtin that touches a key, refused for the same reason and by the
/// same gate. Written as a table so a seventh builder is a row rather than a
/// forgotten call site — which is what the two missing ones were.
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
    ];
    for (builtin, args) in cases {
        let mut regions = ply_eval::TaskRegions::new();
        let refused = ply_eval::builtins::call(builtin, args, &mut regions, ply_span::Span::DUMMY);
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
}

/// A `{key, value}` record, as `map_of_entries` reads one.
fn entry(k: Value, v: Value) -> Value {
    use std::collections::BTreeMap;
    let mut fields = BTreeMap::new();
    fields.insert(Symbol::new("key"), k);
    fields.insert(Symbol::new("value"), v);
    Value::Record(Arc::new(fields))
}
