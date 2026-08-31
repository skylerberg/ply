//! What an `effect set` does to a check, at the source level.
//!
//! Three claims, from ADR 0009 and ADR 0013 §1.5:
//!
//! 1. **Annotation-only.** Inference still produces the precise atom set. An
//!    alias buys legibility and gives up no precision, so `DefInfo::performed`
//!    is what the body reaches and `DefInfo::footprint` is what the signature
//!    published — and everything downstream reads atoms, never a name.
//! 2. **Checked as an upper bound, exactly as a written row already is.** There
//!    is one code path and it is the existing one, so a violation names the
//!    atoms that were not permitted and quotes the *expansion*.
//! 3. **A set name reaches no namespace.** Expansion has erased it before
//!    `resolve` runs, so `effect set Web` beside `type Web` is two things in two
//!    positions.

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

fn only<'a>(diags: &'a [Diagnostic], code: &str) -> &'a Diagnostic {
    diags
        .iter()
        .find(|d| d.code == code)
        .unwrap_or_else(|| panic!("no `{code}` in {diags:#?}"))
}

fn published(out: &CheckOutput, name: &str) -> String {
    out.defs[&Symbol::new(name)].footprint.to_string()
}

fn performed(out: &CheckOutput, name: &str) -> String {
    out.defs[&Symbol::new(name)].performed.to_string()
}

