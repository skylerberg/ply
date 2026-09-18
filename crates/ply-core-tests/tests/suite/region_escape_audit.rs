use crate::fixture::expanded;
use ply_span::{Diagnostic, codes};

#[track_caller]
fn errors(source: &str) -> Vec<Diagnostic> {
    match expanded(source) {
        Ok(_) => panic!("this escapes its region and was accepted:\n{source}"),
        Err(d) => d,
    }
}

#[track_caller]
fn code(source: &str, want: &str) -> Diagnostic {
    let diags = errors(source);
    match diags.iter().find(|d| d.code == want) {
        Some(d) => d.clone(),
        None => panic!("expected {want} from:\n{source}\ngot {diags:#?}"),
    }
}

fn says(d: &Diagnostic, text: &str) -> bool {
    d.message.contains(text)
        || d.notes.iter().any(|n| n.contains(text))
        || d.labels.iter().any(|l| l.message.contains(text))
}

#[track_caller]
fn names(d: &Diagnostic, text: &str) {
    assert!(
        says(d, text),
        "the diagnostic does not say `{text}`: {d:#?}"
    );
}

#[test]
fn storing_through_a_closure_bound_before_the_region_is_an_escape() {
    let d = code(
        r#"fn leak() -> Int = with_cell[k](panic("seed")) { slot ->
  { let store = |x| cell_set(slot, x);
    with_region[r] { with_cell[r](0) { c -> { store(c); 1 } } } }
}"#,
        codes::REGION_ESCAPE,
    );
    names(&d, "`store`");
    names(&d, "`r`");
}

/// The outer cell is one field access away, which a check of bare cell-typed bindings would miss.
#[test]
fn storing_through_a_record_field_holding_the_outer_cell_is_an_escape() {
    let d = code(
        r#"fn leak() -> Int = with_cell[k](panic("seed")) { slot ->
  { let holder = {slot: slot};
    with_region[r] { with_cell[r](0) { c -> { cell_set(holder.slot, c); 1 } } } }
}"#,
        codes::REGION_ESCAPE,
    );
    names(&d, "`r`");
}

#[test]
fn storing_from_a_lambda_written_inside_the_region_is_an_escape() {
    let d = code(
        r#"fn leak() -> Int = with_cell[k](panic("seed")) { slot ->
  with_region[r] {
    with_cell[r](0) { c -> { let f = || cell_set(slot, c); f(); 1 } }
  }
}"#,
        codes::REGION_ESCAPE,
    );
    names(&d, "`slot`");
}

#[test]
fn a_chain_of_type_aliases_does_not_hide_a_cell_in_an_operation() {
    let d = code(
        "type A = Cell<Int>
type B = A
effect sink { write put(x: B) -> Unit }",
        codes::REGION_ESCAPE,
    );
    names(&d, "put");
}

/// The `Cell` is the alias's argument, so nothing written is spelled `Cell` at its head.
#[test]
fn a_generic_alias_does_not_hide_a_cell_in_an_operation() {
    let d = code(
        "type Boxed<t> = List<t>
effect sink { write put(x: Boxed<Cell<Int>>) -> Unit }",
        codes::REGION_ESCAPE,
    );
    names(&d, "put");
}

#[test]
fn a_record_alias_does_not_hide_a_cell_in_a_variant_field() {
    let d = code(
        "type Rec = { held: Cell<Int> }
type H = | Held(Rec)",
        codes::REGION_ESCAPE,
    );
    names(&d, "Held");
}

#[test]
fn a_brand_survives_a_mutually_recursive_pair() {
    let d = code(
        "fn one<t>(x: t) -> t = two(x)
fn two<t>(x: t) -> t = x
fn leak() = with_region[r] { with_cell[r](0) { c -> one(c) } }",
        codes::REGION_ESCAPE,
    );
    names(&d, "Cell[r]<Int>");
}

#[test]
fn a_region_inside_a_handle_still_reports_its_escape() {
    let d = code(
        "effect ask { read get() -> Int }
fn leak() = handle { with_region[r] { with_cell[r](0) { c -> c } } } with { return x -> x }",
        codes::REGION_ESCAPE,
    );
    names(&d, "Cell[r]<Int>");
}

