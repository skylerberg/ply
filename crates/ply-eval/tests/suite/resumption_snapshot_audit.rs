//! Whether a region that a continuation is captured across is recognised as one, and whether the
//! arena's save-and-restore primitive covers what it says it covers.

use ply_core::{CheckOutput, check_program};
use ply_eval::arena::{Arena, RegionKind, Slot};
use ply_eval::region_kind::{Cause, Regions, check, infer};
use ply_eval::{Machine, Value};
use ply_span::{SourceId, SourceMap, Span};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::parse_program;
use ply_syntax::resolve::{Resolved, resolve};

fn load(src: &str) -> (Program, Resolved) {
    let mut map = SourceMap::new();
    let id: SourceId = map.add("snapshot.ply", src.to_string());
    let mut program = match parse_program([(id, ModuleName::from_dotted("snapshot"), src)]) {
        Ok(p) => p,
        Err(ds) => panic!("the probe must parse: {ds:#?}\n{src}"),
    };
    let resolved = resolve(&mut program).expect("the probe must resolve");
    (program, resolved)
}

#[track_caller]
fn kinds(src: &str) -> Vec<(String, RegionKind)> {
    let (program, resolved) = load(src);
    let regions: Regions = infer(&program, &resolved);
    assert!(
        !regions.is_empty(),
        "this probe opens no region, so it decides nothing\n{src}"
    );
    regions
        .iter()
        .map(|r| (r.brand.to_string(), r.kind))
        .collect()
}

#[track_caller]
fn kind_of(src: &str, brand: &str) -> RegionKind {
    kinds(src)
        .into_iter()
        .find(|(b, _)| b == brand)
        .unwrap_or_else(|| panic!("no region branded `{brand}`\n{src}"))
        .1
}

/// The cells the machine is left holding after one test, rendered, in id order.
#[track_caller]
fn cells_after(src: &str, test: &str) -> Vec<String> {
    let (program, resolved) = load(src);
    let checked: CheckOutput = check_program(&program, &resolved)
        .unwrap_or_else(|d| panic!("the probe must typecheck: {d:#?}\n{src}"));
    let index = checked
        .tests
        .iter()
        .position(|t| t.name == test)
        .unwrap_or_else(|| panic!("no test named {test:?}"));
    let mut machine = Machine::new(&program, &resolved, &checked);
    // The reclamation journal, not the residue: a region hands its slots back at its close, so what
    // a run leaves behind is empty whatever it wrote.
    machine.cells_mut().journal();
    machine
        .eval_test(index)
        .unwrap_or_else(|d| panic!("{test:?} must run: {d:#?}"));
    let mut cells: Vec<(u32, String)> = machine
        .cells()
        .journalled()
        .iter()
        .map(|(slot, v)| (slot.index(), v.render()))
        .collect();
    cells.sort_by_key(|(slot, _)| *slot);
    cells.into_iter().map(|(_, v)| v).collect()
}

fn int_at(arena: &Arena, slot: Slot) -> Option<i64> {
    match arena.get(slot) {
        Some(Value::Int(i)) => Some(*i),
        _ => None,
    }
}

/// The region-kind rule's two-resumption example with the `handle` written *outside* the `with_cell` instead
/// of inside it.
const HANDLE_ENCLOSES: &str = r#"
effect amb { read flip[coin]() -> Bool }

test "handler outside the region" {
  let total = handle {
    with_cell[trace](0) { c -> {
      let b = amb.flip[coin]();
      cell_set(c, cell_get(c) + 1);
      if b { cell_get(c) } else { cell_get(c) * 10 }
    } }
  } with {
    amb.flip[coin]() resume k -> k(true) + k(false),
    return x -> x
  };
  assert_eq(total, 21)
}
"#;

/// A capture crosses `trace` on every resumption, so `trace` is `shared` and the site is named.
#[test]
fn a_handle_enclosing_the_region_does_not_hide_the_capture() {
    let (program, resolved) = load(HANDLE_ENCLOSES);
    let regions = infer(&program, &resolved);
    let region = regions.iter().next().expect("one region");
    assert_eq!(region.brand.as_str(), "trace");
    assert_eq!(
        region.kind,
        RegionKind::Shared,
        "a continuation is captured across `trace` and resumed twice"
    );
    let site = region
        .capture
        .as_ref()
        .expect("and the site has to be nameable, or nothing downstream can report it");
    assert!(
        matches!(&site.cause, Cause::Escapes { effect, op }
            if effect.as_str() == "snapshot.amb" && op.as_str() == "flip"),
        "{:?}",
        site.cause
    );
}

