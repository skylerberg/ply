//! What a **stale** entry in either of R3's two caches can do to a program.

use ply_core::{CheckOutput, check_program};
use ply_eval::region_kind::Cause;
use ply_eval::{Machine, RegionKind, Value};
use ply_span::{SourceId, Span};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::resolve::{Resolved, resolve};

struct Compiled {
    program: Program,
    resolved: Resolved,
    check: CheckOutput,
}

impl Compiled {
    #[track_caller]
    fn new(src: &str) -> Compiled {
        let inputs = [(SourceId(0), ModuleName::from_dotted("m"), src)];
        let mut program = ply_syntax::parse_program(inputs)
            .unwrap_or_else(|d| panic!("the fixture must parse: {d:#?}\n{src}"));
        let resolved = resolve(&mut program)
            .unwrap_or_else(|d| panic!("the fixture must resolve: {d:#?}\n{src}"));
        let check = check_program(&program, &resolved)
            .unwrap_or_else(|d| panic!("the fixture must typecheck: {d:#?}\n{src}"));
        Compiled {
            program,
            resolved,
            check,
        }
    }

    fn machine(&self) -> Machine<'_> {
        Machine::new(&self.program, &self.resolved, &self.check)
    }

    #[track_caller]
    fn call(&self, name: &str) -> Value {
        let mut machine = self.machine();
        machine
            .call(name, Vec::new(), Span::DUMMY)
            .unwrap_or_else(|d| panic!("[{}] {}", d.code, d.message))
    }
}

#[track_caller]
fn int(value: Value) -> i64 {
    match value {
        Value::Int(i) => i,
        other => panic!("expected an Int, got {other:?}"),
    }
}

/// The two fixtures below differ only inside the region's body and are padded to the same length,
/// so `with_cell[r]` occupies **one span** in both.
const PRELUDE: &str =
    "effect amb { read flip[coin]() -> Bool }\n\nfn go() -> Int =\n  with_cell[r](0) { c -> ";

/// Two resumptions over one cell — ADR 0017 §3's deciding example, with the trace cell folded into
/// the answer so one integer carries both.
const CAPTURING: &str = "{ let total = handle { let b = amb.flip[coin](); cell_set(c, cell_get(c) + 1); if b { cell_get(c) } else { cell_get(c) * 10 } } with { amb.flip[coin]() resume k -> k(true) + k(false), return x -> x }; total + cell_get(c) * 1000 }";

/// The same region with no capture in it at all, so the analysis infers `unique`.
const PURE: &str = "{ cell_set(c, cell_get(c) + 1); 21 + cell_get(c) * 1000 }";

fn fixture(body: &str) -> String {
    let pad = " ".repeat(CAPTURING.len().saturating_sub(body.len()));
    format!("{PRELUDE}{body}{pad} }}\n")
}

/// The same pair for a tail-resumptive clause.
const TAIL_PRELUDE: &str = "effect log { write note[tape](n: Int) -> Int }\n\nfn go() -> Int =\n  with_cell[tape](0) { c -> ";

const TAIL_CAPTURING: &str = "{ let total = handle { log.note[tape](1) + log.note[tape](2) } with { log.note[tape](n) -> { cell_set(c, cell_get(c) * 10 + n); n } }; total + cell_get(c) * 1000 }";

const TAIL_PURE: &str = "{ cell_set(c, 12); 3 + cell_get(c) * 1000 }";

fn tail_fixture(body: &str) -> String {
    let pad = " ".repeat(TAIL_CAPTURING.len().saturating_sub(body.len()));
    format!("{TAIL_PRELUDE}{body}{pad} }}\n")
}

