//! `std.random`: the entropy a run that is not simulated has.
//!
//! The prelude's `random` is the scheduler's, answered with values a seed decides; a program that
//! needs a value a server cannot predict — a SASL nonce, a key — reads this instead, and the host
//! is what draws it.

use ply_eval::{Machine, Value};
use ply_span::Span;
use std::sync::Arc;

const PROGRAM: &str = r#"
import std.random (entropy, next, below, nonce)

// Two draws are independent, so 63 bits each colliding is not a thing that happens.
pub fn differs() -> Bool / {entropy.next} = next() != next()

// A bound above zero is honoured, and a draw is the low end of the range.
pub fn in_range() -> Bool / {entropy.below} = below(4) >= 0 && below(4) < 4

pub fn named() -> String / {entropy.next} = nonce()

// A bound the host cannot honour is refused rather than folded into range.
pub fn no_range() -> Int / {entropy.below} = below(0)
"#;

fn tiered(service: &str) -> (ply_ty::Front, &'static ply_codegen::Unit) {
    let answered =
        ply_codegen::c::producer::checked_front_with_std(&[("m".to_string(), service.to_string())])
            .unwrap_or_else(|e| panic!("they check: {e:#}"));
    let front = answered.front;
    let unit = ply_codegen::Unit::over_front(&front, answered.modules.into_iter().collect())
        .expect("this host has a C compiler");
    (front, unit)
}

fn call(entry: &str) -> Result<Value, ply_span::Diagnostic> {
    let host = ply_host::Host::new();
    let (front, unit) = tiered(PROGRAM);
    let binding = host
        .registry()
        .bind(&front.check)
        .expect("the declaration and the registration agree");

    let mut machine = Machine::new(&front);
    machine.set_compiled(ply_eval::Provider::attach(unit));
    machine.set_host_binding(Arc::new(binding));
    machine.set_host_runtime(host.runtime());
    let simple = entry.rsplit('.').next().expect("an entry has a name");
    if let Some(declared) = front
        .check
        .defs
        .values()
        .find(|d| d.simple_name.as_str() == simple)
        .map(|d| d.footprint.clone())
    {
        machine.set_declared_footprint(declared);
    }
    machine.call(entry, Vec::new(), Span::DUMMY)
}

fn answered(entry: &str) -> Value {
    call(entry).unwrap_or_else(|e| panic!("`{entry}` answers: {e}"))
}

fn text(value: &Value) -> String {
    match value {
        Value::Str(s) => s.to_string(),
        other => panic!("answered {other}, not text"),
    }
}

#[test]
fn a_draw_is_the_operating_systems_and_two_of_them_are_independent() {
    assert_eq!(answered("m.differs"), Value::Bool(true));
}

#[test]
fn a_bound_above_zero_is_honoured() {
    assert_eq!(answered("m.in_range"), Value::Bool(true));
}

#[test]
fn a_nonce_names_two_draws() {
    let text = text(&answered("m.named"));
    let parts: Vec<&str> = text.split('-').collect();
    assert_eq!(parts.len(), 2, "`{text}` is not two draws");
    assert_ne!(parts[0], "", "`{text}` has an empty draw");
}

#[test]
fn a_bound_the_host_cannot_honour_is_refused() {
    let why = call("m.no_range").expect_err("a bound of zero names no range");
    assert!(
        format!("{why}").contains("above zero"),
        "the refusal does not say what is wrong: {why}"
    );
}
