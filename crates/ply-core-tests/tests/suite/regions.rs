use crate::fixture::expanded;
use ply_core::CheckOutput;
use ply_span::{Diagnostic, codes};

fn ok(source: &str) -> CheckOutput {
    match expanded(source) {
        Ok(out) => out,
        Err(d) => panic!("expected this to check:\n{source}\ngot {d:#?}"),
    }
}

fn errors(source: &str) -> Vec<Diagnostic> {
    match expanded(source) {
        Ok(_) => panic!("expected a diagnostic from:\n{source}"),
        Err(d) => d,
    }
}

fn code(source: &str, code: &str) -> Diagnostic {
    let diags = errors(source);
    match diags.iter().find(|d| d.code == code) {
        Some(d) => d.clone(),
        None => panic!("expected {code} from:\n{source}\ngot {diags:#?}"),
    }
}

fn says(d: &Diagnostic, text: &str) -> bool {
    d.message.contains(text)
        || d.notes.iter().any(|n| n.contains(text))
        || d.labels.iter().any(|l| l.message.contains(text))
}

fn names_value_and_region(d: &Diagnostic, ty: &str, region: &str) {
    assert!(
        says(d, ty),
        "the diagnostic does not show the escaping type `{ty}`: {d:#?}"
    );
    assert!(
        says(d, &format!("`{region}`")),
        "the diagnostic does not name region `{region}`: {d:#?}"
    );
}

#[test]
fn a_region_that_answers_with_a_plain_value_checks_and_discharges_its_cells() {
    let out = ok("fn total() -> Int = with_region[r] {
  with_cell[r](0) { c -> { cell_set(c, 7); cell_get(c) } }
}");
    let def = &out.defs[&ply_span::Symbol::new("m.total")];
    assert!(
        def.footprint.is_empty(),
        "the region discharges the cell's atoms: {:?}",
        def.footprint
    );
}

#[test]
fn a_region_with_no_cell_in_it_is_just_its_body() {
    ok("fn plain() -> Int = with_region[r] { 1 + 2 }");
}

#[test]
fn an_inner_region_may_read_an_outer_regions_cell() {
    ok("fn nested() -> Int = with_region[outer] {
  with_cell[outer](1) { c ->
    with_region[inner] {
      with_cell[inner](2) { d -> cell_get(c) + cell_get(d) }
    }
  }
}");
}

#[test]
fn an_inner_regions_value_may_be_read_and_its_reading_returned() {
    ok("fn nested() -> Int = with_region[outer] {
  with_cell[outer](0) { c ->
    {
      cell_set(c, with_region[inner] { with_cell[inner](5) { d -> cell_get(d) } });
      cell_get(c)
    }
  }
}");
}

#[test]
fn a_cell_may_outlive_its_with_cell_when_a_region_of_that_name_is_open() {
    ok("fn hold() -> Int = with_region[r] {
  cell_get(with_cell[r](3) { c -> c })
}");
}

#[test]
fn returning_the_cell_is_an_escape() {
    let d = code(
        "fn leak() = with_region[r] { with_cell[r](0) { c -> c } }",
        codes::REGION_ESCAPE,
    );
    names_value_and_region(&d, "Cell[r]<Int>", "r");
}

#[test]
fn a_cell_in_a_record_field_is_an_escape() {
    let d = code(
        "fn leak() = with_region[r] { with_cell[r](0) { c -> {held: c, n: 1} } }",
        codes::REGION_ESCAPE,
    );
    names_value_and_region(&d, "Cell[r]<Int>", "r");
}

#[test]
fn a_cell_in_a_list_element_is_an_escape() {
    let d = code(
        "fn leak() = with_region[r] { with_cell[r](0) { c -> [c] } }",
        codes::REGION_ESCAPE,
    );
    names_value_and_region(&d, "List<Cell[r]<Int>>", "r");
}

#[test]
fn a_cell_as_a_map_value_is_an_escape() {
    let d = code(
        "fn leak() = with_region[r] { with_cell[r](0) { c -> map_insert(map_new(), 1, c) } }",
        codes::REGION_ESCAPE,
    );
    names_value_and_region(&d, "Cell[r]<Int>", "r");
}

#[test]
fn a_cell_as_a_map_key_is_an_escape() {
    let d = code(
        "fn leak() = with_region[r] { with_cell[r](0) { c -> map_insert(map_new(), c, 1) } }",
        codes::REGION_ESCAPE,
    );
    names_value_and_region(&d, "Cell[r]<Int>", "r");
}

#[test]
fn a_cell_in_a_constructors_type_argument_is_an_escape() {
    let d = code(
        "type Box<a> = | Wrap(a)
fn leak() = with_region[r] { with_cell[r](0) { c -> Wrap(c) } }",
        codes::REGION_ESCAPE,
    );
    names_value_and_region(&d, "Cell[r]<Int>", "r");
}

