use crate::fixture::compile;
use ply_core::CheckOutput;
use ply_span::{Diagnostic, SourceId, Symbol, codes};
use ply_syntax::ast::ModuleName;

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

#[test]
fn a_set_that_reaches_a_cycle_does_not_publish_a_partial_expansion() {
    let diags = errors(&program(
        "effect set Loop = {Loop}\n\
         effect set Outer = {Loop, db.read[users]}\n\
         fn handler() -> Int / {Outer} { log.line(1); db.all[users]() }\n",
    ));
    only(&diags, codes::EFFECT_SET_CYCLE);
    // `log.write` is outside every written bound, so any expansion of `Outer` would refuse it.
    assert!(
        !diags.iter().any(|d| d.code == codes::EFFECT_NOT_PERMITTED),
        "{diags:#?}"
    );
}

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
