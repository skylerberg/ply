//! An adversarial reading of ADR 0017 §2, taken as a claim to be falsified
//! rather than as a design to be illustrated.
//!
//! §2 says that returning a branded value, storing it in an outer structure,
//! capturing it in a closure that outlives the region, or sending it to another
//! task "are all the same error". `tests/regions.rs` walks the routes the
//! implementation was written against; this file walks the ones it was not, and
//! the two sections below are the two answers it got.
//!
//! The first section is routes out of a `with_region[r]` that were not pinned
//! anywhere: an outer binding reached through a closure or a record field, a
//! `Cell` reached through two chained aliases or a generic one, a brand carried
//! through a mutually recursive pair, and — the one that matters most — the
//! fact that two definitions spelling a region `k` do not share a brand at the
//! point it would be dereferenced.
//!
//! The second section is the route ADR 0017 §"Where this could go wrong" lists
//! first: "an escape the brand does not catch — through a closure", out of a
//! bare `with_cell[r]`, which ADR 0017 §1 declares to be a region. It was open,
//! because the bare form filed no region site and checked only the shape of its
//! own result type. It is closed: `with_cell` opens a region like any other and
//! all four of `Checker::check_regions`'s rules run on it, over the **resolved**
//! type and including a function type's effect row.
//!
//! Refusing programs that used to run is a change of meaning, so it is a
//! decision rather than a patch and it is recorded in ADR 0017 §2, together with
//! the one exclusion (`task.spawn`) and the one route that stays open (a
//! continuation parked in an enclosing region's cell, which is ADR 0005
//! required test 6).

use ply_core::{CheckOutput, check_program};
use ply_span::{Diagnostic, SourceId, codes};
use ply_syntax::ast::ModuleName;
use ply_syntax::resolve::resolve;

fn compile(source: &str) -> Result<CheckOutput, Vec<Diagnostic>> {
    let inputs = vec![(SourceId(0), ModuleName::from_dotted("m"), source)];
    let mut program = ply_syntax::parse_program(inputs)?;
    let diags = ply_derive::expand_program(&mut program);
    if !diags.is_empty() {
        return Err(diags);
    }
    let resolved = resolve(&program)?;
    check_program(&program, &resolved)
}

