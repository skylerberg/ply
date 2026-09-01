//! The region model and its brand at the type level: `with_region[r] { .. }`, the brand `r` carries in the
//! types of the values allocated in it, and every route a branded value could take out of the
//! region.

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

/// The first diagnostic carrying `code`, and a readable failure when there is none — a route that
/// opened usually opens by producing *no* diagnostic at all.
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

/// Every escape names the value's type and the region it belongs to.
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

// --- what is meant to work --------------------------------------------------

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

/// The region model: "an inner region may reference an outer region's values".
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

/// The `with_cell` inside a region no longer closes a region of its own, so the cell may outlive
/// the `{ .. }` that made it and not the region.
#[test]
fn a_cell_may_outlive_its_with_cell_when_a_region_of_that_name_is_open() {
    ok("fn hold() -> Int = with_region[r] {
  cell_get(with_cell[r](3) { c -> c })
}");
}

// --- the escape routes ------------------------------------------------------

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

/// A `Map` key is the position W2's hole was reachable through, so it gets its own test even though
/// the walk that finds it is the same one.
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

/// A concrete `Cell` field is refused where it is *declared*.
#[test]
fn a_declared_type_may_not_have_a_cell_for_a_field() {
    let d = code("type Holder = | Held(Cell<Int>)", codes::REGION_ESCAPE);
    assert!(says(&d, "Held"), "{d:#?}");
    assert!(says(&d, "outside every region"), "{d:#?}");
}

/// Same refusal through an alias, because the check runs on the converted type rather than on what
/// was spelled — the W2 route, closed at the declaration.
#[test]
fn a_declared_cell_field_written_through_an_alias_is_refused_too() {
    let d = code(
        "type Counter = Cell<Int>
type Holder = | Held(Counter)",
        codes::REGION_ESCAPE,
    );
    assert!(says(&d, "Held"), "{d:#?}");
}

/// The closure route.
#[test]
fn a_closure_that_captured_the_cell_is_an_escape() {
    let d = code(
        "fn leak() = with_region[r] { with_cell[r](0) { c -> || cell_get(c) } }",
        codes::REGION_ESCAPE,
    );
    names_value_and_region(&d, "cell.read[r]", "r");
    assert!(says(&d, "the closure's row"), "{d:#?}");
}

/// Reading a cell somewhere else is not a route at all: `cell_get` needs the region statically, so
/// a function taking a `Cell<Int>` cannot read it and the only place a branded cell is ever read is
/// where its brand is known.
#[test]
fn a_cell_cannot_be_read_through_a_function_that_does_not_know_its_region() {
    let d = code(
        "fn read(c: Cell<Int>) -> Int = cell_get(c)",
        codes::RESOURCE_REQUIRED,
    );
    assert!(says(&d, "region is unknown here"), "{d:#?}");
}

/// The type-alias route at the *use* site: `Counter` and `Cell<Int>` are one type by the time the
/// check looks, so they get one answer.
#[test]
fn an_alias_for_a_cell_does_not_hide_the_brand() {
    let d = code(
        "type Counter = Cell<Int>
fn leak() -> Counter = with_region[r] { with_cell[r](0) { c -> c } }",
        codes::REGION_ESCAPE,
    );
    names_value_and_region(&d, "Cell[r]<Int>", "r");
}

/// The route no result type can see: the region answers with `Unit` and the brand is sitting in a
/// binding that predates it.
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

/// A cell reached through a parameter cannot be written at all, so a store into something outside
/// the definition is not a shape a region has to refuse.
#[test]
fn a_cell_parameter_cannot_be_stored_into() {
    let d = code(
        "fn leak(sink: Cell<Int>) -> Int = { cell_set(sink, 1); 0 }",
        codes::RESOURCE_REQUIRED,
    );
    assert!(says(&d, "region is unknown here"), "{d:#?}");
}

/// A general clause's body has the whole `handle`'s type, so it is an exit of the region around it
/// and a continuation-shaped escape is reported there.
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

/// The same spawn is fine when the scheduler is opened *inside* the region: that `simulate` ends
/// before the region does, and so does every task it runs.
#[test]
fn a_task_spawned_by_a_scheduler_inside_the_region_is_not_an_escape() {
    ok("fn shared() -> Int = with_region[r] {
  with_cell[r](0) { c ->
    simulate { let t = task.spawn(|| cell_get(c)); task.join(t) }
  }
}");
}

