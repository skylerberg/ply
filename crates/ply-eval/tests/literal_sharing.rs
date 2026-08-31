//! A literal is a compile-time constant, and its `Value` is built once.
//!
//! `Lit::Str` and `Lit::Bytes` are the only two literals that ever reached the
//! allocator — `Int`, `Bool`, `Float`, `Decimal` and `Unit` are inline variants
//! of `Value` — and they reached it on **every evaluation**, because
//! `NodeKind::Lit` carried only the `Lit` and the machine called
//! `interp::literal` per step. ADR 0019 §2 is that node carrying the `Value`
//! too, built at lowering; the machine clones it, and an `Arc` clone is a
//! refcount bump.
//!
//! Two claims have to hold and they pull in opposite directions:
//!
//! - the sharing is **real** — a second evaluation of one literal hands back
//!   the same `Arc`, and a loop over one costs the allocator nothing per
//!   iteration;
//! - the sharing is **unobservable** — no Ply expression reads an address, so
//!   a shared `Str` has to compare, order, render and match exactly as a freshly
//!   built one does. This is `Value::builtin`'s argument (`value.rs:200`)
//!   applied to a second kind of constant, and it is checked here rather than
//!   asserted.
//!
//! Its own binary with a `#[global_allocator]`, for the reason
//! `lowering_sharing.rs` has one: a counter on every allocation in `ply-eval`'s
//! unit tests would perturb every other number the crate takes.
//!
//! What this does **not** claim is a figure for a served request. That is
//! `crates/ply-corpus/tests/r4_value_construction.rs`, which fits a slope over
//! two request windows so that per-`Machine` setup cannot masquerade as
//! per-request work.

use ply_core::{CheckOutput, check_program};
use ply_eval::{Machine, Value};
use ply_span::{SourceId, Span};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::resolve::{Resolved, resolve};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Arc;

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
}

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if ARMED.try_with(Cell::get).unwrap_or(false) {
            let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn counted<T>(f: impl FnOnce() -> T) -> (T, usize) {
    ALLOCS.with(|c| c.set(0));
    ARMED.with(|c| c.set(true));
    let out = f();
    ARMED.with(|c| c.set(false));
    (out, ALLOCS.with(Cell::get))
}

/// The three loops are the same shape down to the node — one `if`, one
/// comparison, one self-call — and differ in **the kind of the two literals
/// compared**. `Int` literals are inline `Value` variants and never touched the
/// allocator; `Str` and `Bytes` literals did, once per evaluation. So the
/// difference between the `Int` loop's slope and either of the others is the
/// literal construction and nothing else.
///
/// The occurrence under test sits in a comparison rather than in a call, and
/// deliberately: a control that differs by a call differs by that call's
/// argument vector too, which is `size_of::<Value>()` per argument and is a
/// larger line than the literal. The self-call the three loops share cancels;
/// an unmatched one would not.
const SOURCE: &str = r#"
fn loop_int(n: Int, acc: Int) -> Int =
  if n <= 0 { acc } else { loop_int(n - 1, if 11 == 11 { acc + 1 } else { acc }) }

fn loop_str(n: Int, acc: Int) -> Int =
  if n <= 0 { acc } else { loop_str(n - 1, if "abcd" == "abcd" { acc + 1 } else { acc }) }

fn loop_bytes(n: Int, acc: Int) -> Int =
  if n <= 0 { acc } else { loop_bytes(n - 1, if b"abcd" == b"abcd" { acc + 1 } else { acc }) }

fn text(k: Int) -> String = "abcd"
fn raw(k: Int) -> Bytes = b"abcd"
fn computed(k: Int) -> String = string_concat("ab", "cd")
fn ints(k: Int) -> Int = 4
fn flag(k: Int) -> Bool = true
fn nothing(k: Int) -> Unit = ()
"#;

/// Every fixture below takes an argument it ignores, and that is load-bearing:
/// `Machine::constant` memoizes a **nullary** pure definition, so a nullary
/// `fn text() -> String = "abcd"` hands back one `Value` on every call whether
/// or not the literal itself is shared. A test written against one would pass
/// before this change and prove nothing.
fn once(machine: &mut Machine<'_>, name: &str) -> Value {
    call(machine, name, vec![Value::Int(0)])
}

/// Small enough to run in a test and large enough that one allocation per
/// iteration is unmistakable against the frame pool settling.
const SMALL: usize = 100;
const LARGE: usize = 1000;

struct Compiled {
    program: Program,
    resolved: Resolved,
    check: CheckOutput,
}

impl Compiled {
    fn new(src: &str) -> Compiled {
        let inputs = [(SourceId(0), ModuleName::from_dotted("m"), src)];
        let mut program = ply_syntax::parse_program(inputs).expect("the fixture must parse");
        let resolved =
            resolve(&mut program).unwrap_or_else(|d| panic!("the fixture must resolve: {d:#?}"));
        let check = check_program(&program, &resolved)
            .unwrap_or_else(|d| panic!("the fixture must typecheck: {d:#?}"));
        Compiled {
            program,
            resolved,
            check,
        }
    }

    fn machine(&self) -> Machine<'_> {
        Machine::new(&self.program, &self.resolved, &self.check)
    }
}

