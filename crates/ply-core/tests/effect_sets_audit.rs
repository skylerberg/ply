//! Adversarial audit of "an `effect set` is annotation-only".
//!
//! `effect_sets.rs` shows the feature working. This file tries to make it lie,
//! because the dangerous failure here is not a refusal — it is an acceptance. A
//! set that could *narrow* what a body publishes would hand the scheduler a
//! footprint that under-reports, and every defect this project has found was of
//! that shape: a green result over unexplored space.
//!
//! Four things are attacked:
//!
//! 1. **An alias cannot launder an atom.** Whatever the body reaches has to be
//!    inside the expansion, including a `nondet` atom, an atom under a
//!    different resource label, and an atom reached only through a callee.
//! 2. **An alias cannot widen what inference produced.** `performed` is the
//!    body's row whatever the annotation says, and the difference between the
//!    two is published rather than absorbed.
//! 3. **A refused set contributes nothing and says so once.** A cycle, an
//!    unknown name and a duplicate are refusals, not silent empty expansions
//!    that would let a body through unchecked.
//! 4. **`E0412` sees the atoms.** A `nondet` atom inside an expansion is a
//!    determinism verdict exactly as the written atom is.

use ply_core::{CheckOutput, check_program};
use ply_span::{Diagnostic, SourceId, Symbol, codes};
use ply_syntax::ast::ModuleName;
use ply_syntax::resolve::resolve;

fn compile(source: &str) -> Result<CheckOutput, Vec<Diagnostic>> {
    let inputs = vec![(SourceId(0), ModuleName::from_dotted("m"), source)];
    let mut program = ply_syntax::parse_program(inputs)?;
    let resolved = resolve(&mut program)?;
    check_program(&program, &resolved)
}

#[track_caller]
fn ok(source: &str) -> CheckOutput {
    match compile(source) {
        Ok(out) => out,
        Err(d) => panic!("expected this to check:\n{source}\ngot {d:#?}"),
    }
}

#[track_caller]
fn errors(source: &str) -> Vec<Diagnostic> {
    match compile(source) {
        Ok(_) => panic!("expected a diagnostic from:\n{source}"),
        Err(d) => d,
    }
}

#[track_caller]
fn only<'a>(diags: &'a [Diagnostic], code: &str) -> &'a Diagnostic {
    diags
        .iter()
        .find(|d| d.code == code)
        .unwrap_or_else(|| panic!("no `{code}` in {diags:#?}"))
}

fn text_of(d: &Diagnostic) -> String {
    let mut out = d.message.clone();
    for label in &d.labels {
        out.push('\n');
        out.push_str(&label.message);
    }
    for note in &d.notes {
        out.push('\n');
        out.push_str(note);
    }
    out
}

fn published(out: &CheckOutput, name: &str) -> String {
    out.defs[&Symbol::new(name)].footprint.to_string()
}

fn performed(out: &CheckOutput, name: &str) -> String {
    out.defs[&Symbol::new(name)].performed.to_string()
}

const EFFECTS: &str = "\
effect db {
  read  all[t]() -> Int
  write save[t](n: Int) -> Int
}
effect log {
  write line(n: Int) -> Int
}
nondet effect clock {
  read now() -> Int
}
";

fn program(rest: &str) -> String {
    format!("{EFFECTS}{rest}")
}

// --- 1. an alias cannot launder an atom -------------------------------------

/// The attack that matters most. If an alias could hide a `nondet` atom, the
/// determinism check would be reading a row the body does not have, and a flaky
/// test would compile.
#[test]
fn an_alias_cannot_hide_a_nondet_atom_the_body_performs() {
    let diags = errors(&program(
        "effect set Narrow = {db.read[users], log.write}\n\
         fn handler() -> Int / {Narrow} { log.line(1); clock.now() }\n",
    ));
    let d = only(&diags, codes::EFFECT_NOT_PERMITTED);
    let text = text_of(d);
    assert!(
        text.contains("clock.read"),
        "the refusal must name the laundered atom: {text}"
    );
    assert!(
        !text.contains("Narrow"),
        "the body failed to satisfy the expansion, not a name: {text}"
    );
}