/// **The premature-free hunt, and it comes back empty.**
#[test]
fn a_region_kind_from_the_wrong_program_cannot_free_a_region_a_continuation_reaches() {
    let pure = Compiled::new(&fixture(PURE));
    let capturing = Compiled::new(&fixture(CAPTURING));

    let filler = pure.machine();
    let span = {
        let honest = capturing.machine();
        let regions: Vec<_> = honest.region_kinds().iter().map(|r| r.span).collect();
        assert_eq!(regions.len(), 1, "the fixture must open exactly one region");
        regions[0]
    };
    assert_eq!(
        filler.region_kind(span),
        Some(RegionKind::Unique),
        "the two fixtures no longer place their region at one span, so this poisons nothing"
    );
    assert_eq!(
        capturing.machine().region_kind(span),
        Some(RegionKind::Shared),
        "the capturing fixture stopped capturing, so there is nothing to get wrong"
    );

    assert_eq!(int(capturing.call("m.go")), 2021, "the honest answer moved");

    let mut poisoned = capturing.machine();
    poisoned.share_region_kinds(filler.shared_region_kinds());
    assert_eq!(
        poisoned.region_kind(span),
        Some(RegionKind::Unique),
        "the poisoned machine did not read the wrong analysis, so nothing was tested"
    );
    let answered = poisoned
        .call("m.go", Vec::new(), Span::DUMMY)
        .unwrap_or_else(|d| panic!("a stale `unique` broke the run: [{}] {}", d.code, d.message));
    assert_eq!(
        int(answered),
        2021,
        "a region the analysis called `unique` was reclaimed at its close while a continuation \
         could still reach it"
    );
}

/// The same question on a tail-resumptive region, which takes no pin.
///
/// The staleness arm is vacuous since ADR 0033 §8 — `unique` is now the honest inference for this
/// shape, so injecting it injects the honest answer — and is kept as a regression guard: it reddens
/// at the `Unique` assertion if the clause form goes back to forcing `shared`.
#[test]
fn a_tail_resumptive_region_is_unique_and_a_stale_kind_does_not_move_it() {
    let pure = Compiled::new(&tail_fixture(TAIL_PURE));
    let tail = Compiled::new(&tail_fixture(TAIL_CAPTURING));

    let filler = pure.machine();
    let span = tail.machine().region_kinds().iter().next().unwrap().span;
    assert_eq!(
        filler.region_kind(span),
        Some(RegionKind::Unique),
        "the two fixtures no longer place their region at one span"
    );
    assert_eq!(
        tail.machine().region_kind(span),
        Some(RegionKind::Unique),
        "ADR 0033 §8: a tail-resumptive clause is not a capture that outlives its region"
    );

    let honest = int(tail.call("m.go"));
    assert_eq!(honest, 12003, "the honest answer moved");

    let mut machine = tail.machine();
    machine.share_region_kinds(filler.shared_region_kinds());
    assert_eq!(machine.region_kind(span), Some(RegionKind::Unique));
    let on_machine = machine
        .call("m.go", Vec::new(), Span::DUMMY)
        .unwrap_or_else(|d| {
            panic!(
                "a stale `unique` broke the machine: [{}] {}",
                d.code, d.message
            )
        });

    assert_eq!(
        int(on_machine),
        honest,
        "a tail-resumptive region the analysis called `unique` answered differently"
    );
}

/// The ordinary shape of staleness: an edit moves every span after it, so a handle filled before
/// the edit answers about spans the running program does not have.
#[test]
fn an_analysis_whose_spans_moved_answers_for_no_region_and_the_program_still_runs() {
    let pure = Compiled::new(&fixture(PURE));
    // One extra line of prelude, so every span in the body moves.
    let shifted = Compiled::new(&format!(
        "// an edit above the region\n{}",
        fixture(CAPTURING)
    ));

    let filler = pure.machine();
    let _ = filler.region_kinds();

    let span = {
        let honest = shifted.machine();
        let regions: Vec<_> = honest.region_kinds().iter().map(|r| r.span).collect();
        regions[0]
    };
    assert_eq!(
        filler.region_kind(span),
        None,
        "the edit did not move the region's span, so this is not the stale-span case"
    );

    let mut poisoned = shifted.machine();
    poisoned.share_region_kinds(filler.shared_region_kinds());
    assert_eq!(poisoned.region_kind(span), None);
    assert_eq!(
        poisoned.region_kinds().kind(span),
        RegionKind::Shared,
        "a span the analysis never saw must answer `shared`, because the safe answer to \"was a \
         capture reachable\" is always yes"
    );
    let answered = poisoned
        .call("m.go", Vec::new(), Span::DUMMY)
        .unwrap_or_else(|d| panic!("a stale analysis broke the run: [{}] {}", d.code, d.message));
    assert_eq!(
        int(answered),
        2021,
        "the answer moved under a stale analysis"
    );
}