fn call(machine: &mut Machine<'_>, name: &str, args: Vec<Value>) -> Value {
    machine
        .call(&format!("m.{name}"), args, Span::DUMMY)
        .unwrap_or_else(|d| panic!("`m.{name}` raised: {d:#?}"))
}

/// Allocations the loop body costs per iteration, on a machine whose lowering
/// and frame pool are already warm — so what is measured is evaluation and not
/// the one-time construction this change moves the work *into*.
fn per_iteration(compiled: &Compiled, name: &str) -> f64 {
    let mut machine = compiled.machine();
    let at = |machine: &mut Machine<'_>, n: usize| {
        call(machine, name, vec![Value::Int(n as i64), Value::Int(0)])
    };
    // To the deepest recursion the measurement will reach, and twice, because
    // `crate::pool`'s free list of frame links is **thread-local**: a loop run
    // after a deeper one reuses its links and reads near zero whatever it does.
    // That is order dependence, not a saving, and it is what makes a shallow
    // warm-up useless here.
    at(&mut machine, LARGE);
    at(&mut machine, LARGE);
    let (_, small) = counted(|| at(&mut machine, SMALL));
    let (_, large) = counted(|| at(&mut machine, LARGE));
    println!("   {name}: {small} allocations at {SMALL} iterations, {large} at {LARGE}");
    (large as f64 - small as f64) / (LARGE - SMALL) as f64
}

/// The number this change exists to move: a `Str` literal in a loop body used to
/// add exactly one allocation per iteration and now adds none.
#[test]
fn a_string_literal_in_a_loop_costs_the_allocator_nothing_per_iteration() {
    let compiled = Compiled::new(SOURCE);
    let base = per_iteration(&compiled, "loop_int");
    let with_literal = per_iteration(&compiled, "loop_str");
    let added = with_literal - base;
    println!(
        "two `Str` literals in place of two `Int` literals add {added:+.3} allocations per \
         iteration ({with_literal:.3} against the `Int` loop's {base:.3})"
    );
    assert!(
        added.abs() < 0.05,
        "two `Str` literals added {added:+.3} allocations per iteration: a literal is being \
         rebuilt per evaluation, so `NodeKind::Lit`'s `Value` is not what the machine returns"
    );
}

/// The same for `Bytes`, which mirrors `Str` deliberately and would be the half
/// a change to one of them forgot.
#[test]
fn a_bytes_literal_in_a_loop_costs_the_allocator_nothing_per_iteration() {
    let compiled = Compiled::new(SOURCE);
    let base = per_iteration(&compiled, "loop_int");
    let with_literal = per_iteration(&compiled, "loop_bytes");
    let added = with_literal - base;
    println!(
        "two `Bytes` literals in place of two `Int` literals add {added:+.3} allocations per \
         iteration ({with_literal:.3} against the `Int` loop's {base:.3})"
    );
    assert!(
        added.abs() < 0.05,
        "two `Bytes` literals added {added:+.3} allocations per iteration: a literal is being \
         rebuilt per evaluation"
    );
}

fn str_arc(v: &Value) -> Arc<str> {
    match v {
        Value::Str(s) => Arc::clone(s),
        other => panic!("expected a `Str`, got {other:?}"),
    }
}

fn bytes_arc(v: &Value) -> Arc<[u8]> {
    match v {
        Value::Bytes(b) => Arc::clone(b),
        other => panic!("expected `Bytes`, got {other:?}"),
    }
}

