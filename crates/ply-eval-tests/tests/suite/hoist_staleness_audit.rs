//! What a **stale** entry in either of R3's two caches can do to a program.

use crate::fixture::Compiled;
use ply_eval::region_kind::Cause;
use ply_eval::{RegionKind, Value};
use ply_span::Span;

impl Compiled {
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

/// The same pair for a tail-resumptive clause.
const TAIL_PRELUDE: &str = "effect log { write note[tape](n: Int) -> Int }\n\nfn go() -> Int =\n  with_cell[tape](0) { c -> ";

const TAIL_CAPTURING: &str = "{ let total = handle { log.note[tape](1) + log.note[tape](2) } with { log.note[tape](n) -> { cell_set(c, cell_get(c) * 10 + n); n } }; total + cell_get(c) * 1000 }";

const TAIL_PURE: &str = "{ cell_set(c, 12); 3 + cell_get(c) * 1000 }";

fn tail_fixture(body: &str) -> String {
    let pad = " ".repeat(TAIL_CAPTURING.len().saturating_sub(body.len()));
    format!("{TAIL_PRELUDE}{body}{pad} }}\n")
}

/// The same question on a tail-resumptive region, which takes no pin.
///
/// The staleness arm is vacuous since the tail-resumptive refinement — `unique` is now the honest inference for this
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
        "the tail-resumptive refinement: a tail-resumptive clause is not a capture that outlives its region"
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

const CAPTURE_ELSEWHERE: &str = r#"
effect amb { read flip[coin]() -> Bool }

fn search() -> Int =
  handle { if amb.flip[coin]() { 1 } else { 2 } } with {
    amb.flip[coin]() resume k -> k(true) + k(false),
    return x -> x
  }
"#;

/// A callback builtin whose function argument the analysis cannot name is the second half of the
/// same rule, and the region model names it separately — "an escape the brand does not catch
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

/// The other half of the same defect, and the one the region-kind rule states as a rule rather than as a
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
