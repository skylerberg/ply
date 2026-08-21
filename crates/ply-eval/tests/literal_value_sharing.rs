//! One `Value` per *lowered literal node*, and what that may not change.
//!
//! A literal is a compile-time constant: `"abcd"` denotes the same
//! `Value::Str` however many times it is evaluated. Until ADR 0019 §2 item 1
//! the machine rebuilt it on every evaluation — 65.0 allocations per `/health`
//! request and 36.0 on the routing rung, re-taken by `cargo test -p ply-corpus
//! --release --test r4_value_construction -- --nocapture` — and
//! `ply_eval::code`'s `NodeKind::Lit` now carries the value, built once where
//! the node is lowered, which `Machine::eval` clones.
//!
//! `crates/ply-corpus/tests/r4_value_construction.rs` owns the allocation
//! count. What this file owns is that the shared value is the same value:
//!
//! - **shared per node, not per program.** Two evaluations of one occurrence
//!   answer with one `Arc`; two separate occurrences of the same spelling
//!   answer with two. The second half is what says this is node identity
//!   rather than an intern table over equal strings, and it is what bounds how
//!   much of the program a single `Arc` is reachable from.
//! - **meaning did not move.** A shared literal is equal to a fresh one, still
//!   matches a literal pattern, still keys a `Map` the same way, and renders
//!   the same bytes. `--engine both` over the corpora on disk is the
//!   mechanical form of that claim (`differential_corpus.rs`); this is the
//!   version that names the construct. The tree-walker is unchanged and both
//!   engines are run over every case here for that reason.
//! - **it holds nothing a region owns.** A value with the program's lifetime
//!   may not point at anything a region reclaims or at a credential.
//!   `interp::literal` answers a scalar, a `Str` or a `Bytes` and never a
//!   compound, a `Cell`, a `Closure` or a `Secret`, and
//!   [`no_literal_of_any_kind_evaluates_to_a_value_that_holds_anything`] is
//!   that statement rather than a reading of the source.
//! - **nothing mutates it.** The one in-place update path in the evaluator is
//!   `builtins::push` and it is `List`-only; no literal builds a `List`. A
//!   `Str` or a `Bytes` built *from* a shared literal is a fresh value, and
//!   [`building_from_a_shared_literal_does_not_disturb_it`] is what checks the
//!   shared one is unchanged afterwards.

use ply_core::{CheckOutput, check_program};
use ply_eval::{Interp, Machine, SECRET_REDACTED, Value, values_equal};
use ply_span::{SourceId, Span};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::resolve::{Resolved, resolve};

struct Compiled {
    program: Program,
    resolved: Resolved,
    check: CheckOutput,
}

fn compile(source: &str) -> Compiled {
    let inputs = [(SourceId(0), ModuleName::from_dotted("m"), source)];
    let program = ply_syntax::parse_program(inputs).expect("the fixture must parse");
    let resolved = resolve(&program).unwrap_or_else(|d| panic!("the fixture must resolve: {d:#?}"));
    let check = check_program(&program, &resolved)
        .unwrap_or_else(|d| panic!("the fixture must typecheck: {d:#?}"));
    Compiled {
        program,
        resolved,
        check,
    }
}

/// `one_str` and `one_bytes` each hold **one** literal occurrence, so calling
/// one twice evaluates one node twice; `two_strs` and `two_bytes` hold two
/// occurrences of the same spelling, so a list of two holds what two separate
/// nodes evaluated to.
const SOURCE: &str = r#"
pub fn one_str(ignored: Int) -> String = "abcd"

pub fn one_bytes(ignored: Int) -> Bytes = b"abcd"

pub fn two_strs(ignored: Int) -> List<String> = ["abcd", "abcd"]

pub fn two_bytes(ignored: Int) -> List<Bytes> = [b"abcd", b"abcd"]

pub fn matched(s: String) -> Int = match s { "abcd" -> 1, _ -> 0 }

pub fn keyed(ignored: Int) -> Map<String, Int> = map_insert(map_new(), "abcd", 7)

pub fn grown(ignored: Int) -> String = string_concat("abcd", "ef")

pub fn hidden(ignored: Int) -> Secret<String> = secret_of_string("abcd")

pub fn an_int(ignored: Int) -> Int = 7
pub fn a_bool(ignored: Int) -> Bool = true
pub fn a_float(ignored: Int) -> Float = 1.5
pub fn a_decimal(ignored: Int) -> Decimal = 1.50m
pub fn a_unit(ignored: Int) -> Unit = ()
"#;

/// Both engines run every case here, because a `--engine both` divergence is
/// what this change is not allowed to cost. The tree-walker still builds a
/// literal per evaluation, so only the assertions about *meaning* are made of
/// both; the `Arc` identity ones name their engine.
fn on_both(c: &Compiled, name: &str, arg: Value) -> [Value; 2] {
    let call = |v: Result<Value, ply_span::Diagnostic>| match v {
        Ok(value) => value,
        Err(d) => panic!("`{name}` raised: {d:#?}"),
    };
    let mut interp = Interp::new(&c.program, &c.resolved, &c.check);
    let walked = call(interp.call(name, vec![arg.clone()], Span::DUMMY));
    let mut machine = Machine::new(&c.program, &c.resolved, &c.check);
    let stepped = call(machine.call(name, vec![arg], Span::DUMMY));
    [walked, stepped]
}