/// The mechanism, stated so that a regression that merely *costs* less without
/// sharing — a smaller allocation, say — cannot pass the two slope tests above
/// while the constant is still rebuilt.
#[test]
fn two_evaluations_of_one_literal_hand_back_one_arc() {
    let compiled = Compiled::new(SOURCE);
    let mut machine = compiled.machine();

    let first = str_arc(&once(&mut machine, "text"));
    let second = str_arc(&once(&mut machine, "text"));
    assert!(
        Arc::ptr_eq(&first, &second),
        "two evaluations of one `Str` literal handed back two different `Arc`s"
    );

    let first = bytes_arc(&once(&mut machine, "raw"));
    let second = bytes_arc(&once(&mut machine, "raw"));
    assert!(
        Arc::ptr_eq(&first, &second),
        "two evaluations of one `Bytes` literal handed back two different `Arc`s"
    );
}

/// Sharing is per lowering, and a second machine that lowered the program for
/// itself holds its own constants. Nothing depends on that — it is recorded so
/// that a reader who finds two `Arc`s for one literal knows which case they are
/// in before calling it a defect.
#[test]
fn a_second_machine_with_its_own_lowering_holds_its_own_copy_and_answers_the_same() {
    let compiled = Compiled::new(SOURCE);
    let mut first = compiled.machine();
    let mut second = compiled.machine();
    let a = once(&mut first, "text");
    let b = once(&mut second, "text");
    assert!(
        !Arc::ptr_eq(&str_arc(&a), &str_arc(&b)),
        "two machines that each lowered the program share one literal `Arc`, so the constant \
         outlives the lowering that built it"
    );
    assert_eq!(
        a.render(),
        b.render(),
        "two machines disagreed about what one literal is"
    );
    assert!(
        ply_eval::values_equal(&a, &b, Span::DUMMY).expect("two strings compare"),
        "a literal from one machine is not equal to the same literal from another"
    );
}

/// The half that matters more than the saving: a shared constant has to be
/// indistinguishable from a rebuilt one at every place meaning is decided —
/// equality, ordering, rendering and the pattern match that reads it back.
#[test]
fn a_shared_literal_is_indistinguishable_from_a_freshly_built_value() {
    let compiled = Compiled::new(SOURCE);
    let mut machine = compiled.machine();

    let shared = once(&mut machine, "text");
    let fresh = Value::str("abcd");
    // Built by `string_concat` at run time, so it is a third `Arc` with the same
    // bytes and no relationship to either.
    let computed = once(&mut machine, "computed");

    for (label, other) in [
        ("a freshly built Value", &fresh),
        ("a computed String", &computed),
    ] {
        assert!(
            ply_eval::values_equal(&shared, other, Span::DUMMY).expect("two strings compare"),
            "the shared literal is not equal to {label}"
        );
        assert_eq!(
            shared.cmp(other),
            std::cmp::Ordering::Equal,
            "the shared literal does not order equal to {label}, so it would be a second `Map` key"
        );
        assert_eq!(
            shared.render(),
            other.render(),
            "the shared literal renders differently from {label}, and a rendered value is stored \
             in `Outcome::Fail.message`"
        );
    }

    let shared_bytes = once(&mut machine, "raw");
    let fresh_bytes = Value::bytes(b"abcd");
    assert!(
        ply_eval::values_equal(&shared_bytes, &fresh_bytes, Span::DUMMY)
            .expect("two byte strings compare"),
        "the shared `Bytes` literal is not equal to a freshly built one"
    );
    assert!(
        !ply_eval::values_equal(&shared_bytes, &shared, Span::DUMMY)
            .expect("a `Bytes` and a `Str` compare"),
        "sharing made a `Bytes` literal equal to a `Str` literal with the same bytes"
    );
}

/// Every literal kind still denotes what it denoted, including the five that
/// never allocated and are now carried beside the `Lit` anyway. `Secret` is not
/// among them and cannot be: there is no secret literal, and an interned
/// credential would have program lifetime (ADR 0019 §0.1).
#[test]
fn every_literal_kind_denotes_what_it_did_and_none_of_them_is_a_secret() {
    let compiled = Compiled::new(SOURCE);
    let mut machine = compiled.machine();
    for (name, expected, type_name) in [
        ("text", "\"abcd\"", "String"),
        ("raw", "b\"abcd\"", "Bytes"),
        ("ints", "4", "Int"),
        ("flag", "true", "Bool"),
        ("nothing", "()", "Unit"),
    ] {
        let value = once(&mut machine, name);
        assert_eq!(
            value.render(),
            expected,
            "`m.{name}` rendered differently after its literal was hoisted"
        );
        assert_eq!(
            value.type_name(),
            type_name,
            "`m.{name}` changed type after its literal was hoisted"
        );
    }
}
