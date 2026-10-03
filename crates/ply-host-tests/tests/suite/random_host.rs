//! `std.random`: the entropy a run that is not simulated has.
//!
//! The prelude's `random` is the scheduler's, answered with values a seed decides; a program that
//! needs a value a server cannot predict — a SASL nonce, a key — reads this instead, and the host
//! is what draws it.

use ply_eval::{Machine, Span, Symbol, Value};
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

fn tiered(service: &str) -> (ply_eval::Analysis, &'static ply_codegen::Unit) {
    crate::support::answered::tiered("m", service)
}
fn call(entry: &str) -> Result<Value, ply_eval::Diagnostic> {
    let host = std::sync::Arc::new(ply_host::Host::new());
    let (front, unit) = tiered(PROGRAM);
    let binding = host
        .registry()
        .bind(&front.check)
        .expect("the declaration and the registration agree");

    let mut machine = Machine::new(&front, ply_eval::Provider::attach(unit))
        .expect("the unit was compiled from this program");
    machine.set_host_binding(Arc::new(binding));
    machine.set_host_runtime({
        let host = std::sync::Arc::clone(&host);
        std::sync::Arc::new(move || host.runtime())
    });
    let declared = front
        .check
        .defs
        .get(&Symbol::new(entry))
        .expect("the entry is a definition of the program");
    machine.set_declared_footprint(declared.footprint.clone());
    machine.call(entry, Vec::new(), Span::DUMMY).into_parts().0
}

fn answered(entry: &str) -> Value {
    call(entry).unwrap_or_else(|e| panic!("`{entry}` answers: {e}"))
}

fn text(value: &Value) -> String {
    match value {
        Value::Str(s) => s.to_string(),
        other => panic!("answered {other:?}, not text"),
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