/// The same program, run.
#[test]
fn the_region_that_infers_shared_is_written_by_both_resumptions() {
    assert_eq!(
        cells_after(HANDLE_ENCLOSES, "handler outside the region"),
        vec!["2".to_string()],
        "one cell, allocated before the capture and incremented by each resumption"
    );
}

/// Two spellings of one program must agree, which is how the hole was diagnosed rather than merely
/// observed.
#[test]
fn hoisting_the_perform_into_a_helper_does_not_flip_the_inferred_kind() {
    let inline = r#"
effect amb { read flip[coin]() -> Bool }

fn f() -> Int =
  handle { with_cell[trace](0) { c -> { let b = amb.flip[coin](); cell_get(c) } } } with {
    amb.flip[coin]() resume k -> k(true) + k(false),
    return x -> x
  }
"#;
    let hoisted = r#"
effect amb { read flip[coin]() -> Bool }

fn ask() -> Bool = amb.flip[coin]()

fn f() -> Int =
  handle { with_cell[trace](0) { c -> { let b = ask(); cell_get(c) } } } with {
    amb.flip[coin]() resume k -> k(true) + k(false),
    return x -> x
  }
"#;
    assert_eq!(kind_of(inline, "trace"), RegionKind::Shared);
    assert_eq!(
        kind_of(hoisted, "trace"),
        RegionKind::Shared,
        "the two programs mean the same thing and must infer the same kind"
    );
}

/// The hole was not confined to one spelling of the region or one clause form.
#[test]
fn the_enclosing_handle_hides_the_capture_for_no_shape_of_region() {
    const AMB: &str = "effect amb { read flip[coin]() -> Bool }\n";

    // A tail-resumptive clause.
    let tail = format!(
        r#"{AMB}
fn f() -> Int =
  handle {{ with_cell[trace](0) {{ c -> {{ let b = amb.flip[coin](); cell_get(c) }} }} }} with {{
    amb.flip[coin]() -> true,
    return x -> x
  }}
"#
    );
    assert_eq!(kind_of(&tail, "trace"), RegionKind::Shared);

    // `with_region[r]` with a cell allocated into it — the region syntax.
    let with_region = format!(
        r#"{AMB}
fn f() -> Int =
  handle {{
    with_region[r] {{ with_cell[r](0) {{ c -> {{ let b = amb.flip[coin](); cell_get(c) }} }} }}
  }} with {{
    amb.flip[coin]() resume k -> k(true) + k(false),
    return x -> x
  }}
"#
    );
    assert_eq!(kind_of(&with_region, "r"), RegionKind::Shared);

    // Nested regions: the capture crosses both, so a snapshot of either would have been required
    // and neither may be freed at its close.
    let nested = format!(
        r#"{AMB}
fn f() -> Int =
  handle {{
    with_cell[outer](0) {{ a ->
      with_cell[inner](0) {{ b -> {{ let f = amb.flip[coin](); cell_get(a) + cell_get(b) }} }} }}
  }} with {{
    amb.flip[coin]() resume k -> k(true) + k(false),
    return x -> x
  }}
"#
    );
    let mut both = kinds(&nested);
    both.sort();
    assert_eq!(
        both,
        vec![
            ("inner".to_string(), RegionKind::Shared),
            ("outer".to_string(), RegionKind::Shared),
        ]
    );

    // Through a callback builtin, where the region is opened per element and the capture is taken
    // inside it.
    let callback = format!(
        r#"{AMB}
fn f() -> List<Int> =
  handle {{
    map([1, 2], |n| with_cell[r](0) {{ c -> {{ let b = amb.flip[coin](); cell_get(c) + n }} }})
  }} with {{
    amb.flip[coin]() resume k -> k(true),
    return x -> x
  }}
"#
    );
    assert_eq!(kind_of(&callback, "r"), RegionKind::Shared);
}

