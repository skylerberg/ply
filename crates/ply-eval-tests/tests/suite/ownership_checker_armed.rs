//! Arming the ownership checker: two programs whose cost is known before the checker is asked, and
//! the counters beside every answer.

use ply_eval::costs::{Costs, Verdict};
use ply_eval::rc;
use ply_eval::{Machine, TaskRegions};
use ply_span::{SourceId, SourceMap, Span};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::parse_program;
use ply_syntax::resolve::{Resolved, resolve};

struct Program1 {
    program: Program,
    resolved: Resolved,
    map: SourceMap,
    source: SourceId,
}

/// One inline module, parsed and resolved.
fn inline(src: &str) -> Program1 {
    let name = ModuleName::from_dotted("armed");
    let mut map = SourceMap::new();
    let source = map.add(std::path::Path::new("armed.ply"), src.to_string());
    let mut program =
        parse_program(vec![(source, name, src)]).expect("the armed program must parse");
    let expanded = ply_derive::expand_program(&mut program);
    assert!(expanded.is_empty(), "derive refused: {expanded:?}");
    let resolved = resolve(&mut program).expect("the armed program must resolve");
    Program1 {
        program,
        resolved,
        map,
        source,
    }
}

/// What the checker said, per line of the source.
fn verdicts(p: &Program1) -> Vec<(u32, Verdict, String)> {
    let costs = Costs::new(&p.program, &p.resolved);
    let report = costs.check();
    let mut out = Vec::new();
    for def in report.all() {
        for site in &def.sites {
            out.push((
                line_of(&p.map, site.span),
                site.verdict,
                site.reason.clone(),
            ));
        }
    }
    out.sort_by_key(|(line, _, _)| *line);
    out
}

/// What the run said, per line of the source.
fn counted(p: &Program1) -> Vec<(u32, rc::SiteCount)> {
    let mut machine = Machine::for_program(&p.program, &p.resolved);
    machine.set_regions(TaskRegions::new());
    rc::record_sites(true);
    let mut ran = 0;
    for index in 0..machine.test_count() {
        machine
            .eval_test(index)
            .unwrap_or_else(|d| panic!("the armed program's test must run: {d:?}"));
        ran += 1;
    }
    assert!(
        ran > 0,
        "the armed program declares no test, so nothing ran"
    );
    let mut out: Vec<(u32, rc::SiteCount)> = rc::sites()
        .into_iter()
        .filter(|(span, _)| span.source == p.source)
        .map(|(span, count)| (line_of(&p.map, span), count))
        .collect();
    rc::record_sites(false);
    out.sort_by_key(|(line, _)| *line);
    out
}

fn line_of(map: &SourceMap, span: Span) -> u32 {
    map.get(span.source)
        .map(|f| f.line_col(span.start).0)
        .unwrap_or(0)
}

/// The one `push` the program contains, with the checker's verdict and the run's count side by
/// side.
fn only_site(p: &Program1) -> (Verdict, String, rc::SiteCount) {
    let said = verdicts(p);
    assert_eq!(
        said.len(),
        1,
        "the armed program must contain exactly one `push`, so there is nothing to \
         confuse one verdict with; got {said:?}"
    );
    let ran = counted(p);
    assert_eq!(
        ran.len(),
        1,
        "exactly one `push` site must have executed; got {ran:?}"
    );
    assert_eq!(
        said[0].0, ran[0].0,
        "the checker and the counters disagree about which line the append is on, so \
         they are not keyed by the same span and no comparison below means anything"
    );
    (said[0].1, said[0].2.clone(), ran[0].1)
}

/// The binding is read again after the append — a genuine second owner, which is what is left of
/// "copies" now that position decides nothing: the clone that later read forces is the owner.
const COPIES_BY_SECOND_READ: &str = r#"
fn grow(acc: List<Int>, i: Int, n: Int) -> List<Int> = {
  let next = push(acc, i);
  if len(acc) > n { acc } else if i >= n { next } else { grow(next, i + 1, n) }
}

test "a growing accumulator read again after the append" {
  assert_eq(len(grow([], 0, 60)), 61)
}
"#;