#[test]
fn a_continuation_handed_to_a_generic_operation_is_an_escape() {
    let d = code(
        "effect ask { read get() -> Int }
effect sink { write put(x: a) -> Unit }
fn leak() -> Int / {sink.write} = with_region[r] {
  with_cell[r](0) { c ->
    handle { cell_set(c, ask.get()); cell_get(c) } with {
      ask.get() resume j -> { sink.put(j); 0 }
    }
  }
}",
        codes::REGION_ESCAPE,
    );
    names(&d, "sink.put");
    names(&d, "cell.read[r]");
}

#[test]
fn a_region_in_a_test_block_reports_its_escape() {
    let d = code(
        "test \"leak\" { with_region[r] { with_cell[r](0) { c -> c } } }",
        codes::REGION_ESCAPE,
    );
    names(&d, "Cell[r]<Int>");
}

#[test]
fn two_definitions_spelling_one_region_name_do_not_share_a_brand() {
    let diags = errors(
        r#"effect sink { write put(x: a) -> Unit }
fn produce() -> Int / {sink.write} = with_cell[k](7) { c -> { sink.put(c); 0 } }
fn consume() -> Int = with_cell[k](0) { other ->
  handle { produce() } with {
    sink.put(x) resume j -> { j(()); cell_get(x) },
    return n -> n
  }
}"#,
    );
    assert!(
        diags.iter().any(|d| d.code == codes::RESOURCE_REQUIRED),
        "the foreign cell must not be readable through a same-named region: {diags:#?}"
    );
}

#[test]
fn only_a_written_row_can_put_a_foreign_regions_atom_in_a_callers_row() {
    let d = code(
        "fn writer() = with_cell[k](0) { c -> {get: || cell_get(c), set: |v| cell_set(c, v)} }",
        codes::TYPE_MISMATCH,
    );
    names(&d, "escapes its `with_cell[k]` region");

    // What remains is an annotation, which a region named `k` discharges only inside itself.
    let out = expanded(
        "fn touches(n: Int) -> Int / {cell.read[k]} = n
fn outside() -> Int / {cell.read[k]} = { let seen = with_cell[k](0) { c -> cell_get(c) }; touches(seen) }
fn inside() -> Int = with_cell[k](0) { c -> touches(cell_get(c)) }",
    )
    .expect("both check");
    let footprint = |name: &str| {
        out.defs[&ply_span::Symbol::new(format!("m.{name}"))]
            .footprint
            .atoms()
            .map(|a| a.to_string())
            .collect::<Vec<String>>()
    };
    assert_eq!(footprint("outside"), vec!["cell.read[k]".to_string()]);
    assert!(
        footprint("inside").is_empty(),
        "a region named `k` discharges `cell.read[k]`, which is what the brand being the name \
         means: {:?}",
        footprint("inside")
    );
}

#[test]
fn a_closure_capturing_a_bare_with_cells_cell_is_an_escape() {
    let d = code(
        "fn leak() = with_cell[k](0) { c -> || cell_get(c) }",
        codes::TYPE_MISMATCH,
    );
    names(&d, "escapes its `with_cell[k]` region");
}

#[test]
fn a_reader_and_writer_pair_over_a_bare_with_cells_cell_is_an_escape() {
    let d = code(
        "fn leak() = with_cell[k](0) { c -> {get: || cell_get(c), set: |v| cell_set(c, v)} }",
        codes::TYPE_MISMATCH,
    );
    names(&d, "escapes its `with_cell[k]` region");
}

#[test]
fn a_bare_with_cells_cell_handed_to_a_generic_operation_is_an_escape() {
    let d = code(
        "effect sink { write put(x: a) -> Unit }
fn leak() -> Int / {sink.write} = with_cell[k](0) { c -> { sink.put(c); 0 } }",
        codes::REGION_ESCAPE,
    );
    names(&d, "sink.put");
}

#[test]
fn a_bare_with_cells_cell_stored_into_an_enclosing_cell_is_an_escape() {
    let d = code(
        r#"fn leak() -> Int = with_cell[outer](panic("seed")) { o ->
  { with_cell[inner](0) { i -> cell_set(o, [i]) }; 0 }
}"#,
        codes::REGION_ESCAPE,
    );
    names(&d, "`inner`");
}