/// The region-kind rule: "forcing `unique` where a capture is reachable is a compile error naming the
/// capture site".
#[test]
fn forcing_unique_over_a_capture_an_enclosing_handle_answers_is_refused() {
    let (program, resolved) = load(HANDLE_ENCLOSES);
    let span = infer(&program, &resolved)
        .iter()
        .next()
        .expect("one region")
        .span;
    let Err(ds) = check(&program, &resolved, &[(span, RegionKind::Unique)]) else {
        panic!("a capture is reachable across `trace`, so `unique` must be refused");
    };
    assert_eq!(ds.len(), 1, "{ds:#?}");
    assert_eq!(ds[0].code, ply_span::codes::REGION_KIND_REFUSED);
    assert!(ds[0].message.contains("`trace`"), "{}", ds[0].message);
    assert!(
        ds[0].labels.iter().any(|l| !l.primary),
        "the refusal names the capture site: {:#?}",
        ds[0].labels
    );
}

/// [`Arena::snapshot`] covers one region and the regions nested inside it.
#[test]
fn only_the_open_form_of_snapshot_covers_an_enclosing_regions_writes() {
    let mut arena = Arena::new();
    let outer = arena.open(RegionKind::Shared, Span::DUMMY);
    let x = arena.alloc(Value::Int(0)).expect("inside a region");
    let inner = arena.open(RegionKind::Shared, Span::DUMMY);
    let y = arena.alloc(Value::Int(0)).expect("inside a region");

    let narrow = arena.snapshot(inner).expect("a shared region snapshots");
    assert_eq!(
        narrow.len(),
        1,
        "the narrow form holds the inner region's slot and nothing below it"
    );
    arena.set(x, Value::Int(1));
    arena.set(y, Value::Int(1));
    arena.restore(&narrow);
    assert_eq!(int_at(&arena, y), Some(0), "the inner region was restored");
    assert_eq!(
        int_at(&arena, x),
        Some(1),
        "and the enclosing region was not, which is the documented limit"
    );

    arena.set(x, Value::Int(0));
    let wide = arena
        .snapshot_open()
        .expect("no open region is unique")
        .expect("two regions are open");
    assert_eq!(wide.region(), outer, "rooted at the outermost open region");
    assert_eq!(wide.regions(), 2);
    arena.set(x, Value::Int(1));
    arena.set(y, Value::Int(1));
    arena.restore(&wide);
    assert_eq!(int_at(&arena, x), Some(0), "both regions came back");
    assert_eq!(int_at(&arena, y), Some(0));
    arena.close(outer);
}

/// The same statement as a cost: covering every open region costs the whole live arena, not the
/// innermost scope.
#[test]
fn covering_every_open_region_costs_the_whole_live_arena_at_every_capture() {
    let mut arena = Arena::new();
    let outer = arena.open(RegionKind::Shared, Span::DUMMY);
    for i in 0..1_000 {
        arena.alloc(Value::Int(i));
    }
    let inner = arena.open(RegionKind::Shared, Span::DUMMY);
    arena.alloc(Value::Int(-1));

    let narrow = arena.snapshot(inner).expect("a shared region snapshots");
    let correct = arena
        .snapshot_open()
        .expect("no open region is unique")
        .expect("two regions are open");

    assert_eq!(narrow.len(), 1);
    assert_eq!(
        correct.len(),
        1_001,
        "the snapshot that actually isolates the writes is the outermost one"
    );
    arena.close(outer);
}

/// A `unique` region open at the point of a checkpoint is the inference and the machine
/// disagreeing, and the caller has to be told *which* region so it can name it.
#[test]
fn a_unique_region_open_at_a_checkpoint_is_reported_rather_than_skipped() {
    let mut arena = Arena::new();
    let outer = arena.open(RegionKind::Shared, Span::DUMMY);
    arena.alloc(Value::Int(1));
    let unique = arena.open(RegionKind::Unique, Span::DUMMY);
    arena.alloc(Value::Int(2));

    assert_eq!(arena.snapshot_open().err(), Some(unique));
    assert_eq!(arena.stats().snapshots, 0, "and nothing was copied");
    arena.close(outer);
}

