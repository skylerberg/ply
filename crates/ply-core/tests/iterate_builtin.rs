//! `iterate`'s type surface, at the source level: the one loop whose bound is
//! an argument rather than the evaluator's call ceiling.
//!
//! Two things are pinned here that nothing else pins. The **signature**, whose
//! shape is the whole argument for a second type parameter — `Stop` carries an
//! `r` the seed never held, so a loop can finish with something it computed on
//! its last step instead of running one more round to report it. And the
//! **row**, which `iterate` threads from its step into its caller's footprint:
//! a builtin that swallowed it would let a definition reaching a socket publish
//! an empty footprint, which is the failure ADR 0012 calls a green result over
//! unexplored space.

use ply_core::{CheckOutput, check_program, print_type};
use ply_span::{Diagnostic, SourceId, Symbol, codes};
use ply_syntax::ast::ModuleName;
use ply_syntax::resolve::resolve;

fn compile(source: &str) -> Result<CheckOutput, Vec<Diagnostic>> {
    let inputs = vec![(SourceId(0), ModuleName::from_dotted("m"), source)];
    let mut program = ply_syntax::parse_program(inputs)?;
    let resolved = resolve(&mut program)?;
    check_program(&program, &resolved)
}

fn ok(source: &str) -> CheckOutput {
    match compile(source) {
        Ok(out) => out,
        Err(d) => panic!("expected this to check:\n{source}\ngot {d:#?}"),
    }
}

fn errors(source: &str) -> Vec<Diagnostic> {
    match compile(source) {
        Ok(_) => panic!("expected a diagnostic from:\n{source}"),
        Err(d) => d,
    }
}

fn sig(out: &CheckOutput, name: &str) -> String {
    print_type(&out.defs[&Symbol::new(format!("m.{name}"))].scheme.ty)
}

fn footprint(out: &CheckOutput, name: &str) -> String {
    out.defs[&Symbol::new(format!("m.{name}"))]
        .footprint
        .to_string()
}

/// `fn probe_f() = f` returns the builtin itself, so the printed signature of
/// the probe carries the builtin's whole type.
#[test]
fn iterate_has_the_type_the_contract_states() {
    let out = ok("fn probe_iterate() = iterate\n");
    assert_eq!(
        sig(&out, "probe_iterate"),
        "() -> (a, Int, (a) -> Iter<a, b> / e) -> b / e"
    );
}

/// The budget sits **second** and the callback **last**. Not a matter of taste:
/// `ply_eval::region_kind::Analysis::walk_callback` reads the callback out of
/// `args.last()`, so a callback in the middle would be read as data and the
/// budget read as the callback, silently. Stated here in the type, where a
/// change to the order fails rather than degrades.
#[test]
fn the_budget_is_the_second_argument_and_the_step_is_the_last() {
    ok("fn go(n: Int) -> Int = iterate(0, n, |s: Int| Stop(s))\n");
    let d = errors("fn go(n: Int) -> Int = iterate(0, |s: Int| Stop(s), n)\n");
    assert!(
        d.iter().any(|d| d.code == codes::TYPE_MISMATCH),
        "swapping the budget and the step must not check: {d:#?}"
    );
}

/// A step's row is the caller's. Every higher-order builtin threads one —
/// `map`, `filter`, `fold`, `map_fold` and `bytes_position` — and `iterate` is
/// the only one whose loop can end without a collection to end at.
#[test]
fn an_iterate_publishes_the_row_of_the_step_it_drives() {
    let out = ok(r#"
effect tell { write say[out](what: String) -> Unit }

fn pure_loop(n: Int) -> Int = iterate(0, n, |s: Int| if s >= n { Stop(s) } else { Continue(s + 1) })

fn loud_loop(n: Int) -> Int =
  iterate(0, n, |s: Int| if s >= n { Stop(s) } else { tell.say[out]("x"); Continue(s + 1) })
"#);
    assert_eq!(
        footprint(&out, "pure_loop"),
        "{}",
        "an empty row prints empty"
    );
    assert!(
        footprint(&out, "loud_loop").contains("tell.write[out]"),
        "the step's row must reach the caller's footprint, got {}",
        footprint(&out, "loud_loop")
    );
}

/// `Iter` joins `builtin_types()`, so a project's own `type Iter` is `E0105`
/// exactly as `type Option` already is. That is the cost of the name and it is
/// stated rather than discovered — see `FRONTEND_VERSION` 0.16.0.
#[test]
fn a_project_may_not_declare_its_own_iter() {
    let d = errors("type Iter = Yes | No\n");
    let dup: Vec<&Diagnostic> = d
        .iter()
        .filter(|d| d.code == codes::DUPLICATE_DEFINITION)
        .collect();
    assert_eq!(dup.len(), 1, "{d:#?}");
    assert!(
        dup[0].message.contains("`Iter` is a builtin type"),
        "{}",
        dup[0].message
    );
}

/// `Continue` and `Stop` are **constructors**, not type names, and constructors
/// are not globally reserved: a module's own shadow the prelude's, which is why
/// `std.signal`'s `type Stop` and `std.json`'s `type Step` still check. The
/// alternative spelling — naming the ADT `Step` — would have broken a shipped
/// `std` module, and this is what records that it was checked rather than
/// assumed.
#[test]
fn a_module_may_still_declare_its_own_stop_and_continue() {
    let out = ok(r#"
type Stop = { stopping: Bool }
type Phase = Continue(Int) | Done

fn halt() -> Stop = { stopping: true }
fn first() -> Phase = Continue(1)
"#);
    // The alias is normalized away in the printed type; that it checked at all
    // is the claim — the prelude's `Stop` constructor did not collide with it.
    assert_eq!(sig(&out, "halt"), "() -> {stopping: Bool}");
    assert_eq!(sig(&out, "first"), "() -> m.Phase");
    // And the prelude's own `Stop` is still reachable where nothing shadows it.
    let plain = ok("fn stop_at(n: Int) -> Iter<Int, Int> = Stop(n)\n");
    assert_eq!(sig(&plain, "stop_at"), "(Int) -> Iter<Int, Int>");
}
