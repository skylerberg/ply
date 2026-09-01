//! `cell_update` and `map_update` at the type surface: the fused updates ADR 0024 specified and
//! the cost checker recommends, typed as the third cell call form and as an ordinary
//! row-polymorphic builtin respectively.

use crate::fixture::compile;
use ply_core::{CheckOutput, print_type};
use ply_span::{Diagnostic, Symbol, codes};

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

/// Both atoms the update performs are discharged at the region that owns the cell, exactly as
/// `cell_get` and `cell_set` are.
#[test]
fn cell_update_types_as_unit_and_both_of_its_atoms_are_discharged_at_the_region() {
    let out = ok(
        "fn go() -> Int = with_cell[r]([1]) { c -> { cell_update(c, |xs| push(xs, 2)); \
                  len(cell_get(c)) } }\n",
    );
    assert_eq!(sig(&out, "go"), "() -> Int");
    assert!(
        out.defs[&Symbol::new("m.go")].footprint.is_empty(),
        "the region discharges the update's atoms: {}",
        footprint(&out, "go")
    );
}

/// What the function performs, the update performs.
#[test]
fn the_functions_row_flows_into_the_update() {
    let out = ok("effect tick {\n  write beat() -> Unit\n}\n\
                  fn go() -> Int = with_cell[r](0) { c -> { cell_update(c, |n| { tick.beat(); \
                  n + 1 }); cell_get(c) } }\n");
    assert!(
        footprint(&out, "go").contains("tick"),
        "the callback's effect reaches the definition's row: {}",
        footprint(&out, "go")
    );
}

#[test]
fn the_function_must_take_and_answer_the_cells_element_type() {
    let d = errors("fn go() -> Unit = with_cell[r](0) { c -> cell_update(c, |n| \"x\") }\n");
    assert!(d.iter().any(|d| d.code == codes::TYPE_MISMATCH), "{d:#?}");
}

/// The same rule as `cell_get` / `cell_set`: the atom names the region of the argument, so the
/// form has no value to be.
#[test]
fn cell_update_used_as_a_value_is_refused() {
    let d = errors("fn go() -> Int = { let f = cell_update; 1 }\n");
    assert!(
        d.iter().any(|d| d.code == codes::RESOURCE_REQUIRED),
        "{d:#?}"
    );
}

#[test]
fn a_module_cannot_redefine_cell_update() {
    let d = errors("fn cell_update(x: Int) -> Int = x\n");
    assert!(
        d.iter().any(|d| d.code == codes::DUPLICATE_DEFINITION),
        "{d:#?}"
    );
}

/// `fn probe() -> <the contract's type> = map_update` returns the builtin itself, so the probe's
/// own signature carries the builtin's whole type.
#[test]
fn map_update_has_the_type_the_contract_states() {
    let want = "(Map<a, b>, a, (b) -> b / e) -> Map<a, b> / e";
    let source = format!("fn probe<a, b | e>() -> {want} where derivable(ord, a) = map_update\n");
    let out = ok(&source);
    assert_eq!(sig(&out, "probe"), format!("() -> {want}"));
}
