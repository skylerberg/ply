//! Arming the ownership checker: two programs whose cost is known before the
//! checker is asked, and the counters beside every answer.
//!
//! `ownership_checker_oracle` measures the checker over the shipped corpus. It
//! cannot tell whether the instrument works, because a checker that answered
//! `Copies` everywhere would agree with a corpus that copies everywhere and a
//! checker that answered `Reuses` everywhere would agree with one that does
//! not. What this file adds is the pair: **the same append, written the two
//! ways, one linear and one quadratic**, so no constant answer passes.
//!
//! Every assertion here is made twice — once against `ply_eval::costs` and once
//! against `ply_eval::rc::sites` — so neither the checker nor the expectation
//! can be wrong alone and still be green.

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

/// One inline module, parsed and resolved. Panics rather than answering
/// `Option`: a test whose program does not parse has measured nothing, and
/// skipping it silently is how an armed test disarms itself.
fn inline(src: &str) -> Program1 {
    let name = ModuleName::from_dotted("armed");
    let mut map = SourceMap::new();
    let source = map.add(std::path::Path::new("armed.ply"), src.to_string());
    let mut program =
        parse_program(vec![(source, name, src)]).expect("the armed program must parse");
    let expanded = ply_derive::expand_program(&mut program);
    assert!(expanded.is_empty(), "derive refused: {expanded:?}");
    let resolved = resolve(&program).expect("the armed program must resolve");
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

/// The one `push` the program contains, with the checker's verdict and the run's
/// count side by side.
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

/// `push` at argument 0 of 3: the enclosing call's frame carries the scope for
/// the whole of the append, so the accumulator is at two owners and the whole
/// array is copied on every iteration. ADR 0025 §Context measures this shape at
/// 0 of 200 in place.
const QUADRATIC: &str = r#"
fn grow(acc: List<Int>, i: Int, n: Int) -> List<Int> =
  if i >= n { acc } else { grow(push(acc, i), i + 1, n) }

test "a growing accumulator in a non-last argument position" {
  assert_eq(len(grow([], 0, 60)), 60)
}
"#;

/// The identical loop with the append moved into last position, which ADR 0025
/// §Context measures at 200 of 200 in place.
const LINEAR_BY_POSITION: &str = r#"
fn grow(i: Int, n: Int, acc: List<Int>) -> List<Int> =
  if i >= n { acc } else { grow(i + 1, n, push(acc, i)) }

test "a growing accumulator in last argument position" {
  assert_eq(len(grow(0, 60, [])), 60)
}
"#;

/// A `fold` accumulator, which is the shape the standard library is written in
/// and the one a checker must not call shared.
const LINEAR_BY_FOLD: &str = r#"
fn build(n: Int) -> List<Int> = fold(range(0, n), [], |acc, x| push(acc, x))

test "an accumulator threaded through a fold" {
  assert_eq(len(build(60)), 60)
}
"#;

#[test]
fn a_quadratic_append_is_flagged_and_the_counters_confirm_it() {
    let p = inline(QUADRATIC);
    let (verdict, reason, count) = only_site(&p);
    println!("quadratic: {verdict:?} — {reason}\n  {count:?}");
    assert_eq!(
        count.in_place, 0,
        "the control is not quadratic after all: {count:?}, so this test arms nothing"
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
}

#[test]
fn the_same_loop_in_last_position_is_not_flagged_and_the_counters_confirm_it() {
    let p = inline(LINEAR_BY_POSITION);
    let (verdict, reason, count) = only_site(&p);
    println!("linear by position: {verdict:?} — {reason}\n  {count:?}");
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

/// The pair, stated as one assertion: two programs that compute the same answer
/// and differ only in argument order must get **different** verdicts.
///
/// This is what a constant answer cannot pass, and it is the whole reason the
/// two shapes are in one file.
#[test]
fn the_two_shapes_get_different_verdicts_which_is_what_a_constant_answer_cannot_do() {
    let (slow, _, slow_count) = only_site(&inline(QUADRATIC));
    let (fast, _, fast_count) = only_site(&inline(LINEAR_BY_POSITION));
    assert_ne!(
        slow_count.rate(),
        fast_count.rate(),
        "the two controls cost the same at runtime, so they are not a pair"
    );
    assert_ne!(
        slow, fast,
        "the checker gave one answer to a quadratic loop and to the same loop \
         written linearly; it is a constant and agreement with any corpus is vacuous"
    );
}