/// The assumption under `Lowering`: a body's lowered form is a function of the body and its
/// parameter list, and of nothing the machine holds.
#[test]
fn a_lambdas_lowered_body_is_independent_of_what_it_captured() {
    let compiled = Compiled::new(
        r#"
fn adder(n: Int) -> (Int) -> Int = |x| x + n

fn go() -> Int {
  let a = adder(1);
  let b = adder(20);
  let c = adder(300);
  (a(0) + a(0)) + (b(0) + b(0)) + (c(0) + c(0))
}
"#,
    );
    let mut machine = compiled.machine();
    let answered = machine
        .call("m.go", Vec::new(), Span::DUMMY)
        .unwrap_or_else(|d| panic!("[{}] {}", d.code, d.message));
    assert_eq!(
        int(answered),
        2 * (1 + 20 + 300),
        "a lambda applied under one lowered body answered differently per capture"
    );
    // Two bodies — `adder`'s and `go`'s — however many closures `adder` minted.
    assert_eq!(
        machine.share_lowering().len(),
        2,
        "the lambda was lowered more than once, so the three closures did not share a body"
    );

    // The same shape with the closures built in a loop, so the lambda is entered once per element
    // of a list the program chose.
    let looped = Compiled::new(
        r#"
fn adder(n: Int) -> (Int) -> Int = |x| x + n

fn apply(f: (Int) -> Int) -> Int = f(0) + f(0)

fn go() -> Int = fold([1, 20, 300], 0, |acc, n| acc + apply(adder(n)))
"#,
    );
    assert_eq!(
        int(looped.call("m.go")),
        2 * (1 + 20 + 300),
        "a lambda applied under one lowered body answered differently per capture"
    );
}

/// The failure a captured binding would take if lowering ever marked it `Owned`: a closure's free
/// variable is reachable from the closure for as long as the closure lives, so moving it out at
/// what looks like a last use empties a binding a second call still reads.
#[test]
fn a_binding_a_closure_captured_is_not_moved_out_from_under_a_second_call() {
    let compiled = Compiled::new(
        r#"
fn go() -> Int {
  let xs = [1, 2, 3];
  let g = || len(push(xs, 4));
  g() + g() + len(xs)
}
"#,
    );
    assert_eq!(
        int(compiled.call("m.go")),
        4 + 4 + 3,
        "a binding a closure captured was moved out of the scope the closure shares"
    );
}

/// Moving work from runtime to a cache can change **when** it happens, and an effect is what makes
/// that observable.
#[test]
fn a_machine_reading_another_machines_lowering_performs_the_same_effects_in_the_same_order() {
    let compiled = Compiled::new(
        r#"
effect log { write note[tape](n: Int) -> Int }

fn work(n: Int) -> Int = log.note[tape](n) * 10

fn go() -> Int =
  with_cell[tape](0) { c ->
    handle {
      let a = work(1);
      let b = work(2);
      let d = fold([3, 4], 0, |acc, x| acc + work(x));
      a + b + d
    } with {
      log.note[tape](n) resume k -> { cell_set(c, cell_get(c) * 10 + n); k(n) },
      return x -> x + cell_get(c) * 100000
    } }
"#,
    );

    let mut first = compiled.machine();
    let alone = first
        .call("m.go", Vec::new(), Span::DUMMY)
        .unwrap_or_else(|d| panic!("[{}] {}", d.code, d.message));

    let mut second = compiled.machine();
    second.set_lowering(first.share_lowering());
    let shared = second
        .call("m.go", Vec::new(), Span::DUMMY)
        .unwrap_or_else(|d| panic!("[{}] {}", d.code, d.message));

    let (alone, shared) = (int(alone), int(shared));
    assert_eq!(
        alone, shared,
        "a machine reading another's lowering performed a different sequence of operations"
    );
    // 1234 in the trace cell, so the operations ran left to right exactly once each and the cache
    // changed neither the order nor the count.
    assert_eq!(shared / 100000, 1234, "the operation order moved");
}