#[test]
fn a_declared_type_may_not_have_a_cell_for_a_field() {
    let d = code("type Holder = | Held(Cell<Int>)", codes::REGION_ESCAPE);
    assert!(says(&d, "Held"), "{d:#?}");
    assert!(says(&d, "outside every region"), "{d:#?}");
}

#[test]
fn a_declared_cell_field_written_through_an_alias_is_refused_too() {
    let d = code(
        "type Counter = Cell<Int>
type Holder = | Held(Counter)",
        codes::REGION_ESCAPE,
    );
    assert!(says(&d, "Held"), "{d:#?}");
}

#[test]
fn a_closure_that_captured_the_cell_is_an_escape() {
    let d = code(
        "fn leak() = with_region[r] { with_cell[r](0) { c -> || cell_get(c) } }",
        codes::REGION_ESCAPE,
    );
    names_value_and_region(&d, "cell.read[r]", "r");
    assert!(says(&d, "the closure's row"), "{d:#?}");
}

#[test]
fn a_cell_cannot_be_read_through_a_function_that_does_not_know_its_region() {
    let d = code(
        "fn read(c: Cell<Int>) -> Int = cell_get(c)",
        codes::RESOURCE_REQUIRED,
    );
    assert!(says(&d, "region is unknown here"), "{d:#?}");
}

#[test]
fn an_alias_for_a_cell_does_not_hide_the_brand() {
    let d = code(
        "type Counter = Cell<Int>
fn leak() -> Counter = with_region[r] { with_cell[r](0) { c -> c } }",
        codes::REGION_ESCAPE,
    );
    names_value_and_region(&d, "Cell[r]<Int>", "r");
}

#[test]
fn storing_an_inner_regions_cell_into_an_outer_regions_cell_is_an_escape() {
    let d = code(
        "fn leak() -> Int = with_region[outer] {
  with_cell[outer]([]) { o ->
    {
      with_region[inner] {
        with_cell[inner](0) { i -> cell_set(o, [i]) }
      };
      0
    }
  }
}",
        codes::REGION_ESCAPE,
    );
    names_value_and_region(&d, "Cell[inner]<Int>", "inner");
    assert!(says(&d, "`o`"), "the store's target is not named: {d:#?}");
}

#[test]
fn a_cell_parameter_cannot_be_stored_into() {
    let d = code(
        "fn leak(sink: Cell<Int>) -> Int = { cell_set(sink, 1); 0 }",
        codes::RESOURCE_REQUIRED,
    );
    assert!(says(&d, "region is unknown here"), "{d:#?}");
}

#[test]
fn a_handler_clause_answering_with_a_closure_over_the_cell_is_an_escape() {
    let d = code(
        "effect ask { read get() -> Int }
fn leak() = with_region[r] {
  with_cell[r](0) { c ->
    handle ask.get() with {
      ask.get() resume k -> || cell_get(c),
      return x -> || x + cell_get(c)
    }
  }
}",
        codes::REGION_ESCAPE,
    );
    names_value_and_region(&d, "cell.read[r]", "r");
}

#[test]
fn sending_the_cell_to_a_task_the_region_cannot_outlive_is_an_escape() {
    let d = code(
        "fn leak() -> Int = simulate {
  with_region[r] {
    with_cell[r](0) { c ->
      { let t = task.spawn(|| cell_get(c)); task.join(t) }
    }
  }
}",
        codes::REGION_ESCAPE,
    );
    names_value_and_region(&d, "cell.read[r]", "r");
    assert!(says(&d, "another task"), "{d:#?}");
}

/// That `simulate` ends before the region does, and so does every task it runs.
#[test]
fn a_task_spawned_by_a_scheduler_inside_the_region_is_not_an_escape() {
    ok("fn shared() -> Int = with_region[r] {
  with_cell[r](0) { c ->
    simulate { let t = task.spawn(|| cell_get(c)); task.join(t) }
  }
}");
}

#[test]
fn two_regions_of_one_name_in_scope_at_once_are_refused() {
    let d = code(
        "fn shadow() -> Int = with_region[r] { with_region[r] { 1 } }",
        codes::REGION_ALREADY_OPEN,
    );
    assert!(says(&d, "already open"), "{d:#?}");
}

#[test]
fn two_regions_of_one_name_in_sequence_are_fine() {
    ok("fn twice() -> Int = {
  let a = with_region[r] { with_cell[r](1) { c -> cell_get(c) } };
  let b = with_region[r] { with_cell[r](2) { c -> cell_get(c) } };
  a + b
}");
}