#[track_caller]
fn errors(source: &str) -> Vec<Diagnostic> {
    match compile(source) {
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

// --- routes that are closed, and were not pinned ----------------------------

/// The store route where the region body never names the cell it stores into.
/// `store` was bound before the region opened, so the brand lands in *its*
/// parameter type and the escape is reported against the call.
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

/// The same route with the outer cell one field access away, which is the shape
/// a check that only looked at bare occurrences of a *cell-typed* binding would
/// miss.
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

/// The store performed by a lambda written inside the region, so the region's
/// own result type is `Unit` and the enclosing definition's is `Int`.
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

/// W2's hole was a check that ran before alias resolution. One alias is pinned
/// in `tests/regions.rs`; a chain of them is the version where an expansion that
/// stopped after a single step would answer differently.
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

/// A generic alias, where the `Cell` is the *argument* rather than the alias
/// body, so nothing in the written parameter type is spelled `Cell` at its head.
#[test]
fn a_generic_alias_does_not_hide_a_cell_in_an_operation() {
    let d = code(
        "type Boxed<t> = List<t>
effect sink { write put(x: Boxed<Cell<Int>>) -> Unit }",
        codes::REGION_ESCAPE,
    );
    names(&d, "put");
}

/// The same at a variant field, with the `Cell` behind a record alias.
#[test]
fn a_record_alias_does_not_hide_a_cell_in_a_variant_field() {
    let d = code(
        "type Rec = { held: Cell<Int> }
type H = | Held(Rec)",
        codes::REGION_ESCAPE,
    );
    names(&d, "Held");
}

/// `tests/regions.rs` pins one polymorphic call. A mutually recursive pair is
/// where the two definitions are generalized against each other, which is the
/// point an inference bug would drop the brand on the floor.
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

/// A region whose value leaves through a `handle` installed *around* it, so the
/// exit the region records is not the expression the definition answers with.
#[test]
fn a_region_inside_a_handle_still_reports_its_escape() {
    let d = code(
        "effect ask { read get() -> Int }
fn leak() = handle { with_region[r] { with_cell[r](0) { c -> c } } } with { return x -> x }",
        codes::REGION_ESCAPE,
    );
    names(&d, "Cell[r]<Int>");
}

/// The continuation carries the region's atoms in its row, so handing it to an
/// operation is the operation route reached through the row rather than through
/// the shape.
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

/// A `test` is a definition like any other and its body is checked the same way.
#[test]
fn a_region_in_a_test_block_reports_its_escape() {
    let d = code(
        "test \"leak\" { with_region[r] { with_cell[r](0) { c -> c } } }",
        codes::REGION_ESCAPE,
    );
    names(&d, "Cell[r]<Int>");
}

/// The brand is the region's *name* (`E0446`'s "region is already open" note
/// says so), and a name is not unique across definitions. This is what stops
/// that from mattering: a cell laundered out of one definition's `k` cannot be
/// dereferenced inside another definition's `k`, because `cell_get` resolves its
/// region from the enclosing `with_cell` binder and not from the spelling.
///
/// Without this, the operation route out of a bare `with_cell` — which is open,
/// see below — would be a readable dangling cell rather than only an
/// unreadable one.
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

/// A footprint's resource is the region's **name** (ADR 0008 §6), so a caller
/// whose own region is spelled `k` discharges `cell.read[k]` out of everything
/// its region encloses. That was a laundering route while a cell could escape:
/// an escaped cell's atoms reached the caller's row, and the caller's unrelated
/// `k` swallowed them, erasing the escape from the published footprint.
///
/// The route is closed at its source — the escape itself is now refused, above
/// — and what is left is the design: a `cell.read[k]` atom can now only come
/// from a *written* row, which is a claim about a region named `k`, and a region
/// named `k` is the thing entitled to discharge it. This pins both halves so
/// that reopening either is a failing test rather than a silent regression.
#[test]
fn only_a_written_row_can_put_a_foreign_regions_atom_in_a_callers_row() {
    // The producer that used to hand its cell out is refused, so no atom
    // belonging to a region the caller does not own can reach the caller at all.
    let d = code(
        "fn writer() = with_cell[k](0) { c -> {get: || cell_get(c), set: |v| cell_set(c, v)} }",
        codes::TYPE_MISMATCH,
    );
    names(&d, "escapes its `with_cell[k]` region");

    // What remains is an annotation, and a region named `k` discharges it —
    // inside the region, and only there.
    let out = compile(
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

// --- the routes out of a bare `with_cell` -----------------------------------
//
// ADR 0017 §1 makes `with_cell[r]` a region and §2 makes every escape from a
// region a type error. Every test below is a value allocated in a region, alive
// after that region's `}`, and each one checked clean until `with_cell` began
// filing a region site of its own. The identical program under `with_region[r]`
// has always been refused, which is what made these a hole rather than a
// design.

/// ADR 0017 §2, the closure clause, applied to the region ADR 0017 §1 says a
/// `with_cell` is. The identical program under `with_region[r]` is `E0446`
/// (`tests/regions.rs::a_closure_that_captured_the_cell_is_an_escape`); here it
/// checks, and `leak()()` reads a cell whose region closed.
///
/// `mentions_region` is `brand_in` with the `Type::Fn` effect-row case missing,
/// which is the only reason the two answers differ.
#[test]
fn a_closure_capturing_a_bare_with_cells_cell_is_an_escape() {
    let d = code(
        "fn leak() = with_cell[k](0) { c -> || cell_get(c) }",
        codes::TYPE_MISMATCH,
    );
    names(&d, "escapes its `with_cell[k]` region");
}

/// The writable half, which is what makes the escape more than a stale read: the
/// pair is a full mutable handle on a region that has closed, and it survives
/// being taken apart into two separate bindings.
#[test]
fn a_reader_and_writer_pair_over_a_bare_with_cells_cell_is_an_escape() {
    let d = code(
        "fn leak() = with_cell[k](0) { c -> {get: || cell_get(c), set: |v| cell_set(c, v)} }",
        codes::TYPE_MISMATCH,
    );
    names(&d, "escapes its `with_cell[k]` region");
}

/// The operation route, which ADR 0017 calls "the route no other check can see"
/// and closes for `with_region` in `Checker::check_region_handoffs`. A bare
/// `with_cell` files no handoff, so the cell reaches a handler in another
/// definition at a bare type variable.
#[test]
fn a_bare_with_cells_cell_handed_to_a_generic_operation_is_an_escape() {
    let d = code(
        "effect sink { write put(x: a) -> Unit }
fn leak() -> Int / {sink.write} = with_cell[k](0) { c -> { sink.put(c); 0 } }",
        codes::REGION_ESCAPE,
    );
    names(&d, "sink.put");
}

/// The store route. `with_region` closes it in `Checker::check_region_stores`;
/// a bare `with_cell` records no outer bindings, so the inner region's cell is
/// parked in the outer one and outlives the `}` that frees it.
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