/// The spelling that was this file's *pessimal* control on the chain machine: the append in a
/// non-last argument position. Under ADR 0034's slot frames a last use moves the value out of its
/// slot wherever it sits, so this is now the linear control — which is the whole of what changed.
const LINEAR_BY_LAST_USE: &str = r#"
fn grow(acc: List<Int>, i: Int, n: Int) -> List<Int> =
  if i >= n { acc } else { grow(push(acc, i), i + 1, n) }

test "a growing accumulator at its last use" {
  assert_eq(len(grow([], 0, 60)), 60)
}
"#;

/// A `fold` accumulator, which is the shape the standard library is written in and the one a
/// checker must not call shared.
const LINEAR_BY_FOLD: &str = r#"
fn build(n: Int) -> List<Int> = fold(range(0, n), [], |acc, x| push(acc, x))

test "an accumulator threaded through a fold" {
  assert_eq(len(build(60)), 60)
}
"#;

/// A `push` onto an element read out of a list by index.
const COPIES_VIA_LIST_AT: &str = r#"
fn head_plus(rows: List<List<Int>>, i: Int) -> List<Int> =
  match list_at(rows, 0) { Some(row) -> push(row, i), None -> [] }

fn touch(rows: List<List<Int>>, i: Int, n: Int) -> Int =
  if i >= n { 0 } else { len(head_plus(rows, i)) + touch(rows, i + 1, n) }

test "an append onto an element matched out of an Option" {
  assert_eq(touch([[1, 2, 3]], 0, 60), 240)
}
"#;

/// The control for both: the identical loop, the identical helper, the identical `push` in the
/// helper's tail — over a list the round built itself rather than one read out of a container.
const FRESH_LIST_IN_THE_SAME_SHAPE: &str = r#"
fn head_plus(i: Int) -> List<Int> = push(range(0, 3), i)

fn touch(rows: List<List<Int>>, i: Int, n: Int) -> Int =
  if i >= n { 0 } else { len(head_plus(i)) + touch(rows, i + 1, n) }

test "an append onto a list this round built" {
  assert_eq(touch([[1, 2, 3]], 0, 60), 240)
}
"#;

#[test]
fn an_append_onto_an_indexed_element_is_flagged_and_the_counters_confirm_it() {
    let p = inline(COPIES_VIA_LIST_AT);
    let (verdict, reason, count) = only_site(&p);
    println!("list_at: {verdict:?} — {reason}\n  {count:?}");
    assert_eq!(
        count.in_place, 0,
        "the control is not copying after all: {count:?}, so this test arms nothing"
    );
    assert!(
        count.copies >= 50,
        "the loop must actually have run: {count:?}"
    );
    assert_eq!(
        verdict,
        Verdict::Copies,
        "the checker claimed an in-place append onto an element `list_at` read out of a \
         list the program still holds, and the run copied {} times — reason given: {reason}",
        count.copies
    );
    assert!(
        reason.contains("list_at"),
        "the verdict is `Copies`, but for a reason that is not the list index's: \
         {reason}. A `Copies` reached by the position rule is the same answer by \
         another route, and it is green with `result_owner`'s `ListAt` arm deleted — \
         which is exactly what this test did until the program above was rewritten"
    );
}

#[test]
fn the_same_loop_over_a_list_it_built_itself_is_not_flagged() {
    let p = inline(FRESH_LIST_IN_THE_SAME_SHAPE);
    let (verdict, reason, count) = only_site(&p);
    println!("fresh list in the same shape: {verdict:?} — {reason}\n  {count:?}");
    assert_eq!(
        count.copies, 0,
        "the control is not linear after all: {count:?}, so this test arms nothing"
    );
    assert!(
        count.in_place >= 50,
        "the loop must actually have run: {count:?}"
    );
    assert_eq!(
        verdict,
        Verdict::Reuses,
        "the checker called an append onto a list this round built shared, which is the \
         false positive that would send an author to insert `copy` — reason given: {reason}"
    );
}