const CAPTURE_ELSEWHERE: &str = r#"
effect amb { read flip[coin]() -> Bool }

fn search() -> Int =
  handle { if amb.flip[coin]() { 1 } else { 2 } } with {
    amb.flip[coin]() resume k -> k(true) + k(false),
    return x -> x
  }
"#;

/// A callback builtin whose function argument the analysis cannot name is the second half of the
/// same rule, and ADR 0017 §Consequences names it separately — "an escape the brand does not catch
/// — through a closure, a constructor field, a Map key, a returned continuation, or a task".
#[test]
fn a_callback_builtin_over_a_local_makes_the_region_shared() {
    let src = format!(
        "{CAPTURE_ELSEWHERE}
fn go(f: (Int) -> Int, xs: List<Int>) -> Int =
  with_cell[acc](0) {{ c -> {{ cell_set(c, len(map(xs, f))); cell_get(c) }} }}
"
    );
    let compiled = Compiled::new(&src);
    let machine = compiled.machine();
    let acc = machine
        .region_kinds()
        .iter()
        .find(|r| r.brand.as_str() == "acc")
        .expect("the probe opens `acc`");
    assert_eq!(
        acc.kind,
        RegionKind::Shared,
        "`map` over a callback this analysis cannot name must be `shared`: {acc:?}"
    );
    assert!(
        matches!(
            acc.capture.as_ref().map(|c| &c.cause),
            Some(Cause::Callback { builtin: "map" })
        ),
        "the site is attributed to something other than the callback: {:?}",
        acc.capture
    );
}

/// The control, and the contract: a call whose callee is a parameter is a call to anything in the
/// program, so a region holding one is `shared` as soon as the program writes a capture anywhere.
#[test]
fn a_region_whose_body_calls_a_parameter_is_shared() {
    let src = format!(
        "{CAPTURE_ELSEWHERE}
fn go(f: (Int) -> Int) -> Int =
  with_cell[acc](0) {{ c -> {{ cell_set(c, f(1)); cell_get(c) }} }}
"
    );
    let compiled = Compiled::new(&src);
    let machine = compiled.machine();
    let acc = machine
        .region_kinds()
        .iter()
        .find(|r| r.brand.as_str() == "acc")
        .expect("the probe opens `acc`");
    assert_eq!(
        acc.kind,
        RegionKind::Shared,
        "an unknown callee in a program that captures must be `shared`: {acc:?}"
    );
}