/// A [`Snapshot`] records the scope stack as well as the bump range, so a restore restores the
/// arena's state rather than a range of it.
#[test]
fn a_restore_brings_a_closed_regions_scope_back_with_its_slots() {
    let mut arena = Arena::new();
    let r = arena.open(RegionKind::Shared, Span::DUMMY);
    arena.alloc(Value::Int(0)).expect("inside a region");
    let inner = arena.open(RegionKind::Unique, Span::DUMMY);
    let x = arena.alloc(Value::Int(7)).expect("inside a region");

    let at_capture = arena.snapshot(r).expect("a shared region snapshots");

    arena.set(x, Value::Int(8));
    arena.close(inner);
    assert_eq!(int_at(&arena, x), None, "the inner region's close freed it");
    assert_eq!(arena.depth(), 1);

    assert!(arena.restore(&at_capture));

    assert_eq!(int_at(&arena, x), Some(7), "the slot came back");
    assert_eq!(
        arena.depth(),
        2,
        "and so did the region that owns it, or nothing will free it again"
    );
    assert_eq!(arena.kind(inner), Some(RegionKind::Unique));
    assert_eq!(arena.current(), Some(inner));

    arena.close(inner);
    assert_eq!(int_at(&arena, x), None, "and the close frees it again");
    arena.close(r);
    assert_eq!(arena.live(), 0);
}

/// **Opened since the snapshot.**
#[test]
fn a_region_opened_after_the_snapshot_does_not_survive_the_restore() {
    let mut arena = Arena::new();
    let r = arena.open(RegionKind::Shared, Span::DUMMY);
    arena.alloc(Value::Int(0)).expect("inside a region");

    let at_capture = arena.snapshot(r).expect("a shared region snapshots");

    arena.alloc(Value::Int(1)).expect("inside a region");
    let opened = arena.open(RegionKind::Shared, Span::DUMMY);
    arena.alloc(Value::Int(2)).expect("inside a region");

    assert!(arena.restore(&at_capture));

    assert_eq!(arena.live(), 1, "the bump pointer went back to the capture");
    assert_eq!(arena.depth(), 1, "and so did the scope stack");
    assert_eq!(arena.current(), Some(r));
    assert_eq!(arena.kind(opened), None);

    let after = arena.alloc(Value::Int(9)).expect("inside a region");
    arena.close(r);
    assert_eq!(
        int_at(&arena, after),
        None,
        "an allocation made after the restore belongs to the region that is open"
    );
    assert_eq!(arena.live(), 0);
}

/// The precondition [`Arena::extent`] and [`Arena::snapshot`] subtract on, held across the
/// open/alloc/close/snapshot/restore cycle rather than asserted once.
#[test]
fn no_regions_mark_ever_sits_above_the_bump_pointer() {
    let mut arena = Arena::new();
    let r = arena.open(RegionKind::Shared, Span::DUMMY);
    arena.alloc(Value::Int(0)).expect("inside a region");
    let at_capture = arena.snapshot(r).expect("a shared region snapshots");
    arena.alloc(Value::Int(1)).expect("inside a region");
    let opened = arena.open(RegionKind::Shared, Span::DUMMY);

    assert_eq!(arena.extent(opened), Some(0));
    assert_eq!(arena.live(), 2);

    arena.restore(&at_capture);

    assert_eq!(arena.live(), 1);
    assert!(
        arena.kind(opened).is_none(),
        "`opened` is no longer a scope"
    );
    assert_eq!(arena.extent(r), Some(1));
    assert!(arena.extent(r).unwrap() <= arena.live());
    assert!(arena.snapshot_open().is_ok());
    arena.close(r);
}

/// A slot's generation is the number of times its physical position has been freed, and it is a
/// `u32` incremented with `wrapping_add`.
#[test]
fn a_slots_generation_counts_frees_and_is_a_wrapping_u32() {
    let mut arena = Arena::new();
    let mut seen = Vec::new();
    for _ in 0..8 {
        let r = arena.open(RegionKind::Unique, Span::DUMMY);
        seen.push(
            arena
                .alloc(Value::Int(0))
                .expect("inside a region")
                .generation(),
        );
        arena.close(r);
    }
    assert_eq!(
        seen,
        (0..8).collect::<Vec<u32>>(),
        "one increment per close at the same index; `u32::MAX` closes later the sequence starts \
         over and slot @0.0 is live again"
    );
}
