//! What a **stale** entry in either of R3's two caches can do to a program.
//!
//! R3 moved two compile-time analyses off the request path by giving each a
//! cache: `ply_eval::region_kind::Kinds`, one region-kind analysis per program,
//! and `ply_eval::Lowering`, one lowered body per program. `region_kind_sharing`
//! and `lowering_sharing` assert that a *correct* handle is shared. Neither asks
//! the question a cache exists to raise, which is what happens when the entry is
//! **wrong** — and one of these caches carries the kind that ADR 0017 §3 says
//! decides when a region's memory may be reclaimed.
//!
//! Every fixture here is therefore a deliberately poisoned cache, and what is
//! asserted is that meaning did not move under it.
//!
//! Two of the tests below **used** to pin a defect rather than a contract, and
//! both defects are now closed; each carries a block quoting what it measured
//! before. `a_local_shadowing_a_definitions_name_is_still_a_local` was
//! `..is_read_as_that_definition_and_infers_unique`, closed by
//! `region_kind::Analysis::locals`;
//! `a_lowering_from_another_program_answers_for_nothing_in_this_one` was
//! `the_lowering_cache_accepts_a_body_that_does_not_outlive_it`, closed by
//! making `Lowering<'a>` invariant in `'a` — whose compile-time half is a
//! `compile_fail` doc-test on the type, because no `#[test]` can observe a
//! variance.
//!
//! # The reclamation question, answered
//!
//! ADR 0017 §Consequences names "`unique` inferred where a capture is reachable,
//! which frees memory a continuation can still reach" as the hardest of its three
//! failures to see. It is not reachable through the kind on this tree, and the
//! reason is structural rather than lucky:
//! `Arena::close_at` decides between truncating and retaining by asking
//! `Arena::pinned`, and it never reads the scope's `RegionKind`. The only three
//! readers of that field in `crates/ply-eval/src/arena.rs` are `Arena::kind`,
//! `Arena::unique_open` and `Arena::snapshot`/`snapshot_open`, and
//! `grep -rn 'unique_open\|snapshot_open' crates/*/src` finds no caller outside
//! `arena.rs` itself. `a_region_kind_from_the_wrong_program_cannot_free_a_region_a_continuation_reaches`
//! is that argument run end to end through an engine rather than read off the
//! source, and
//! `a_stale_unique_over_a_tail_resumptive_region_moves_neither_engine` is the
//! same question on the one shape `--engine both` can compare.

use ply_core::{CheckOutput, check_program};
use ply_eval::region_kind::Cause;
use ply_eval::{Interp, Machine, RegionKind, Value};
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
        let program = ply_syntax::parse_program(inputs)
            .unwrap_or_else(|d| panic!("the fixture must parse: {d:#?}\n{src}"));
        let resolved =
            resolve(&program).unwrap_or_else(|d| panic!("the fixture must resolve: {d:#?}\n{src}"));
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

    fn interp(&self) -> Interp<'_> {
        Interp::new(&self.program, &self.resolved, &self.check)
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

// ------------------------------------------- a region kind from another program

/// The two fixtures below differ only inside the region's body and are padded to
/// the same length, so `with_cell[r]` occupies **one span** in both. That is what
/// lets a `Kinds` filled from the first be read by an engine running the second:
/// the handle is keyed by nothing but the span, so where two programs agree on a
/// span the wrong answer is delivered rather than missed.
const PRELUDE: &str =
    "effect amb { read flip[coin]() -> Bool }\n\nfn go() -> Int =\n  with_cell[r](0) { c -> ";

/// Two resumptions over one cell — ADR 0017 §3's deciding example, with the
/// trace cell folded into the answer so one integer carries both. Threaded state
/// gives `1 + 2*10 = 21` and a cell of `2`, so `2021`; snapshot-at-capture would
/// give `11` and a cell of `1`, so `1011`.
const CAPTURING: &str = "{ let total = handle { let b = amb.flip[coin](); cell_set(c, cell_get(c) + 1); if b { cell_get(c) } else { cell_get(c) * 10 } } with { amb.flip[coin]() resume k -> k(true) + k(false), return x -> x }; total + cell_get(c) * 1000 }";

/// The same region with no capture in it at all, so the analysis infers
/// `unique`. It answers `1021` on its own and is never run: it exists only to
/// fill a `Kinds` with the wrong answer about the fixture above.
const PURE: &str = "{ cell_set(c, cell_get(c) + 1); 21 + cell_get(c) * 1000 }";

fn fixture(body: &str) -> String {
    let pad = " ".repeat(CAPTURING.len().saturating_sub(body.len()));
    format!("{PRELUDE}{body}{pad} }}\n")
}

/// The same pair for a tail-resumptive clause, which both engines run. The
/// handler writes the operation's argument into the cell, so the answer carries
/// the order the two operations were performed in as well as the total.
const TAIL_PRELUDE: &str = "effect log { write note[tape](n: Int) -> Int }\n\nfn go() -> Int =\n  with_cell[tape](0) { c -> ";