#[test]
fn an_append_whose_binding_is_read_again_is_flagged_and_the_counters_confirm_it() {
    let p = inline(COPIES_BY_SECOND_READ);
    let (verdict, reason, count) = only_site(&p);
    println!("read again: {verdict:?} — {reason}\n  {count:?}");
    assert_eq!(
        count.in_place, 0,
        "the control is not copying after all: {count:?}, so this test arms nothing"
    );
    assert!(
        count.copies >= 50,
        "the loop must actually have run: {count:?}"
    );
    assert_eq!(
        verdict,
        Verdict::Copies,
        "the checker missed a program that copies the whole list on every one of \
         {} iterations — reason given: {reason}",
        count.copies
    );
    assert!(
        reason.contains("read again"),
        "the verdict is `Copies`, but not for the later read's reason: {reason}"
    );
}

#[test]
fn the_same_loop_at_its_last_use_is_not_flagged_and_the_counters_confirm_it() {
    let p = inline(LINEAR_BY_LAST_USE);
    let (verdict, reason, count) = only_site(&p);
    println!("linear by last use: {verdict:?} — {reason}\n  {count:?}");
    assert_eq!(
        count.copies, 0,
        "the control is not linear after all: {count:?}, so this test arms nothing"
    );
    assert!(
        count.in_place >= 50,
        "the loop must actually have run: {count:?}"
    );
    assert_eq!(
        verdict,
        Verdict::Reuses,
        "the checker called a linear loop shared, which is the false positive that \
         would send an author to insert `copy` and make it quadratic — reason given: {reason}"
    );
}

#[test]
fn a_fold_accumulator_is_not_flagged_and_the_counters_confirm_it() {
    let p = inline(LINEAR_BY_FOLD);
    let (verdict, reason, count) = only_site(&p);
    println!("linear by fold: {verdict:?} — {reason}\n  {count:?}");
    assert_eq!(
        count.copies, 0,
        "the control is not linear after all: {count:?}, so this test arms nothing"
    );
    assert!(
        count.in_place >= 50,
        "the fold must actually have run: {count:?}"
    );
    assert_eq!(
        verdict,
        Verdict::Reuses,
        "the checker called a `fold` accumulator shared — reason given: {reason}"
    );
}

/// The pair, stated as one assertion: two loops that differ only in whether the binding is read
/// again after the append must get **different** verdicts.
#[test]
fn the_two_shapes_get_different_verdicts_which_is_what_a_constant_answer_cannot_do() {
    let (slow, _, slow_count) = only_site(&inline(COPIES_BY_SECOND_READ));
    let (fast, _, fast_count) = only_site(&inline(LINEAR_BY_LAST_USE));
    assert_ne!(
        slow_count.rate(),
        fast_count.rate(),
        "the two controls cost the same at runtime, so they are not a pair"
    );
    assert_ne!(
        slow, fast,
        "the checker gave one answer to a copying loop and to the same loop whose \
         append is the last use; it is a constant and agreement with any corpus is vacuous"
    );
}

/// The fix the checker recommends for a `cell` cause, run: the contents leave the arena for the
/// length of the function, so the append is at one owner.
const REUSES_VIA_CELL_UPDATE: &str = r#"
fn fill(c: Cell<r, List<Int>>, i: Int) -> Unit / {cell.read[r], cell.write[r]} =
  if i >= 60 { () } else { cell_update(c, |xs| push(xs, i)); fill(c, i + 1) }

test "an accumulator kept in a cell, appended through the fused update" {
  with_cell[r]([]) { c -> { fill(c, 0); assert_eq(len(cell_get(c)), 60) } }
}
"#;

#[test]
fn an_append_through_cell_update_is_not_flagged_and_the_counters_confirm_it() {
    let p = inline(REUSES_VIA_CELL_UPDATE);
    let (verdict, reason, count) = only_site(&p);
    println!("cell_update: {verdict:?} — {reason}\n  {count:?}");
    assert_eq!(
        count.copies, 0,
        "the control is not linear after all: {count:?}, so this test arms nothing"
    );
    assert!(
        count.in_place >= 50,
        "the loop must actually have run: {count:?}"
    );
    assert_eq!(
        verdict,
        Verdict::Reuses,
        "the checker flagged the very edit it recommends for a `cell` cause — reason given: \
         {reason}"
    );
}