/// Two calls into **one** `Machine`, which is what makes this about one lowered
/// node rather than about two programs.
fn twice_on_one_machine(c: &Compiled, name: &str) -> [Value; 2] {
    let mut machine = Machine::new(&c.program, &c.resolved, &c.check);
    let mut go = || match machine.call(name, vec![Value::Int(0)], Span::DUMMY) {
        Ok(value) => value,
        Err(d) => panic!("`{name}` raised: {d:#?}"),
    };
    let first = go();
    let second = go();
    [first, second]
}

#[track_caller]
fn pair(value: &Value) -> (&Value, &Value) {
    match value {
        Value::List(items) if items.len() == 2 => (&items[0], &items[1]),
        other => panic!("expected a list of two occurrences, found {other}"),
    }
}

#[track_caller]
fn str_ptr(v: &Value) -> *const u8 {
    match v {
        Value::Str(s) => s.as_ptr(),
        other => panic!("expected a `Str`, found {other}"),
    }
}

#[track_caller]
fn bytes_ptr(v: &Value) -> *const u8 {
    match v {
        Value::Bytes(b) => b.as_ptr(),
        other => panic!("expected a `Bytes`, found {other}"),
    }
}

#[test]
fn one_literal_node_evaluated_twice_answers_one_buffer_on_the_machine() {
    let c = compile(SOURCE);
    let [first, second] = twice_on_one_machine(&c, "m.one_str");
    assert_eq!(
        str_ptr(&first),
        str_ptr(&second),
        "one `String` literal evaluated twice answered with two buffers, so the machine is \
         rebuilding it per evaluation and the 65.0 allocations per request ADR 0019 §2 item 1 \
         took off the /health path are back"
    );
    let [first, second] = twice_on_one_machine(&c, "m.one_bytes");
    assert_eq!(
        bytes_ptr(&first),
        bytes_ptr(&second),
        "one `Bytes` literal evaluated twice answered with two buffers"
    );
}

/// The bound on how far one `Arc` reaches, and the control that the test above
/// is about node identity rather than about two equal strings being interned.
#[test]
fn two_occurrences_of_one_spelling_are_two_values() {
    let c = compile(SOURCE);
    let [_, stepped] = on_both(&c, "m.two_strs", Value::Int(0));
    let (first, second) = pair(&stepped);
    assert_ne!(
        str_ptr(first),
        str_ptr(second),
        "two separate occurrences of `\"abcd\"` share one buffer: the value is interned across \
         nodes rather than built once per node, which is a wider claim than ADR 0019 §2 item 1 \
         makes and is not what anything here has measured"
    );
    assert!(
        values_equal(first, second, Span::DUMMY).expect("two `Str`s compare"),
        "two occurrences of one spelling are not equal"
    );
}

#[test]
fn a_shared_literal_is_equal_to_a_fresh_one_on_both_engines() {
    let c = compile(SOURCE);
    let fresh = Value::str("abcd");
    for (engine, answered) in
        ["treewalk", "machine"]
            .iter()
            .zip(on_both(&c, "m.one_str", Value::Int(0)))
    {
        assert!(
            values_equal(&answered, &fresh, Span::DUMMY).expect("two `Str`s compare"),
            "on {engine} the literal is not equal to a `Str` built beside it"
        );
        assert_eq!(
            answered.render(),
            fresh.render(),
            "on {engine} the literal renders differently from a `Str` built beside it, and a \
             rendered byte is stored in `Outcome::Fail.message`"
        );
    }
    let fresh = Value::bytes(b"abcd");
    for (engine, answered) in
        ["treewalk", "machine"]
            .iter()
            .zip(on_both(&c, "m.one_bytes", Value::Int(0)))
    {
        assert!(
            values_equal(&answered, &fresh, Span::DUMMY).expect("two `Bytes` compare"),
            "on {engine} the `Bytes` literal is not equal to one built beside it"
        );
        assert_eq!(answered.render(), fresh.render(), "on {engine}");
    }
}

/// A literal pattern is matched by `interp::lit_matches` against the `Lit`, not
/// against the node's value, so this is the check that the two did not drift
/// apart: the value the node carries must still be the one its `Lit` matches.
#[test]
fn a_shared_literal_still_matches_a_literal_pattern_on_both_engines() {
    let c = compile(SOURCE);
    for (engine, answered) in
        ["treewalk", "machine"]
            .iter()
            .zip(on_both(&c, "m.matched", Value::str("abcd")))
    {
        assert_eq!(
            answered,
            Value::Int(1),
            "on {engine} a `\"abcd\"` pattern stopped matching `\"abcd\"`"
        );
    }
    for (engine, answered) in
        ["treewalk", "machine"]
            .iter()
            .zip(on_both(&c, "m.matched", Value::str("abce")))
    {
        assert_eq!(
            answered,
            Value::Int(0),
            "on {engine} a `\"abcd\"` pattern matched something else"
        );
    }
}