const TAIL_CAPTURING: &str = "{ let total = handle { log.note[tape](1) + log.note[tape](2) } with { log.note[tape](n) -> { cell_set(c, cell_get(c) * 10 + n); n } }; total + cell_get(c) * 1000 }";

const TAIL_PURE: &str = "{ cell_set(c, 12); 3 + cell_get(c) * 1000 }";

fn tail_fixture(body: &str) -> String {
    let pad = " ".repeat(TAIL_CAPTURING.len().saturating_sub(body.len()));
    format!("{TAIL_PRELUDE}{body}{pad} }}\n")
}

/// **The premature-free hunt, and it comes back empty.**
///
/// A `Kinds` filled from the pure program says `unique` at the very span the
/// capturing program opens its region at — "no continuation is captured across
/// this region, so its slots go back at its close". The capturing program then
/// resumes twice across that close. If the kind decided reclamation, the second
/// resumption would read a slot the first close handed back, and `2021` would
/// come out as something else or as a stale-slot diagnostic.
///
/// It comes out `2021`, because the close is decided by the pin the capture took
/// and not by the kind. That is the module comment's argument, measured.
///
/// The machine only. The tree-walker refuses every clause that binds a
/// continuation — `E0504`, which this test asserts rather than works around,
/// because it is the hole ADR 0017 §"What must be measured" ¶4 records in
/// `--engine both`: the oracle audits nothing about multi-shot resumption, which
/// is exactly the construct a region kind is a claim about.
/// `a_stale_unique_over_a_tail_resumptive_region_moves_neither_engine` is the
/// same question on a shape both engines will run.
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

    let mut treewalk = capturing.interp();
    treewalk.share_region_kinds(filler.shared_region_kinds());
    let refused = treewalk
        .call("m.go", Vec::new(), Span::DUMMY)
        .expect_err("the tree-walker binds no continuation");
    assert_eq!(
        refused.code,
        ply_span::codes::MACHINE_ONLY_CLAUSE,
        "the tree-walker now runs a `resume` binder, so `--engine both` can audit this shape and \
         the second engine belongs in this test: {}",
        refused.message
    );
}

/// The same question on the shape `--engine both` *does* audit.
///
/// A tail-resumptive clause captures too — ADR 0005 §1.3 runs `K.capture(n)` for
/// both clause forms — so a region holding one is `shared`, and it is the shape
/// the whole corpus is written in
/// (`region_kind_inference::the_split_over_the_repositorys_own_examples`).
/// It is also the shape that takes **no pin**, so the pin-driven argument above
/// does not cover it: what makes it safe is that the `Resume` frame consumes the
/// continuation one frame above the region's `CloseRegion`.
///
/// A stale `unique` therefore has to move neither engine, and the two have to
/// stay equal to each other, which is what an `E0503` divergence would be.
#[test]
fn a_stale_unique_over_a_tail_resumptive_region_moves_neither_engine() {
    let pure = Compiled::new(&tail_fixture(TAIL_PURE));
    let tail = Compiled::new(&tail_fixture(TAIL_CAPTURING));

    let filler = pure.machine();
    let span = tail.machine().region_kinds().iter().next().unwrap().span;
    assert_eq!(
        filler.region_kind(span),
        Some(RegionKind::Unique),
        "the two fixtures no longer place their region at one span"
    );
    assert_eq!(tail.machine().region_kind(span), Some(RegionKind::Shared));

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

    let mut treewalk = tail.interp();
    treewalk.share_region_kinds(filler.shared_region_kinds());
    let on_treewalk = treewalk
        .call("m.go", Vec::new(), Span::DUMMY)
        .unwrap_or_else(|d| {
            panic!(
                "a stale `unique` broke the tree-walker: [{}] {}",
                d.code, d.message
            )
        });

    assert_eq!(
        int(on_machine),
        honest,
        "a tail-resumptive region the analysis called `unique` answered differently on the machine"
    );
    assert_eq!(
        int(on_treewalk),
        honest,
        "the two engines disagree under a stale region kind, which is an `E0503` divergence"
    );
}

/// The ordinary shape of staleness: an edit moves every span after it, so a
/// handle filled before the edit answers about spans the running program does
/// not have.
///
/// The safe answer to "was a capture reachable across a region I have never
/// seen" is taken twice over and in two different ways — `Regions::kind`
/// answers `Shared`, and `Machine::region_kind` answers `None`, which opens no
/// region at all and leaves the cell in the enclosing one. Both retain; neither
/// frees. What must not happen is that a *shifted* span collides with a region
/// the stale analysis called `unique`.
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

// -------------------------------- what a shared lowering may and may not depend on