fn aliases(out: &CheckOutput, name: &str) -> Vec<String> {
    out.defs[&Symbol::new(name)]
        .row_aliases
        .iter()
        .map(|a| a.to_string())
        .collect()
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

// --- 1. annotation-only -----------------------------------------------------

/// The published row is the expansion, and the *precise* row the body reaches
/// is kept beside it. Nothing collapses one into the other.
#[test]
fn an_alias_publishes_its_expansion_and_keeps_the_precise_row() {
    let out = ok(&program(
        "effect set Web = {db.read[users], db.write[orders], log.write}\n\
         fn handler() -> Int / {Web} = db.all[users]()\n",
    ));
    assert_eq!(
        published(&out, "m.handler"),
        "{m.db.write[orders], m.db.read[users], m.log.write}"
    );
    assert_eq!(performed(&out, "m.handler"), "{m.db.read[users]}");
    assert_eq!(aliases(&out, "m.handler"), ["Web"]);
}

/// An unannotated definition publishes what it performs, so the two agree and
/// the alias is the only thing that can separate them.
#[test]
fn without_an_annotation_the_published_row_is_the_precise_one() {
    let out = ok(&program("fn handler() -> Int = db.all[users]()\n"));
    assert_eq!(published(&out, "m.handler"), "{m.db.read[users]}");
    assert_eq!(performed(&out, "m.handler"), "{m.db.read[users]}");
    assert!(aliases(&out, "m.handler").is_empty());
}

/// A caller's row is inferred from the callee's *published* row, which is the
/// expansion — atoms, never a name. This is the sentence "nothing downstream
/// ever sees the alias" as a value a test can read.
#[test]
fn a_caller_of_an_aliased_definition_infers_atoms() {
    let out = ok(&program(
        "effect set Web = {db.read[users], log.write}\n\
         fn handler() -> Int / {Web} = db.all[users]()\n\
         fn caller() -> Int = handler()\n",
    ));
    assert_eq!(
        published(&out, "m.caller"),
        "{m.db.read[users], m.log.write}"
    );
}

/// A set's atoms reach the determinism check as atoms: `E0412` fires for a
/// `nondet` effect a `det` test can reach through an aliased signature, exactly
/// as it would for one written out.
#[test]
fn a_nondet_atom_reached_through_an_alias_still_fires_e0412() {
    let diags = errors(&program(
        "effect set Web = {clock.read}\n\
         fn handler() -> Int / {Web} = clock.now()\n\
         test \"t\" { assert_eq(handler(), 0) }\n",
    ));
    only(&diags, codes::NONDET_IN_DET_TEST);
}

// --- 2. checked as an upper bound -------------------------------------------

#[test]
fn a_body_inside_the_expansion_checks() {
    let out = ok(&program(
        "effect set Web = {db.read[users], db.write[orders], log.write}\n\
         fn handler() -> Int / {Web} { db.save[orders](1); db.all[users]() }\n",
    ));
    assert_eq!(
        performed(&out, "m.handler"),
        "{m.db.write[orders], m.db.read[users]}"
    );
    assert_eq!(
        published(&out, "m.handler"),
        "{m.db.write[orders], m.db.read[users], m.log.write}"
    );
}

/// The violation names the atoms that were not permitted, and the secondary
/// label quotes the **expansion** — never the alias, which is not what the body
/// failed to satisfy.
#[test]
fn a_body_outside_the_expansion_is_e0302_quoting_the_expansion() {
    let diags = errors(&program(
        "effect set Web = {db.read[users], log.write}\n\
         fn handler() -> Int / {Web} { db.save[orders](1); db.all[users]() }\n",
    ));
    let d = only(&diags, codes::EFFECT_NOT_PERMITTED);
    assert!(
        d.message.contains("m.db.write[orders]"),
        "the atoms that were not permitted are named: {d:#?}"
    );
    let secondary: Vec<&str> = d
        .labels
        .iter()
        .filter(|l| !l.primary)
        .map(|l| l.message.as_str())
        .collect();
    let quoted = secondary.join(" ");
    assert!(
        quoted.contains("{m.db.read[users], m.log.write}"),
        "the declared row is quoted as its expansion: {quoted}"
    );
    assert!(
        !quoted.contains("Web"),
        "the alias name is never what a body failed to satisfy: {quoted}"
    );
}

/// A nested set is one upper bound, not two: the inner set's atoms are as
/// permitted as the outer set's own.
#[test]
fn a_nested_set_bounds_the_body_by_its_transitive_expansion() {
    let out = ok(&program(
        "effect set Storage = {db.read[users], db.write[orders]}\n\
         effect set Web = {Storage, log.write}\n\
         fn handler() -> Int / {Web} { log.line(1); db.all[users]() }\n",
    ));
    assert_eq!(
        published(&out, "m.handler"),
        "{m.db.write[orders], m.db.read[users], m.log.write}"
    );
    assert_eq!(
        performed(&out, "m.handler"),
        "{m.db.read[users], m.log.write}"
    );
}

#[test]
fn an_atom_only_the_inner_set_omits_is_still_refused() {
    let diags = errors(&program(
        "effect set Storage = {db.read[users]}\n\
         effect set Web = {Storage, log.write}\n\
         fn handler() -> Int { db.save[orders](1); db.all[users]() }\n\
         fn bounded() -> Int / {Web} = handler()\n",
    ));
    let d = only(&diags, codes::EFFECT_NOT_PERMITTED);
    assert!(d.message.contains("m.db.write[orders]"), "{d:#?}");
}

/// A set beside written atoms and a row variable: the alias contributes atoms
/// and nothing else, so the tail keeps the meaning it had.
#[test]
fn a_set_composes_with_written_atoms_and_a_row_variable() {
    let out = ok(&program(
        "effect set Web = {db.read[users]}\n\
         fn run<a | e>(f: () -> a / e) -> a / {Web, log.write | e} { log.line(1); f() }\n",
    ));
    assert!(published(&out, "m.run").contains("m.db.read[users]"));
    assert!(published(&out, "m.run").contains("m.log.write"));
}

// --- 3. the diagnostics -----------------------------------------------------

#[test]
fn a_row_naming_an_undeclared_set_is_e0114() {
    let diags = errors(&program("fn handler() -> Int / {Web} = 1\n"));
    only(&diags, codes::UNKNOWN_EFFECT_SET);
}

#[test]
fn a_qualified_set_reference_is_e0114() {
    let diags = errors(&program("fn handler() -> Int / {shared::Web} = 1\n"));
    only(&diags, codes::UNKNOWN_EFFECT_SET);
}

#[test]
fn a_pub_set_is_e0114() {
    let diags = errors(&program(
        "pub effect set Web = {log.write}\nfn handler() -> Int / {Web} = 1\n",
    ));
    only(&diags, codes::UNKNOWN_EFFECT_SET);
}

/// A member is an atom or another set, never a whole effect: "every atom of
/// `db`" is every resource label anywhere in the program, and an unrelated
/// table in an unrelated module would then move this definition's hash.
#[test]
fn a_member_naming_a_whole_effect_is_e0114() {
    let diags = errors(&program(
        "effect set Web = {db, log.write}\nfn handler() -> Int / {Web} = 1\n",
    ));
    let d = only(&diags, codes::UNKNOWN_EFFECT_SET);
    assert!(
        d.notes.iter().any(|n| n.contains("an atom")),
        "the fix is stated: {d:#?}"
    );
}

/// A set is expanded by the parser, so one bad member reaches every row that
/// names it. It is reported once, at the one place there is to fix.
#[test]
fn a_bad_atom_in_a_set_is_reported_once_however_many_rows_name_it() {
    let diags = errors(&program(
        "effect set Web = {log.read}\n\
         fn a() -> Int / {Web} = 1\n\
         fn b() -> Int / {Web} = 2\n\
         fn c() -> Int / {Web} = 3\n",
    ));
    let refusals = diags
        .iter()
        .filter(|d| d.code == codes::UNKNOWN_OPERATION)
        .count();
    assert_eq!(refusals, 1, "{diags:#?}");
}

#[test]
fn a_set_containing_itself_is_e0115() {
    let diags = errors(&program(
        "effect set Web = {Web}\nfn handler() -> Int / {Web} = 1\n",
    ));
    only(&diags, codes::EFFECT_SET_CYCLE);
}

#[test]
fn a_cycle_through_two_more_sets_is_e0115_naming_it_in_order() {
    let diags = errors(&program(
        "effect set A = {B}\n\
         effect set B = {C}\n\
         effect set C = {A}\n\
         fn handler() -> Int / {A} = 1\n",
    ));
    let d = only(&diags, codes::EFFECT_SET_CYCLE);
    assert!(
        d.notes
            .iter()
            .any(|n| n.contains("`A` -> `B` -> `C` -> `A`")),
        "{d:#?}"
    );
}

#[test]
fn two_sets_with_one_name_are_e0105() {
    let diags = errors(&program(
        "effect set Web = {log.write}\n\
         effect set Web = {db.read[users]}\n\
         fn handler() -> Int / {Web} = 1\n",
    ));
    only(&diags, codes::DUPLICATE_DEFINITION);
}

/// Expansion has erased the set's name before `resolve` runs, so it lives in no
/// namespace `resolve` knows about and collides with nothing.
#[test]
fn a_set_and_a_type_may_share_a_name() {
    let out = ok(&program(
        "type Web = Int\n\
         effect set Web = {log.write}\n\
         fn handler(x: Web) -> Web / {Web} = x\n",
    ));
    assert_eq!(published(&out, "m.handler"), "{m.log.write}");
}

/// The fixtures `tests/fixtures/` owes for the two new codes, checked here
/// rather than left as files nothing reads.
#[test]
fn the_fixtures_produce_the_codes_they_are_named_for() {
    for (path, code) in [
        ("unknown_effect_set.ply", codes::UNKNOWN_EFFECT_SET),
        ("effect_set_cycle.ply", codes::EFFECT_SET_CYCLE),
    ] {
        let full = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/").to_string() + path;
        let source =
            std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("could not read {full}: {e}"));
        let diags = errors(&source);
        only(&diags, code);
    }
}