/// Resource granularity is the whole design contribution, so an alias that
/// holds `db.read[users]` may not stand in for `db.read[orders]`.
#[test]
fn an_alias_over_one_resource_does_not_cover_another() {
    let diags = errors(&program(
        "effect set Users = {db.read[users]}\n\
         fn handler() -> Int / {Users} = db.all[orders]()\n",
    ));
    let text = text_of(only(&diags, codes::EFFECT_NOT_PERMITTED));
    assert!(
        text.contains("db.read[orders]"),
        "the refusal must name the resource that was not permitted: {text}"
    );
}

/// The mode is half of an atom. A set of readers does not admit a write.
#[test]
fn an_alias_of_reads_does_not_admit_a_write() {
    let diags = errors(&program(
        "effect set Reads = {db.read[users], db.read[orders]}\n\
         fn handler() -> Int / {Reads} = db.save[users](1)\n",
    ));
    let text = text_of(only(&diags, codes::EFFECT_NOT_PERMITTED));
    assert!(
        text.contains("db.write[users]"),
        "the refusal must name the write: {text}"
    );
}

/// An atom reached only through a callee is still the caller's atom, and the
/// alias bounds it the same way.
#[test]
fn an_alias_bounds_what_a_callee_reaches() {
    let diags = errors(&program(
        "effect set Narrow = {db.read[users]}\n\
         fn inner() -> Int { log.line(1); db.all[users]() }\n\
         fn outer() -> Int / {Narrow} = inner()\n",
    ));
    let text = text_of(only(&diags, codes::EFFECT_NOT_PERMITTED));
    assert!(
        text.contains("log.write"),
        "the atom the callee added must be named: {text}"
    );
}

/// An empty set is a bound, not an absence of one. `/ {Nothing}` publishes the
/// closed empty row, so a body that performs anything is refused — an alias
/// that expanded to "no annotation" would silently turn a published signature
/// into an inferred one.
#[test]
fn an_empty_set_is_the_empty_bound_and_refuses_everything() {
    let diags = errors(&program(
        "effect set Nothing = {}\n\
         fn handler() -> Int / {Nothing} = db.all[users]()\n",
    ));
    let text = text_of(only(&diags, codes::EFFECT_NOT_PERMITTED));
    assert!(
        text.contains("db.read[users]"),
        "the refusal must name the atom: {text}"
    );
    let out = ok(&program(
        "effect set Nothing = {}\n\
         fn handler(x: Int) -> Int / {Nothing} = x + 1\n",
    ));
    assert_eq!(published(&out, "m.handler"), "{}");
}

/// A row variable admits *more* atoms, so it must not admit them past the set:
/// the concrete part of `{Web | e}` is still an upper bound on the concrete
/// part of what the body performs.
#[test]
fn a_row_variable_beside_a_set_does_not_dissolve_the_bound() {
    let diags = errors(&program(
        "effect set Narrow = {log.write}\n\
         fn run<a | e>(f: () -> a / e) -> a / {Narrow | e} { db.save[orders](1); f() }\n",
    ));
    let text = text_of(only(&diags, codes::EFFECT_NOT_PERMITTED));
    assert!(
        text.contains("db.write[orders]"),
        "the concrete atom must still be refused: {text}"
    );
}

/// A handler discharges atoms, and the alias bounds what is left over rather
/// than what was performed inside the region. The clause bodies' own atoms are
/// part of that — a handler backed by something effectful still reports it.
#[test]
fn an_alias_bounds_what_survives_a_handler_including_the_clauses() {
    let out = ok(&program(
        "effect set Narrow = {log.write}\n\
         fn inner() -> Int = db.all[users]()\n\
         fn handler() -> Int / {Narrow} =\n\
         \x20 handle { inner() } with { db.all[users]() -> log.line(1) }\n",
    ));
    assert_eq!(performed(&out, "m.handler"), "{m.log.write}");

    let diags = errors(&program(
        "effect set Narrow = {log.write}\n\
         fn inner() -> Int = db.all[users]()\n\
         fn handler() -> Int / {Narrow} =\n\
         \x20 handle { inner() } with { db.all[users]() -> db.save[orders](1) }\n",
    ));
    let text = text_of(only(&diags, codes::EFFECT_NOT_PERMITTED));
    assert!(
        text.contains("db.write[orders]"),
        "a clause's own atoms are the handle's atoms: {text}"
    );
}