// --- nesting ----------------------------------------------------------------

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

/// The escape is reported against the region the value belongs to, not against whichever region
/// happened to be innermost.
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

// --- what must not move -----------------------------------------------------

/// The region model: "the surface syntax is unchanged, so existing programs do not move."
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

/// A cell reaching a task is how tasks share memory (CONTRACTS §`simulate`), and nothing about that
/// changes for a program that never wrote `with_region`.
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

// --- routes that hide the brand from a type ---------------------------------

/// An operation's signature is converted once for the whole program, exactly as a variant field is,
/// so a `Cell` in one is the same hiding place — and the handler that would receive the cell can be
/// installed anywhere, including outside the region.
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

/// A `cell` atom is writable in a row — a cell that outlives its region through a continuation puts
/// one in a published footprint — so a declared field of function type could otherwise name the row
/// a branded closure has and swallow it whole.
#[test]
fn a_declared_field_may_not_name_a_cell_atom_in_its_row() {
    let d = code(
        "type H = | Held(() -> Int / {cell.read[r]})",
        codes::REGION_ESCAPE,
    );
    assert!(says(&d, "Held"), "{d:#?}");
}

/// The store route with nothing yet solved on the receiving side, which is the version a check that
/// ran at the region's closing brace would miss.
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

/// The region is opened inside a lambda and the store happens there, so nothing about the region's
/// own result type could have shown it.
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

/// The brand survives a call, which is what makes the check compositional: a polymorphic function's
/// region argument is quantified, so each call gets its own and the caller's brand comes back out.
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

// --- the operation route ----------------------------------------------------

/// The route that has no type at its far end.
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

/// The same route with the brand one level down, which is the shape a check that only looked at the
/// argument's head constructor would miss.
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

/// A closure handed to an operation is the same escape reached through the row, and it is refused
/// whether or not the declaration left the row open.
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

/// What the escape above actually buys, spelled out so that a future change that re-opens the route
/// fails on the consequence rather than on the mechanism: the handler is in another definition, the
/// value it receives is a freed cell, and the type it receives it at is an ordinary variable it may
/// hand to anyone.
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

/// Performing an operation inside a region is otherwise untouched: only a brand reaching an
/// argument is refused.
#[test]
fn an_operation_performed_inside_a_region_is_fine_without_a_brand() {
    ok("effect ask { read get() -> Int }
fn f() -> Int / {ask.read} = with_region[r] {
  with_cell[r](0) { c -> { cell_set(c, ask.get()); cell_get(c) } }
}");
}

// --- the continuation route -------------------------------------------------

/// A continuation captured inside the region carries the region's atoms in its row — they are the
/// effects of the code it will resume into — so storing it where it outlives the region is the
/// store route, reported against the store.
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

/// The other half of the continuation route, and the reason it needs no check of its own: a clause
/// that binds a continuation answers with the `handle`'s result, so a clause answering with the
/// continuation — bare, in a list, or wrapped in a closure — asks for a type that contains itself.
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

// --- routes the surface does not offer --------------------------------------

/// A brand is spelled by a region and never by a user: `Cell[r]<Int>` is not surface syntax, so a
/// signature cannot claim a region it does not open and smuggle a cell out through an annotation.
#[test]
fn a_brand_cannot_be_written_in_a_type_annotation() {
    let diags = errors("fn g(c: Cell[r]<Int>) -> Int = cell_get(c)");
    assert!(
        diags.iter().any(|d| d.code == codes::UNEXPECTED_TOKEN),
        "{diags:#?}"
    );
}

/// Nor through the row: a written `cell.read[r]` names no cell the callee can reach, so the cell's
/// region is still unknown where it is read.
#[test]
fn a_written_cell_row_does_not_give_a_parameter_a_region() {
    let d = code(
        "fn g(c: Cell<Int>) -> Int / {cell.read[r]} = cell_get(c)",
        codes::RESOURCE_REQUIRED,
    );
    assert!(says(&d, "region is unknown here"), "{d:#?}");
}

/// A region opened inside a lambda escapes through the lambda's result, and the label sits on the
/// lambda's body rather than on the whole definition.
#[test]
fn a_region_inside_a_lambda_still_reports_its_escape() {
    let d = code(
        "fn leak() = || with_region[r] { with_cell[r](0) { c -> c } }",
        codes::REGION_ESCAPE,
    );
    names_value_and_region(&d, "Cell[r]<Int>", "r");
}