/// The control above, with the local renamed to the name of a top-level definition in the same
/// module — once per kind of binder the language has.
#[test]
fn a_local_shadowing_a_definitions_name_is_still_a_local() {
    let bodies = [
        (
            "a parameter",
            "fn go(helper: (Int) -> Int) -> Int =
  with_cell[acc](0) { c -> { cell_set(c, helper(1)); cell_get(c) } }",
        ),
        (
            "a `let`",
            "fn go(f: (Int) -> Int) -> Int =
  with_cell[acc](0) { c -> { let helper = f; cell_set(c, helper(1)); cell_get(c) } }",
        ),
        (
            "a `match` binder",
            "fn go(fs: List<(Int) -> Int>) -> Int =
  with_cell[acc](0) { c ->
    match fs {
      [helper, ..rest] -> { cell_set(c, helper(1)); cell_get(c) },
      _ -> 0
    } }",
        ),
        (
            "a lambda parameter",
            "fn go(f: (Int) -> Int) -> Int =
  with_cell[acc](0) { c -> { cell_set(c, (|helper| helper(1))(f)); cell_get(c) } }",
        ),
        (
            "a callback argument",
            "fn go(helper: (Int) -> Int, xs: List<Int>) -> Int =
  with_cell[acc](0) { c -> { cell_set(c, len(map(xs, helper))); cell_get(c) } }",
        ),
    ];
    for (what, body) in bodies {
        let src = format!("{CAPTURE_ELSEWHERE}\nfn helper(n: Int) -> Int = n + 1\n\n{body}\n");
        let compiled = Compiled::new(&src);
        let machine = compiled.machine();
        let acc = machine
            .region_kinds()
            .iter()
            .find(|r| r.brand.as_str() == "acc")
            .unwrap_or_else(|| panic!("the {what} probe opens `acc`"));
        assert_eq!(
            acc.kind,
            RegionKind::Shared,
            "{what} shadowing `helper` was read as the definition `m.helper`, so a callee that \
             could be any closure in the program inferred `unique`: {acc:?}"
        );
    }
}

/// The other half of the same defect, and the one ADR 0017 §3 states as a rule rather than as a
/// cost: `region_kind::check` must **refuse** a hand-written `unique` over a reachable capture.
#[test]
fn a_declared_unique_over_a_local_shadowing_a_definition_is_refused() {
    let src = format!(
        "{CAPTURE_ELSEWHERE}
fn helper(n: Int) -> Int = n + 1

fn go(helper: (Int) -> Int) -> Int =
  with_cell[acc](0) {{ c -> {{ cell_set(c, helper(1)); cell_get(c) }} }}
"
    );
    let compiled = Compiled::new(&src);
    let span = ply_eval::region_kind::infer(&compiled.program, &compiled.resolved)
        .iter()
        .find(|r| r.brand.as_str() == "acc")
        .expect("the probe opens `acc`")
        .span;
    let refusals = ply_eval::region_kind::check(
        &compiled.program,
        &compiled.resolved,
        &[(span, RegionKind::Unique)],
    )
    .expect_err("`unique` was accepted over a callee that could be any closure in the program");
    assert_eq!(refusals.len(), 1);
    assert_eq!(refusals[0].code, ply_span::codes::REGION_KIND_REFUSED);
    assert!(
        matches!(
            ply_eval::region_kind::infer(&compiled.program, &compiled.resolved)
                .at(span)
                .and_then(|r| r.capture.as_ref())
                .map(|c| &c.cause),
            Some(Cause::Indirect)
        ),
        "the refusal is attributed to something other than the unknown callee"
    );
}

/// The lowering half of the poisoning above, and it comes out the other way: where a span-keyed
/// `Kinds` from the wrong program delivers a wrong answer, an address-keyed `Lowering` from the
/// wrong program cannot.
#[test]
fn a_lowering_from_another_program_answers_for_nothing_in_this_one() {
    let one = Compiled::new("fn go() -> Int = 111\n");
    let two = Compiled::new("fn go() -> Int = 222\n");

    let mut first = one.machine();
    assert_eq!(
        int(first.call("m.go", Vec::new(), Span::DUMMY).unwrap()),
        111
    );
    let filled = first.share_lowering();
    assert_eq!(filled.len(), 1, "the first machine lowered nothing");
    assert!(
        !filled.describes(&two.program),
        "a cache over one program claims to describe another, so the span the two fixtures share \
         is enough to answer from the wrong body"
    );

    let mut poisoned = two.machine();
    poisoned.set_lowering(std::rc::Rc::clone(&filled));
    assert!(
        !std::rc::Rc::ptr_eq(&poisoned.share_lowering(), &filled),
        "a machine installed a lowering taken over another program"
    );
    assert_eq!(
        int(poisoned
            .call("m.go", Vec::new(), Span::DUMMY)
            .unwrap_or_else(|d| panic!("[{}] {}", d.code, d.message))),
        222,
        "a machine answered from a body belonging to another program"
    );
}