// --- 2. an alias cannot widen what inference produced ------------------------

/// The published row is the expansion and `performed` is the body's. An
/// over-broad set is legal, costs what ADR 0013 §1.6 says it costs, and is
/// visible rather than absorbed.
#[test]
fn an_over_broad_alias_widens_the_published_row_and_not_the_performed_one() {
    let out = ok(&program(
        "effect set Wide = {db.read[users], db.write[orders], log.write}\n\
         fn handler() -> Int / {Wide} = db.all[users]()\n",
    ));
    assert_eq!(
        published(&out, "m.handler"),
        "{m.db.write[orders], m.db.read[users], m.log.write}"
    );
    assert_eq!(performed(&out, "m.handler"), "{m.db.read[users]}");
}

/// The consequence a caller feels: it is checked against what the callee
/// *published*, so an over-broad set propagates. That is the scheduling cost
/// §1.6 names, and it is a property rather than an accident.
#[test]
fn a_caller_inherits_the_published_row_rather_than_the_performed_one() {
    let out = ok(&program(
        "effect set Wide = {db.read[users], db.write[orders], log.write}\n\
         fn handler() -> Int / {Wide} = db.all[users]()\n\
         fn caller() -> Int = handler()\n",
    ));
    assert_eq!(
        published(&out, "m.caller"),
        "{m.db.write[orders], m.db.read[users], m.log.write}"
    );
    assert_eq!(
        performed(&out, "m.caller"),
        "{m.db.write[orders], m.db.read[users], m.log.write}",
        "an unannotated definition performs what it publishes"
    );
}

// --- 3. `E0412` sees the atoms ----------------------------------------------

/// A `nondet` atom that a set carries but the body never performs still reaches
/// the determinism check, because a caller is checked against the *published*
/// row. This is the sharpest form of the over-broad-alias cost: it does not
/// merely widen a conflict graph, it can refuse a `det` test — which is the
/// safe direction, and is recorded here so that the day it changes is a day
/// somebody decided to change it.
#[test]
fn a_nondet_atom_an_over_broad_set_declares_still_reaches_e0412() {
    let diags = errors(&program(
        "effect set Wide = {db.read[users], clock.read}\n\
         fn handler() -> Int / {Wide} = db.all[users]()\n\
         test \"reaches it\" {\n\
         \x20 handle { assert_eq(handler(), 1) } with { db.all[users]() -> 1 }\n\
         }\n",
    ));
    let text = text_of(only(&diags, codes::NONDET_IN_DET_TEST));
    assert!(
        text.contains("clock"),
        "the determinism refusal names the effect: {text}"
    );
    assert!(
        !text.contains("Wide"),
        "the verdict is over atoms, never over a name: {text}"
    );
}

/// And handling the atom discharges it, alias or no alias — the set is not a
/// second thing that has to be discharged.
#[test]
fn handling_the_atom_a_set_carries_discharges_it() {
    let out = ok(&program(
        "effect set Wide = {db.read[users], clock.read}\n\
         fn handler() -> Int / {Wide} { let t = clock.now(); db.all[users]() + t }\n\
         test \"handled\" {\n\
         \x20 handle { assert_eq(handler(), 3) } with {\n\
         \x20   db.all[users]() -> 1,\n\
         \x20   clock.now() -> 2,\n\
         \x20 }\n\
         }\n",
    ));
    assert_eq!(out.tests.len(), 1);
    assert_eq!(out.tests[0].footprint.to_string(), "{}");
}

// --- 4. a refused set contributes nothing, and says so ----------------------

/// A cycle is `E0115` and nothing else: no second, misleading `E0302` produced
/// by quietly expanding the cyclic set to nothing and then measuring the body
/// against it.
#[test]
fn a_cyclic_set_is_one_refusal_and_not_a_silently_empty_bound() {
    let diags = errors(&program(
        "effect set A = {B, db.read[users]}\n\
         effect set B = {A, log.write}\n\
         fn handler() -> Int / {A} { log.line(1); db.all[users]() }\n",
    ));
    assert_eq!(
        diags
            .iter()
            .filter(|d| d.code == codes::EFFECT_SET_CYCLE)
            .count(),
        1,
        "one cycle is one report: {diags:#?}"
    );
    assert!(
        !diags.iter().any(|d| d.code == codes::EFFECT_NOT_PERMITTED),
        "a refused set must not also produce an upper-bound refusal against an \
         expansion nobody wrote: {diags:#?}"
    );
}