#[test]
fn an_outer_regions_value_escaping_names_the_outer_region() {
    let d = code(
        "fn leak() = with_region[outer] {
  with_cell[outer](0) { c -> with_region[inner] { c } }
}",
        codes::REGION_ESCAPE,
    );
    names_value_and_region(&d, "Cell[outer]<Int>", "outer");
    assert!(
        !says(&d, "`inner`"),
        "the inner region did not allocate this: {d:#?}"
    );
}

#[test]
fn a_with_cell_written_before_this_change_still_checks_unmodified() {
    let out = ok("fn counter() -> Int = with_cell[r](0) { c -> { cell_set(c, 3); cell_get(c) } }");
    assert!(
        out.defs[&ply_span::Symbol::new("m.counter")]
            .footprint
            .is_empty()
    );
}

#[test]
fn a_bare_with_cell_still_reports_an_escaping_cell_as_e0201() {
    let d = code(
        "fn leak() = with_cell[r](0) { c -> c }",
        codes::TYPE_MISMATCH,
    );
    assert!(says(&d, "escapes its `with_cell[r]` region"), "{d:#?}");
}

#[test]
fn a_bare_with_cell_shared_across_tasks_still_checks() {
    ok("fn shared() -> Int = with_cell[s](0) { c ->
  simulate { let t = task.spawn(|| cell_get(c)); task.join(t) }
}");
}

#[test]
fn with_region_is_still_an_ordinary_name_where_no_bracket_follows() {
    ok("fn f(with_region: Int) -> Int = with_region + 1");
}

#[test]
fn with_cell_is_still_an_ordinary_name_where_no_bracket_follows() {
    ok("fn f(with_cell: Int) -> Int = with_cell + 1");
}

#[test]
fn an_effect_operation_may_not_declare_a_cell() {
    let d = code(
        "effect sink { write put(c: Cell<Int>) -> Unit }",
        codes::REGION_ESCAPE,
    );
    assert!(says(&d, "put"), "{d:#?}");

    let r = code(
        "effect source { read take() -> Cell<Int> }",
        codes::REGION_ESCAPE,
    );
    assert!(says(&r, "take"), "{r:#?}");
}

#[test]
fn a_declared_field_may_not_name_a_cell_atom_in_its_row() {
    let d = code(
        "type H = | Held(() -> Int / {cell.read[r]})",
        codes::REGION_ESCAPE,
    );
    assert!(says(&d, "Held"), "{d:#?}");
}

/// Nothing is solved on the receiving side yet, which a check at the closing brace would miss.
#[test]
fn storing_into_an_outer_cell_whose_element_type_is_still_open_is_an_escape() {
    let d = code(
        r#"fn leak() -> Int = with_cell[k](panic("seed")) { slot ->
  with_region[r] {
    with_cell[r](0) { c -> { cell_set(slot, c); 1 } }
  }
}"#,
        codes::REGION_ESCAPE,
    );
    names_value_and_region(&d, "Cell[r]<Int>", "r");
    assert!(says(&d, "`slot`"), "{d:#?}");
}

#[test]
fn storing_a_closure_over_the_cell_into_an_outer_cell_is_an_escape() {
    let d = code(
        r#"fn leak() -> Int = with_cell[k](panic("seed")) { slot ->
  with_region[r] {
    with_cell[r](0) { c -> { cell_set(slot, || cell_get(c)); 1 } }
  }
}"#,
        codes::REGION_ESCAPE,
    );
    names_value_and_region(&d, "cell.read[r]", "r");
}

#[test]
fn a_store_from_a_region_opened_inside_a_lambda_is_still_an_escape() {
    let d = code(
        r#"fn leak() -> Int = with_cell[k](panic("seed")) { slot ->
  { let f = || with_region[r] { with_cell[r](0) { c -> cell_set(slot, c) } }; f(); 1 }
}"#,
        codes::REGION_ESCAPE,
    );
    names_value_and_region(&d, "Cell[r]<Int>", "r");
}

#[test]
fn a_brand_survives_a_polymorphic_call() {
    let d = code(
        "fn ident<a>(x: a) -> a = x
fn leak() = with_region[r] { with_cell[r](0) { c -> ident(c) } }",
        codes::REGION_ESCAPE,
    );
    names_value_and_region(&d, "Cell[r]<Int>", "r");
}

#[test]
fn a_cell_inside_an_option_is_an_escape() {
    let d = code(
        "fn leak() = with_region[r] { with_cell[r](0) { c -> Some(c) } }",
        codes::REGION_ESCAPE,
    );
    names_value_and_region(&d, "Cell[r]<Int>", "r");
}