/// `Map`'s iteration order is [`Value`]'s `cmp` and four guarantees rest on a
/// value having one canonical form (`map_order.rs`). A shared key that ordered
/// even one step differently from a fresh one would raise nothing: it would put
/// a second entry where the program wrote one, and a lookup with the other
/// spelling would answer nothing. So the assertion is a lookup *by the other
/// value*, in both directions, rather than a comparison.
#[test]
fn a_shared_literal_keys_a_map_exactly_as_a_fresh_one_does() {
    let c = compile(SOURCE);
    for (engine, answered) in
        ["treewalk", "machine"]
            .iter()
            .zip(on_both(&c, "m.keyed", Value::Int(0)))
    {
        let map = match &answered {
            Value::Map(m) => m.clone(),
            other => panic!("on {engine} `keyed` answered {other}"),
        };
        assert_eq!(map.size(), 1, "on {engine} the map has the wrong size");
        assert_eq!(
            map.get(&Value::str("abcd")),
            Some(&Value::Int(7)),
            "on {engine} a key written as a shared literal cannot be found with a fresh `Str` of \
             the same content"
        );
        let (key, _) = map.iter().next().expect("one entry");
        let mut fresh = ply_eval::Map::new();
        fresh.insert_mut(Value::str("abcd"), Value::Int(7));
        assert_eq!(
            fresh.get(key),
            Some(&Value::Int(7)),
            "on {engine} a map keyed by a fresh `Str` cannot be looked up with the shared literal"
        );
    }
}

/// The shared buffer has the program's lifetime, so anything built from it must
/// be built rather than written into it. `string_concat` is the path, and the
/// check is that the literal node still answers the literal afterwards.
#[test]
fn building_from_a_shared_literal_does_not_disturb_it() {
    let c = compile(SOURCE);
    let mut machine = Machine::new(&c.program, &c.resolved, &c.check);
    for _ in 0..3 {
        let grown = machine
            .call("m.grown", vec![Value::Int(0)], Span::DUMMY)
            .expect("`grown` raised");
        assert_eq!(
            grown,
            Value::str("abcdef"),
            "concatenating onto a shared literal answered {grown}"
        );
    }
    let after = machine
        .call("m.one_str", vec![Value::Int(0)], Span::DUMMY)
        .expect("`one_str` raised");
    assert_eq!(
        after,
        Value::str("abcd"),
        "after three concatenations the shared literal reads {after}: something wrote through it"
    );
}

/// ADR 0015 §2 and ADR 0019 §0.1. A shared `Value` with the program's lifetime
/// is exactly how a credential would acquire one, so: no literal is a `Secret`,
/// and a `Secret` built *from* a literal still redacts and does not carry its
/// plaintext into anything rendered.
#[test]
fn a_secret_built_from_a_literal_still_redacts_and_the_literal_is_not_one() {
    let c = compile(SOURCE);
    for (engine, answered) in
        ["treewalk", "machine"]
            .iter()
            .zip(on_both(&c, "m.hidden", Value::Int(0)))
    {
        assert!(
            matches!(answered, Value::Secret(_)),
            "on {engine} `secret_of_string` answered {answered}"
        );
        let rendered = answered.render();
        assert_eq!(rendered, SECRET_REDACTED, "on {engine}");
        assert!(
            !rendered.contains("abcd"),
            "on {engine} a credential built from a literal rendered its payload: {rendered}"
        );
    }
    for (engine, answered) in
        ["treewalk", "machine"]
            .iter()
            .zip(on_both(&c, "m.one_str", Value::Int(0)))
    {
        assert!(
            !matches!(answered, Value::Secret(_)),
            "on {engine} a literal evaluated to a `Secret`"
        );
    }
}

/// Why a value held for the program's life is safe to hold: a literal never
/// denotes anything that owns a region slot, a scope, a handler stack or a
/// credential. Read off the evaluated value for every literal kind the surface
/// syntax has rather than off `interp::literal`'s source, so a new `Lit`
/// variant whose value nested would fail here.
#[test]
fn no_literal_of_any_kind_evaluates_to_a_value_that_holds_anything() {
    let c = compile(SOURCE);
    for name in [
        "m.an_int",
        "m.a_bool",
        "m.a_float",
        "m.a_decimal",
        "m.a_unit",
        "m.one_str",
        "m.one_bytes",
    ] {
        for (engine, answered) in
            ["treewalk", "machine"]
                .iter()
                .zip(on_both(&c, name, Value::Int(0)))
        {
            let ok = matches!(
                answered,
                Value::Int(_)
                    | Value::Bool(_)
                    | Value::Float(_)
                    | Value::Decimal(_)
                    | Value::Unit
                    | Value::Str(_)
                    | Value::Bytes(_)
            );
            assert!(
                ok,
                "on {engine} `{name}` evaluated to a {}, which a lowered node now holds for the \
                 whole life of the program: a literal that denotes a compound, a `Cell`, a \
                 `Closure` or a `Secret` cannot be built once at lowering",
                answered.type_name()
            );
        }
    }
}