/// Two independent cycles are two reports, so a file with several of them tells
/// a reader about all of them in one run.
#[test]
fn two_disjoint_cycles_are_two_refusals() {
    let diags = errors(&program(
        "effect set A = {B}\n\
         effect set B = {A}\n\
         effect set C = {D}\n\
         effect set D = {C}\n\
         fn handler() -> Int / {A, C} = 1\n",
    ));
    assert_eq!(
        diags
            .iter()
            .filter(|d| d.code == codes::EFFECT_SET_CYCLE)
            .count(),
        2,
        "{diags:#?}"
    );
}

/// A set that merely *reaches* a cycle is refused with the cycle rather than
/// expanded to a partial answer. Anything else would publish a row narrower
/// than the atoms the author wrote down.
#[test]
fn a_set_that_reaches_a_cycle_does_not_publish_a_partial_expansion() {
    let diags = errors(&program(
        "effect set Loop = {Loop}\n\
         effect set Outer = {Loop, db.read[users]}\n\
         fn handler() -> Int / {Outer} { log.line(1); db.all[users]() }\n",
    ));
    only(&diags, codes::EFFECT_SET_CYCLE);
    // `log.write` is outside anything written, so if `Outer` were expanded at
    // all the body would be measured against a bound the author did not write.
    // The cycle is the fault, and it is the only one reported.
    assert!(
        !diags.iter().any(|d| d.code == codes::EFFECT_NOT_PERMITTED),
        "{diags:#?}"
    );
}

/// A set naming a set that does not exist is `E0114` at the *member*, so the
/// fix is at the declaration rather than at each row that named the outer set.
#[test]
fn a_member_naming_an_undeclared_set_is_e0114_once() {
    let diags = errors(&program(
        "effect set Web = {Missing, db.read[users]}\n\
         fn a() -> Int / {Web} = 1\n\
         fn b() -> Int / {Web} = 2\n",
    ));
    assert_eq!(
        diags
            .iter()
            .filter(|d| d.code == codes::UNKNOWN_EFFECT_SET)
            .count(),
        1,
        "one bad member is one report however many rows reach it: {diags:#?}"
    );
}

/// A duplicate set name is `E0105` and the file does not compile, so there is
/// no run in which one of two spellings of `Web` silently won.
#[test]
fn a_duplicate_set_is_refused_rather_than_resolved() {
    let diags = errors(&program(
        "effect set Web = {db.read[users]}\n\
         effect set Web = {db.read[users], log.write}\n\
         fn handler() -> Int / {Web} { log.line(1); db.all[users]() }\n",
    ));
    assert_eq!(
        diags
            .iter()
            .filter(|d| d.code == codes::DUPLICATE_DEFINITION)
            .count(),
        1,
        "{diags:#?}"
    );
    assert!(
        !diags.iter().any(|d| d.code == codes::UNKNOWN_EFFECT_SET),
        "the second declaration is a duplicate, not a missing set: {diags:#?}"
    );
}

/// The module-local rule, from the other side: a set declared in one module is
/// not in scope in another, and the refusal says why rather than merely that
/// the name was not found.
#[test]
fn a_set_is_not_reachable_from_another_module() {
    let inputs = [
        (
            SourceId(0),
            ModuleName::from_dotted("a"),
            "pub effect db {\n  read all[t]() -> Int\n}\n",
        ),
        (
            SourceId(1),
            ModuleName::from_dotted("b"),
            "import a\nfn f() -> Int / {a::Web} = a::db.all[users]()\n",
        ),
    ];
    let diags =
        ply_syntax::parse_program(inputs).expect_err("a qualified set reference must be refused");
    let text = text_of(only(&diags, codes::UNKNOWN_EFFECT_SET));
    assert!(
        text.contains("module-local"),
        "the refusal must carry the rule, not just the miss: {text}"
    );
}
