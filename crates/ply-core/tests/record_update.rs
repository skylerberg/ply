//! What `{..b, f: e}` does to a check, at the source level.
//!
//! Three claims, and the first is the reason the feature is shaped the way it
//! is (`docs/adr/0023-record-update.md`):
//!
//! 1. **There is no typing rule for a record update.** By the time inference
//!    runs, `{..s, a: 1}` *is* the literal that copies `s`'s other fields, so it
//!    meets the same exact-key-set unification (`crates/ply-core/src/unify.rs`)
//!    every record literal meets. The update's type is the base's type because
//!    the expansion emits the base's field set, not because a rule says so.
//! 2. **A record update replaces; it never widens.** `E0117` at the syntax
//!    level, before inference sees anything.
//! 3. **A shape the file cannot name is refused, never guessed** — `E0116`.
//!    Expansion reads this module's own `type` items and the annotations written
//!    in this file, for the reason `effect set` expansion does.

use ply_core::{CheckOutput, check_program, print_scheme};
use ply_span::{Diagnostic, SourceId, Symbol, codes};
use ply_syntax::ast::ModuleName;
use ply_syntax::resolve::resolve;

fn compile(source: &str) -> Result<CheckOutput, Vec<Diagnostic>> {
    let inputs = vec![(SourceId(0), ModuleName::from_dotted("m"), source)];
    let program = ply_syntax::parse_program(inputs)?;
    let resolved = resolve(&program)?;
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

#[track_caller]
fn only<'a>(diags: &'a [Diagnostic], code: &str) -> &'a Diagnostic {
    diags
        .iter()
        .find(|d| d.code == code)
        .unwrap_or_else(|| panic!("no `{code}` in {diags:#?}"))
}

fn scheme(out: &CheckOutput, name: &str) -> String {
    print_scheme(&out.defs[&Symbol::new(name)].scheme)
}

const DECLS: &str = "type L = {a: Int, b: Int, c: Int}\ntype W = {lim: L, n: Int}\n";

/// The claim the brief states as the feature's job: **the update's type is the
/// base's type.** Checked by giving the function the base's type as its return
/// annotation and letting exact-key-set unification do the work.
#[test]
fn an_updates_type_is_its_bases_type() {
    let out = ok(&format!(
        "{DECLS}fn f(s: L) -> L = {{..s, b: 1}}\n\
         fn g(w: W) -> L = {{..w.lim, a: 1}}\n\
         fn h(s: L) -> L = {{..s}}\n"
    ));
    for name in ["m.f", "m.g", "m.h"] {
        assert!(
            scheme(&out, name).ends_with("{a: Int, b: Int, c: Int}"),
            "{name}: {}",
            scheme(&out, name)
        );
    }
}

/// The width is not taken on trust. A result annotated narrower than the base is
/// a mismatch, which is what makes a wrong expansion a diagnostic rather than a
/// wrong record: `unify.rs` compares key sets exactly and Ply has no width
/// subtyping.
#[test]
fn a_narrower_result_annotation_is_a_mismatch() {
    let diags = errors(&format!(
        "{DECLS}fn f(s: L) -> {{a: Int, b: Int}} = {{..s, b: 1}}\n"
    ));
    only(&diags, codes::TYPE_MISMATCH);
}

/// A replacement value is checked against the field it replaces, exactly as it
/// would be in the longhand.
#[test]
fn a_replacement_of_the_wrong_type_is_a_mismatch() {
    let diags = errors(&format!("{DECLS}fn f(s: L) -> L = {{..s, b: \"x\"}}\n"));
    only(&diags, codes::TYPE_MISMATCH);
}

/// The twelve fields `chunk_trailers` stops writing cannot be mispaired, because
/// they are not spelled. This is the whole safety claim of the rewrite, reduced
/// to three fields: the longhand admits the swap and the update cannot express
/// it.
#[test]
fn the_longhand_admits_a_mispairing_the_update_cannot_express() {
    // All three fields are `Int`, so swapping two of them type-checks and is
    // silently wrong — the defect record update removes structurally.
    ok(&format!(
        "{DECLS}fn swapped(s: L) -> L = {{a: s.c, b: 1, c: s.a}}\n"
    ));
    let out = ok(&format!("{DECLS}fn updated(s: L) -> L = {{..s, b: 1}}\n"));
    assert!(scheme(&out, "m.updated").ends_with("{a: Int, b: Int, c: Int}"));
}

#[test]
fn adding_a_field_is_e0117() {
    let diags = errors(&format!("{DECLS}fn f(s: L) -> L = {{..s, z: 1}}\n"));
    let d = only(&diags, codes::RECORD_UPDATE_FIELD);
    assert!(
        d.notes.iter().any(|n| n.contains("`a`, `b`, `c`")),
        "{d:#?}"
    );
}

#[test]
fn a_base_with_no_nameable_shape_is_e0116() {
    for (source, note) in [
        (
            format!("{DECLS}fn f(n: Int) -> Int = {{ {{..n, a: 1}}; 0 }}\n"),
            "not a `type` declared in this file",
        ),
        (
            "type S = X | Y\nfn f(s: S) -> Int = { {..s, a: 1}; 0 }\n".to_string(),
            "is a sum type",
        ),
        (
            format!("{DECLS}fn f(s) -> Int = {{ {{..s, a: 1}}; 0 }}\n"),
            "no written type here",
        ),
        (
            format!("{DECLS}fn f() -> Int = {{ {{..missing, a: 1}}; 0 }}\n"),
            "not a local binder with a written type",
        ),
    ] {
        let diags = errors(&source);
        let d = only(&diags, codes::RECORD_UPDATE_SHAPE);
        assert!(
            d.notes.iter().any(|n| n.contains(note)),
            "expected a note about {note:?} in {d:#?}"
        );
    }
}

/// The module-local restriction, stated as a test rather than as prose. This is
/// the cost ADR 0023 §4 records: the stdlib gets the win at its own definition
/// sites and an importing file does not, and it is what keeps ADR 0002's gate 1
/// sound.
#[test]
fn a_shape_declared_in_another_module_is_refused() {
    let inputs = vec![
        (
            SourceId(0),
            ModuleName::from_dotted("lib"),
            "pub type L = {a: Int, b: Int}\npub fn zero() -> L = {a: 0, b: 0}\n",
        ),
        (
            SourceId(1),
            ModuleName::from_dotted("app"),
            "import lib\nfn f(s: lib::L) -> lib::L = {..s, a: 1}\n",
        ),
    ];
    let diags = ply_syntax::parse_program(inputs).expect_err("the update has no shape here");
    let d = only(&diags, codes::RECORD_UPDATE_SHAPE);
    assert!(
        d.notes.iter().any(|n| n.contains("module boundary")),
        "{d:#?}"
    );
}

/// The fixtures `tests/fixtures/` owes for the two new codes, checked here
/// rather than left as files nothing reads.
#[test]
fn the_fixtures_produce_the_codes_they_are_named_for() {
    for (path, code) in [
        ("record_update_shape.ply", codes::RECORD_UPDATE_SHAPE),
        ("record_update_field.ply", codes::RECORD_UPDATE_FIELD),
    ] {
        let full = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/").to_string() + path;
        let source =
            std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("could not read {full}: {e}"));
        let diags = errors(&source);
        only(&diags, code);
    }
}