/// The assumption under `Lowering`: a body's lowered form is a function of the
/// body and its parameter list, and of nothing the machine holds. A closure's
/// captured environment is the thing most obviously not in that pair, and
/// `crate::rc::Live` is what could put it there — an occurrence marked
/// [`ply_eval::Own::Owned`] is one the machine may *move* out of the scope
/// rather than clone.
///
/// So: three closures over one lambda, built with different captures, each
/// applied twice. One lowered body serves all six applications. If the lowering
/// had folded anything about the first closure's environment into that body the
/// later ones would answer with it.
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
    // A count that grew with the closures would mean the lambda was being
    // lowered per capture, and the test above would be proving nothing.
    assert_eq!(
        machine.share_lowering().len(),
        2,
        "the lambda was lowered more than once, so the three closures did not share a body"
    );

    // The same shape with the closures built in a loop, so the lambda is entered
    // once per element of a list the program chose.
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

/// The failure a captured binding would take if lowering ever marked it
/// `Owned`: a closure's free variable is reachable from the closure for as long
/// as the closure lives, so moving it out at what looks like a last use empties
/// a binding a second call still reads.
///
/// `xs` is read once inside the lambda — textually a last use — and twice more
/// outside it. `Live::open` is what keeps it `Borrowed`, and the two `g()` calls
/// plus the trailing `len(xs)` are what notice if it stops.
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

/// Moving work from runtime to a cache can change **when** it happens, and an
/// effect is what makes that observable. Lowering is pure, so the claim is that
/// it cannot — and the way to check it is to compare the sequence of operations
/// two machines perform, one lowering for itself and one reading the other's
/// cache.
///
/// The handler appends to a cell, so the value the test answers *is* the order.
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
    // 1234 in the trace cell, so the operations ran left to right exactly once
    // each and the cache changed neither the order nor the count.
    assert_eq!(shared / 100000, 1234, "the operation order moved");
}

// -------------------------------------------------- a finding, pinned as found

const CAPTURE_ELSEWHERE: &str = r#"
effect amb { read flip[coin]() -> Bool }

fn search() -> Int =
  handle { if amb.flip[coin]() { 1 } else { 2 } } with {
    amb.flip[coin]() resume k -> k(true) + k(false),
    return x -> x
  }
"#;

/// A callback builtin whose function argument the analysis cannot name is the
/// second half of the same rule, and ADR 0017 §Consequences names it separately
/// — "an escape the brand does not catch — through a closure, a constructor
/// field, a Map key, a returned continuation, or a task". `map`'s callback is a
/// parameter here, so `map` may call anything the program holds.
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

/// The control, and the contract: a call whose callee is a parameter is a call
/// to anything in the program, so a region holding one is `shared` as soon as
/// the program writes a capture anywhere.
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

/// The control above, with the local renamed to the name of a top-level
/// definition in the same module — once per kind of binder the language has.
///
/// `Analysis::definition` resolves a bare name against `Resolved::scopes`, which
/// is the **module** scope, so before `Analysis::locals` existed every one of
/// these was read as `m.helper`: an edge to a body that reaches no capture,
/// instead of `Cause::Indirect` over a callee that is whatever the caller
/// passed. Each inferred `unique`, which is the one direction
/// `region_kind`'s module comment says an undecidable case may never land on —
/// "*Inferring `unique` where a capture is reachable frees memory a continuation
/// can still reach*".
///
/// > **Was `a_local_shadowing_a_definitions_name_is_read_as_that_definition_and_infers_unique`,
/// > which pinned that as a defect** and measured
/// > `Region { brand: "acc", kind: Unique, capture: None }` where the control
/// > beside it measured `Shared` with `Cause::Indirect`. It now asserts the
/// > answer rather than the defect, and covers the `let` and pattern binders the
/// > original only noted in prose.
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

/// The other half of the same defect, and the one ADR 0017 §3 states as a rule
/// rather than as a cost: `region_kind::check` must **refuse** a hand-written
/// `unique` over a reachable capture. Reading the shadowed `helper` as the
/// definition made the capture unreachable, so the annotation was accepted.
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

/// The lowering half of the poisoning above, and it comes out the other way:
/// where a span-keyed `Kinds` from the wrong program delivers a wrong answer,
/// an address-keyed `Lowering` from the wrong program cannot.
///
/// The two fixtures below are the same length, so `go`'s body occupies the same
/// span in both — which is all it takes to poison a `Kinds`, as the top of this
/// file measures. It is not enough to poison a `Lowering`, twice over: two live
/// programs hold their bodies at different addresses so every lookup would miss
/// anyway, and `Machine::set_lowering` refuses a cache `Lowering::describes`
/// says was taken over another program before it can even miss.
///
/// > **This replaced a test that pinned a defect** —
/// > `the_lowering_cache_accepts_a_body_that_does_not_outlive_it`, which keyed a
/// > `Box<Expr>` into a longer-lived cache through a covariant coercion and
/// > asserted that the cache took it. It did, and a `Box<Expr>` holding `222`
/// > landing at the freed one's address was answered with `111` on the first of
/// > a thousand attempts. `Lowering` is now invariant in `'a`
/// > (`code.rs`'s `invariant` field), so that coercion no longer compiles and
/// > the test could no longer be written. The compile-time half of the fix is
/// > machine-checked by the `compile_fail` doc-test on `Lowering` itself; this
/// > is the runtime half.
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