#[test]
fn handing_the_cell_to_a_generic_operation_is_an_escape() {
    let d = code(
        "effect sink { write put(x: a) -> Unit }
fn leak() -> Int / {sink.write} = with_region[r] {
  with_cell[r](0) { c -> { sink.put(c); 0 } }
}",
        codes::REGION_ESCAPE,
    );
    names_value_and_region(&d, "Cell[r]<Int>", "r");
    assert!(says(&d, "sink.put"), "{d:#?}");
}

/// The brand is one level down, which a check of the argument's head constructor would miss.
#[test]
fn handing_a_structure_holding_the_cell_to_an_operation_is_an_escape() {
    let d = code(
        "effect sink { write put(x: List<a>) -> Unit }
fn leak() -> Int / {sink.write} = with_region[r] {
  with_cell[r](0) { c -> { sink.put([c]); 0 } }
}",
        codes::REGION_ESCAPE,
    );
    names_value_and_region(&d, "List<Cell[r]<Int>>", "r");

    let r = code(
        "effect sink { write put(x: { held: a }) -> Unit }
fn leak() -> Int / {sink.write} = with_region[r] {
  with_cell[r](0) { c -> { sink.put({held: c}); 0 } }
}",
        codes::REGION_ESCAPE,
    );
    names_value_and_region(&r, "Cell[r]<Int>", "r");
}

#[test]
fn handing_a_closure_over_the_cell_to_an_operation_is_an_escape() {
    let d = code(
        "effect job { write run(f: () -> Int / e) -> Int }
fn leak() -> Int / {job.write} = with_region[r] {
  with_cell[r](0) { c -> job.run(|| cell_get(c)) }
}",
        codes::REGION_ESCAPE,
    );
    names_value_and_region(&d, "cell.read[r]", "r");
}

/// Pins the consequence rather than the mechanism, so re-opening the route fails here too.
#[test]
fn the_operation_route_would_otherwise_launder_the_brand_into_a_bare_variable() {
    let d = code(
        "effect sink { write put(x: a) -> Unit }
fn produce() -> Int / {sink.write} = with_region[p] {
  with_cell[p](7) { c -> { sink.put(c); 0 } }
}
fn consume() = handle { produce() } with {
  sink.put(x) resume j -> [x],
  return n -> []
}",
        codes::REGION_ESCAPE,
    );
    names_value_and_region(&d, "Cell[p]<Int>", "p");
}

#[test]
fn an_operation_performed_inside_a_region_is_fine_without_a_brand() {
    ok("effect ask { read get() -> Int }
fn f() -> Int / {ask.read} = with_region[r] {
  with_cell[r](0) { c -> { cell_set(c, ask.get()); cell_get(c) } }
}");
}

#[test]
fn storing_a_continuation_captured_inside_the_region_is_an_escape() {
    let d = code(
        r#"effect ask { read get() -> Int }
fn leak() -> Int = with_cell[k](panic("seed")) { slot ->
  with_region[r] {
    with_cell[r](0) { c ->
      handle { cell_set(c, ask.get()); cell_get(c) } with {
        ask.get() resume j -> { cell_set(slot, j); 0 }
      }
    }
  }
}"#,
        codes::REGION_ESCAPE,
    );
    names_value_and_region(&d, "cell.read[r]", "r");
    assert!(says(&d, "`slot`"), "{d:#?}");
}

/// A clause answers the `handle`'s result, so answering with the continuation is an infinite type.
#[test]
fn a_clause_cannot_answer_with_its_own_continuation() {
    let d = code(
        "effect ask { read get() -> Int }
fn leak() = with_region[r] {
  with_cell[r](0) { c ->
    handle { cell_set(c, ask.get()); 0 } with { ask.get() resume j -> j }
  }
}",
        codes::OCCURS_CHECK,
    );
    assert!(says(&d, "would have to equal"), "{d:#?}");
}

#[test]
fn a_brand_cannot_be_written_in_a_type_annotation() {
    let diags = errors("fn g(c: Cell[r]<Int>) -> Int = cell_get(c)");
    assert!(
        diags.iter().any(|d| d.code == codes::UNEXPECTED_TOKEN),
        "{diags:#?}"
    );
}

#[test]
fn a_written_cell_row_does_not_give_a_parameter_a_region() {
    let d = code(
        "fn g(c: Cell<Int>) -> Int / {cell.read[r]} = cell_get(c)",
        codes::RESOURCE_REQUIRED,
    );
    assert!(says(&d, "region is unknown here"), "{d:#?}");
}

#[test]
fn a_region_inside_a_lambda_still_reports_its_escape() {
    let d = code(
        "fn leak() = || with_region[r] { with_cell[r](0) { c -> c } }",
        codes::REGION_ESCAPE,
    );
    names_value_and_region(&d, "Cell[r]<Int>", "r");
}
