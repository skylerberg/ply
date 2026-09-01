//! **ADR 0033 §10 G1 — position invariance, registered before the measurement.**
//!
//! ADR 0033's central claim is that the positional rule
//! (`spikes/ply-lexer/GAPS.md` §1) is an artifact of tracking ownership at
//! *scope* granularity over a shared `Rc` chain, and that slot-granular frames
//! (§4, sequenced as S4) remove it. That claim is **argued from the mechanism
//! and not measured**, which is what this file is for: the criterion is written
//! down here, in code, before the fix exists, per `CONTRIBUTING.md`
//! §"Measure an ADR's motivating claim before accepting the ADR".
//!
//! The corpus is **paired**. Each pair computes the same value two ways:
//!
//! - *canonical* — the growing sub-expression is last at every enclosing node;
//! - *pessimal* — it is first, or otherwise non-final, at one or more of them.
//!
//! A pair is the whole instrument. A single program measures nothing here: an
//! evaluator that reused at every append and one that reused at none would both
//! be consistent with any one-sided reading, and it is the *difference between
//! two spellings of one computation* that G1 is about. This is
//! `ownership_checker_armed.rs`'s construction — the same append written two
//! ways so no constant answer passes — applied to a rate rather than to a
//! verdict.
//!
//! Counted with [`rc::sites`], not with a clock. Append counts are
//! deterministic: two runs of one program agree to the digit whatever the
//! machine is doing, so nothing here belongs in the deferred timing shard.
//!
//! ## The three tests, and why one of them is ignored
//!
//! - [`the_same_computation_costs_the_same_in_either_order`] is G1 itself. **It
//!   is red on this tree, and it was run and seen red on 2026-08-31 rather than
//!   assumed to be**, per
//!   `CONTRIBUTING.md` §"Do not state a guarantee you have not armed". It is
//!   `#[ignore]`d so that it does not redden CI before S4 lands; the ignore
//!   reason says how to run it.
//! - [`every_pair_is_pinned_to_what_it_costs_today`] is **not** ignored. It pins
//!   today's per-pair counts exactly, so movement in either direction — S4
//!   fixing it, or anything else quietly changing it — fails rather than going
//!   unnoticed. `region_kind_inference::the_split_over_the_repositorys_own_examples`
//!   is the model: a number a later change is expected to move, printed and
//!   held, with the document to correct named beside it.
//! - [`the_corpus_is_the_five_shapes_it_says_it_is`] holds the corpus itself,
//!   and it is here because **neither of the other two can see what they are
//!   measuring**. Every count in the table below is 200 / 200 or 0 / 200, so
//!   one pair's programs can be replaced by another pair's with G1 red in the
//!   same words and the pin green to the digit. That is not a hypothesis about
//!   this file; it is the state it was found in, and the next section has it.
//!
//! ## That none of them is vacuous, checked rather than argued
//!
//! An ignored test is unobserved by definition, and a pin is green on the day
//! it is written whatever it pins. So each was mutated and watched, 2026-08-31,
//! and each mutation was reverted:
//!
//! - **Measure the canonical member twice** — `pessimal: canonical` in
//!   [`measure_corpus`], so every pair reports identical rates. G1 went
//!   **green**, wrongly, at 200 / 200 against 200 / 200 and a gap of 0.000 on
//!   all five pairs; the pin went **red**, naming every pessimal member that
//!   had stopped being measured. That is the division of labour the two tests
//!   are for: G1 alone cannot tell "the gap closed" from "the instrument
//!   stopped looking", and the pin can.
//! - **Measure the pessimal member twice** — the same edit the other way, so
//!   every pair reports 0 / 200 against 0 / 200 and the gap is again 0.000. G1
//!   stayed **red** on all five pairs, on [`Criteria::min_canonical_rate`]
//!   alone, and the pin went **red**. Taken with the bullet above, **no
//!   constant measurement passes**: a gap of zero is reachable from either
//!   side and neither side gets through both bars.
//! - **Point one pair's canonical member at its pessimal source** — the gap
//!   goes to 0.000 and the canonical rate to 0.000. G1 stayed **red**, and for
//!   that pair it failed on [`Criteria::min_canonical_rate`] alone rather than
//!   on the gap. The second bullet of §10 G1 is therefore armed independently
//!   of the first, which matters because the first is satisfiable by making the
//!   canonical form as slow as the pessimal one.
//! - **Replace one pair's pessimal member with another pair's** —
//!   [`COMPOUNDING_PESSIMAL`] swapped for [`RECORD_FIELD_PESSIMAL`], which
//!   deletes ADR 0025 §Context row five from the corpus and leaves a duplicate
//!   of the pair above it. **This is the state this file was found in on
//!   2026-08-31**, with ADR 0033 §10's third row and §14's "five shapes"
//!   reporting a measurement nothing was taking, and with `COMPOUNDING_PESSIMAL`
//!   unreferenced — `cargo clippy -p ply-eval --tests` said so, and CI runs
//!   clippy with `-D warnings`. Re-run deliberately after the repair: the pin
//!   stays **green** to the digit and G1 stays **red in the same words**, and
//!   [`the_corpus_is_the_five_shapes_it_says_it_is`] is what fails.
//! - **Empty the corpus** — `corpus()` returns `Vec::new()`. G1 asserts that a
//!   list of failures is empty and an empty corpus produces one, so it reported
//!   **`ok`** over nothing at all; [`EXPECTED_PAIRS`], checked in
//!   [`measure_corpus`], now fails it and both other tests on the count.
//! - **Grow a member a second append site** — [`CALL_ARG_CANONICAL`]'s
//!   recursive call split over two branches that append 100 times each. Its
//!   append counts do not move: the pin read `(200, 200, 2)` against the pinned
//!   `(200, 200, 1)` and failed **on the site column alone**. That is the whole
//!   of what [`rc::sites`] buys over `rc::stats` here, and it buys it only
//!   because [`Cost::sites`] is pinned — summing the split back into a total,
//!   which is what this file did before, leaves exactly the number `rc::stats`
//!   would have given and nothing would have failed. (The identity test fails
//!   too, on the digest, because the source changed. It is the count pin that
//!   would catch the same shape arriving from a change in the evaluator.)
//!
//! The first two are guarded by nothing but this note: an instrument that
//! measures one member of a pair twice leaves the corpus untouched, and no test
//! here reads the wiring in [`measure_corpus`]. **The other four redden a
//! test**, which is the difference between a mutation that was run once and a
//! property that is held.

use ply_eval::rc;
use ply_eval::{Machine, TaskRegions};
use ply_span::SourceMap;
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::parse_program;
use ply_syntax::resolve::{Resolved, resolve};

// ------------------------------------------------------------ the criteria

/// The thresholds, pinned before the numbers exist.
///
/// Modelled on `ply_corpus::w6::Criteria::default()` and for its reason: there
/// is deliberately no path from a measurement to these values. Nothing below
/// constructs a `Criteria` from a file, an environment variable or a measured
/// rate, so a run cannot supply the bar it is about to clear.
#[derive(Clone, Copy, Debug)]
struct Criteria {
    /// ADR 0033 §10 G1, first bullet: `|in_place_rate(canonical) −
    /// in_place_rate(pessimal)| ≤ 0.02` for **every** pair. Two spellings of one
    /// computation may not differ in cost by more than measurement noise, and
    /// there is no noise in an append count, so this is nearly an equality.
    max_position_gap: f64,
    /// ADR 0033 §10 G1, second bullet: `in_place_rate(canonical) ≥ 0.95` for
    /// every pair whose canonical form is linear today. Without it the first
    /// bullet is satisfiable by making the canonical form as slow as the
    /// pessimal one, which is the wrong direction to converge in.
    min_canonical_rate: f64,
    /// A member that ran fewer appends than this measured nothing, and a rate
    /// over a handful of updates is one program's accident. Not a threshold on
    /// the result — a guard on the instrument.
    min_updates: u64,
}

impl Default for Criteria {
    fn default() -> Criteria {
        Criteria {
            max_position_gap: 0.02,
            min_canonical_rate: 0.95,
            min_updates: 100,
        }
    }
}

// -------------------------------------------------------------- the corpus

/// One shape, written the two ways.
struct Pair {
    /// What the pair is about, printed in the table.
    name: &'static str,
    /// Where the shape comes from, so a reader can check the pair against the
    /// row it reproduces rather than against this file's opinion of it.
    provenance: &'static str,
    canonical: &'static str,
    pessimal: &'static str,
    /// Whether the canonical spelling is expected to reuse on the tree as it
    /// stands, which is what gates [`Criteria::min_canonical_rate`]. Declared
    /// per pair with a citation rather than read off the measurement, because a
    /// bar the measurement selects itself into is not a bar.
    canonical_linear_today: bool,
}

/// ADR 0025 §Context rows 1 and 2 — `go(i + 1, push(acc, i))` at 200 / 200
/// against `go(push(acc, i), i + 1)` at 0 / 200. The two functions differ in
/// parameter order because that is the shape: the growing argument moves, and
/// nothing else about the computation does.
const CALL_ARG_CANONICAL: &str = r#"
fn go(i: Int, acc: List<Int>) -> List<Int> =
  if i >= 200 { acc } else { go(i + 1, push(acc, i)) }

test "the growing argument is last in the call" {
  assert_eq(len(go(0, [])), 200)
}
"#;

const CALL_ARG_PESSIMAL: &str = r#"
fn go(acc: List<Int>, i: Int) -> List<Int> =
  if i >= 200 { acc } else { go(push(acc, i), i + 1) }

test "the growing argument is first in the call" {
  assert_eq(len(go([], 0)), 200)
}
"#;

/// ADR 0025 §Context rows 3 and 4 — the same loop with the accumulator inside a
/// record, the growing field last against first in the literal. This is the
/// rule as `docs/GUIDE.md` §6.7 states it.
const RECORD_FIELD_CANONICAL: &str = r#"
fn go(i: Int, s: {k: Int, out: List<Int>}) -> {k: Int, out: List<Int>} =
  if i >= 200 { s } else { go(i + 1, {k: s.k + 1, out: push(s.out, i)}) }

test "the growing field is last in its literal" {
  let s = go(0, {k: 0, out: []});
  assert_eq(len(s.out) + s.k, 400)
}
"#;

const RECORD_FIELD_PESSIMAL: &str = r#"
fn go(i: Int, s: {k: Int, out: List<Int>}) -> {k: Int, out: List<Int>} =
  if i >= 200 { s } else { go(i + 1, {out: push(s.out, i), k: s.k + 1}) }

test "the growing field is first in its literal" {
  let s = go(0, {k: 0, out: []});
  assert_eq(len(s.out) + s.k, 400)
}
"#;

/// ADR 0025 §Context row **five**, which is the finding: the growing field is
/// last in its literal — the documented rule, applied correctly — and the
/// record is not last in the enclosing call, so the program is quadratic
/// anyway. The rule compounds at every enclosing node on the path from the
/// `push` up, and this is the pair that says so: its canonical member is
/// byte-identical to [`RECORD_FIELD_CANONICAL`] and only the *outer* node moved.
const COMPOUNDING_PESSIMAL: &str = r#"
fn go(s: {k: Int, out: List<Int>}, i: Int) -> {k: Int, out: List<Int>} =
  if i >= 200 { s } else { go({k: s.k + 1, out: push(s.out, i)}, i + 1) }

test "the growing field is last and its record is first in the call" {
  let s = go({k: 0, out: []}, 0);
  assert_eq(len(s.out) + s.k, 400)
}
"#;

/// ADR 0025 §Context cause 1 — *"an accumulator threaded as a `let` binding is
/// reused; the identical accumulator threaded as a parameter is not"*, measured
/// there as 1 of 1 against 0 of 1 on
/// `{ let t = push(xs, 1); let u = 7; len(t) + u }`. Those three statements are
/// reproduced verbatim in both members — with the binding that produces `xs`
/// added ahead of them, since ADR 0025's fragment does not say where `xs` comes
/// from and that is the whole of what this pair varies: a statement binder here,
/// a parameter there. `code.rs`'s `cumulative` is seeded from statement binders
/// alone, so a parameter can never enter a `Dead` set.
///
/// The driver exists to run the body 200 times rather than once: a rate over one
/// update is not a rate.
///
/// **This pair is not strictly positional and is in the corpus anyway.** The
/// two members differ in where the accumulator *arrives from*, not in where the
/// `push` sits, so a reading of G1 as "argument order" excludes it — and they
/// differ in one more thing, said here because a reader of ADR 0033 §10's table
/// alone would not know it: the pessimal member introduces `inner`, so it runs
/// 200 call frames the canonical member does not. Both answer 2200 and both run
/// exactly 200 appends, which is what the rate is taken over. ADR 0033 §4
/// does not read it that way — *"a parameter is a slot like any other; if the
/// caller passed its last use, the value arrives at count 1"* — and it is the
/// same claim under test: one computation, two spellings, and a cost that
/// should not be able to tell them apart. It is also the shape S3 is scheduled
/// to move on its own, so this pair is the one that can go green before S4.
const PARAM_VS_LET_CANONICAL: &str = r#"
fn probe(n: Int) -> Int = {
  let xs = range(0, n);
  let t = push(xs, 1);
  let u = 7;
  len(t) + u
}

fn drive(i: Int, tot: Int) -> Int =
  if i >= 200 { tot } else { drive(i + 1, tot + probe(3)) }

test "the accumulator arrives as a let binding" {
  assert_eq(drive(0, 0), 2200)
}
"#;

const PARAM_VS_LET_PESSIMAL: &str = r#"
fn inner(xs: List<Int>) -> Int = {
  let t = push(xs, 1);
  let u = 7;
  len(t) + u
}

fn probe(n: Int) -> Int = inner(range(0, n))

fn drive(i: Int, tot: Int) -> Int =
  if i >= 200 { tot } else { drive(i + 1, tot + probe(3)) }

test "the accumulator arrives as a parameter" {
  assert_eq(drive(0, 0), 2200)
}
"#;

/// ADR 0025 §Context row 6 — the `fold` accumulator, which is the shape the
/// standard library is written in, at 200 / 200.
///
/// **Both members call a two-argument helper that answers its list argument,
/// and that is deliberate.** In `|acc, x| push(acc, x)` the append *is* the
/// whole closure body, so there is no non-final position for it to occupy; a
/// pessimal member has to introduce an enclosing node. Introducing one on only
/// one side would leave the pair differing in two things, so both sides get the
/// same helper with its parameters in opposite orders and the position of the
/// `push` is again the only difference.
const FOLD_CLOSURE_CANONICAL: &str = r#"
fn keep_last(i: Int, a: List<Int>) -> List<Int> = a

fn build() -> List<Int> = fold(range(0, 200), [], |acc, x| keep_last(x, push(acc, x)))

test "a fold accumulator appended in last position" {
  assert_eq(len(build()), 200)
}
"#;

const FOLD_CLOSURE_PESSIMAL: &str = r#"
fn keep_first(a: List<Int>, i: Int) -> List<Int> = a

fn build() -> List<Int> = fold(range(0, 200), [], |acc, x| keep_first(push(acc, x), x))

test "a fold accumulator appended in first position" {
  assert_eq(len(build()), 200)
}
"#;

/// How many pairs the corpus has, and therefore how many shapes G1 is taken
/// over.
///
/// Written down because [`the_same_computation_costs_the_same_in_either_order`]
/// asserts that a list of failures is empty, and an empty corpus produces an
/// empty list: with `corpus()` returning `Vec::new()`, G1 reported `ok` — run
/// and watched, 2026-08-31, before [`measure_corpus`] checked this. ADR 0033
/// §10's table has five rows and §14 says "a paired corpus of five shapes", so
/// the number is a claim in two documents and this is the one place it lives.
const EXPECTED_PAIRS: usize = 5;

fn corpus() -> Vec<Pair> {
    vec![
        Pair {
            name: "call argument",
            provenance: "ADR 0025 §Context rows 1-2",
            canonical: CALL_ARG_CANONICAL,
            pessimal: CALL_ARG_PESSIMAL,
            canonical_linear_today: true,
        },
        Pair {
            name: "record field",
            provenance: "ADR 0025 §Context rows 3-4",
            canonical: RECORD_FIELD_CANONICAL,
            pessimal: RECORD_FIELD_PESSIMAL,
            canonical_linear_today: true,
        },
        Pair {
            name: "compounding: field last, record first",
            provenance: "ADR 0025 §Context rows 3, 5",
            canonical: RECORD_FIELD_CANONICAL,
            pessimal: COMPOUNDING_PESSIMAL,
            canonical_linear_today: true,
        },
        Pair {
            name: "let binding against parameter",
            provenance: "ADR 0025 §Context cause 1",
            canonical: PARAM_VS_LET_CANONICAL,
            pessimal: PARAM_VS_LET_PESSIMAL,
            canonical_linear_today: true,
        },
        Pair {
            name: "fold closure accumulator",
            provenance: "ADR 0025 §Context row 6",
            canonical: FOLD_CLOSURE_CANONICAL,
            pessimal: FOLD_CLOSURE_PESSIMAL,
            canonical_linear_today: true,
        },
    ]
}

// --------------------------------------------------------- the measurement

struct Program1 {
    program: Program,
    resolved: Resolved,
}

/// One inline module, parsed and resolved. Panics rather than answering
/// `Option`, exactly as `ownership_checker_armed.rs` does: a member that does
/// not parse has measured nothing, and skipping it silently is how an armed
/// test disarms itself.
fn inline(src: &str) -> Program1 {
    let name = ModuleName::from_dotted("g1");
    let mut map = SourceMap::new();
    let source = map.add(std::path::Path::new("g1.ply"), src.to_string());
    let mut program = match parse_program(vec![(source, name, src)]) {
        Ok(p) => p,
        Err(ds) => panic!("a G1 corpus member must parse: {ds:#?}"),
    };
    let expanded = ply_derive::expand_program(&mut program);
    assert!(expanded.is_empty(), "derive refused: {expanded:?}");
    let resolved = match resolve(&mut program) {
        Ok(r) => r,
        Err(ds) => panic!("a G1 corpus member must resolve: {ds:#?}"),
    };
    Program1 { program, resolved }
}

/// A member's identity, as twelve hex characters of BLAKE3 over its source.
///
/// `map_order::the_iteration_order_is_pinned` is the idiom. It is here because
/// **no count in this corpus identifies the program that produced it**: every
/// canonical member reads 200 of 200 and every pessimal one 0 of 200, so any
/// member may be replaced by any other of its kind with both bars staying
/// green. `the_corpus_is_the_five_shapes_it_says_it_is` holds these.
fn digest(src: &str) -> String {
    format!("b3:{}", &blake3::hash(src.as_bytes()).to_hex()[..12])
}

/// One member's appends: the totals, and the number of `push` sites they came
/// from.
struct Cost {
    count: rc::SiteCount,
    /// Distinct spans [`rc::sites`] attributed an update to. This is the whole
    /// of what the per-site split buys over [`rc::stats`], and it buys it only
    /// because the pin holds this column: a member whose one append site became
    /// two, running half as often each, reads 200 of 200 either way and moves
    /// nothing a total can see.
    sites: usize,
}

/// Every append this member ran, summed over its sites, and how many sites
/// that was.
///
/// What isolates one member from the next is [`rc::record_sites`] clearing the
/// site map, on the arming call as well as the disarming one. Do not add a
/// filter by [`SourceId`] instead: [`inline`] builds a fresh [`SourceMap`] per
/// member and `SourceMap::add` numbers from zero, so every member of this corpus
/// is `SourceId(0)` and such a filter excludes nothing it is meant to.
fn measure(src: &str) -> Cost {
    let p = inline(src);
    let mut machine = Machine::for_program(&p.program, &p.resolved);
    machine.set_regions(TaskRegions::new());
    rc::record_sites(true);
    let mut ran = 0;
    for index in 0..machine.test_count() {
        machine
            .eval_test(index)
            .unwrap_or_else(|d| panic!("a G1 corpus member's test must run: {d:#?}"));
        ran += 1;
    }
    assert!(
        ran > 0,
        "a G1 corpus member declares no test, so nothing ran"
    );
    let sites = rc::sites();
    let mut count = rc::SiteCount::default();
    for (_, site) in &sites {
        count.in_place += site.in_place;
        count.copies += site.copies;
    }
    rc::record_sites(false);
    Cost {
        count,
        sites: sites.len(),
    }
}

struct Measured {
    canonical: Cost,
    pessimal: Cost,
}

impl Measured {
    fn gap(&self) -> f64 {
        let c = self
            .canonical
            .count
            .rate()
            .expect("the canonical member ran appends");
        let p = self
            .pessimal
            .count
            .rate()
            .expect("the pessimal member ran appends");
        (c - p).abs()
    }
}

fn measure_corpus(c: &Criteria) -> Vec<(Pair, Measured)> {
    let pairs = corpus();
    assert_eq!(
        pairs.len(),
        EXPECTED_PAIRS,
        "the corpus is {} pairs where {EXPECTED_PAIRS} is what ADR 0033 §10's table and §14 \
         report; a bar over fewer shapes than it claims is not the bar that was registered",
        pairs.len(),
    );
    let mut out = Vec::new();
    for pair in pairs {
        let m = Measured {
            canonical: measure(pair.canonical),
            pessimal: measure(pair.pessimal),
        };
        for (which, count) in [
            ("canonical", m.canonical.count),
            ("pessimal", m.pessimal.count),
        ] {
            assert!(
                count.total() >= c.min_updates,
                "the {which} member of `{}` ran {} appends, which is fewer than the {} this \
                 instrument needs to read a rate off; it measured nothing",
                pair.name,
                count.total(),
                c.min_updates,
            );
        }
        out.push((pair, m));
    }
    out
}

fn print_table(rows: &[(Pair, Measured)]) {
    println!(
        "\n  {:<38} {:>16} {:>16} {:>7} {:>7}",
        "pair", "canonical", "pessimal", "sites", "gap"
    );
    for (pair, m) in rows {
        println!(
            "  {:<38} {:>7}/{:<8} {:>7}/{:<8} {:>7} {:>7.3}",
            pair.name,
            m.canonical.count.in_place,
            m.canonical.count.total(),
            m.pessimal.count.in_place,
            m.pessimal.count.total(),
            format!("{}/{}", m.canonical.sites, m.pessimal.sites),
            m.gap(),
        );
        println!("  {:<38}   {}", "", pair.provenance);
    }
}

// ------------------------------------------------------------------- G1

/// **ADR 0033 §10 G1.** Two spellings of one computation cost the same.
///
/// Red on this tree, and it is supposed to be: §10 says so in advance —
/// *"Today's figures are 200 / 200 against 0 / 200, so it will be"* — and the
/// numbers [`every_pair_is_pinned_to_what_it_costs_today`] holds are what red
/// looks like. If §4 lands (S4) and this stays red, ADR 0033 §12 item 1 is what
/// follows: the diagnosis in §1 is wrong and §5 is the whole of what survives.
#[test]
#[ignore = "ADR 0033 §10 G1: red until §11 S4 (slot frames) lands, and armed by having been \
            shown red — see every_pair_is_pinned_to_what_it_costs_today for today's numbers. \
            Run it with `cargo test -p ply-eval --test suite position_invariance -- --ignored \
            --nocapture`."]
fn the_same_computation_costs_the_same_in_either_order() {
    let c = Criteria::default();
    let rows = measure_corpus(&c);
    print_table(&rows);

    let mut failures = Vec::new();
    for (pair, m) in &rows {
        let canonical = m
            .canonical
            .count
            .rate()
            .expect("the canonical member ran appends");
        let pessimal = m
            .pessimal
            .count
            .rate()
            .expect("the pessimal member ran appends");
        if m.gap() > c.max_position_gap {
            failures.push(format!(
                "`{}` ({}): canonical {canonical:.3} against pessimal {pessimal:.3}, a gap of \
                 {:.3} where {} is the bar — the same value, written two ways, at two costs",
                pair.name,
                pair.provenance,
                m.gap(),
                c.max_position_gap,
            ));
        }
        if pair.canonical_linear_today && canonical < c.min_canonical_rate {
            failures.push(format!(
                "`{}` ({}): the canonical form reused {canonical:.3} of its appends where {} is \
                 the bar, and it is declared linear today — closing the gap by making the \
                 canonical form slow is not what G1 asks for",
                pair.name, pair.provenance, c.min_canonical_rate,
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "ADR 0033 §10 G1 is not met on {} of {} pairs:\n  - {}",
        failures.len(),
        rows.len(),
        failures.join("\n  - "),
    );
}

// ---------------------------------------------------------------- the corpus

/// The corpus is five shapes, each pair is two different programs, and each is
/// the program it was pinned as.
///
/// **Neither bar can see any of that, and that is why this exists.** Every
/// pinned count is `(200, 200)` against `(0, 200)`, so one pair's members can
/// be replaced by another pair's and both bars stay green on a corpus that has
/// silently lost a shape. That is not hypothetical: on 2026-08-31 this file's
/// compounding pair was found pointing at [`RECORD_FIELD_PESSIMAL`] — a
/// byte-identical duplicate of the pair above it — while ADR 0033 §10's third
/// row and §14's "five shapes" reported a measurement nothing was taking. The
/// pin was green to the digit and G1 was red in exactly the words it is red in
/// now, so neither said anything. Reproduced deliberately after the repair, and
/// this test is what fails.
///
/// The digests are BLAKE3 over the source text, so **editing a member reddens
/// this**. That is intended: re-pin the digest in the same change, and if you
/// did not edit anything, a corpus member moved under you.
#[test]
fn the_corpus_is_the_five_shapes_it_says_it_is() {
    let pairs = corpus();
    assert_eq!(
        pairs.len(),
        EXPECTED_PAIRS,
        "the corpus lost or gained a pair"
    );

    for pair in &pairs {
        assert_ne!(
            digest(pair.canonical),
            digest(pair.pessimal),
            "`{}` is one program written twice, so it measures no difference in position",
            pair.name,
        );
    }
    for (i, a) in pairs.iter().enumerate() {
        for b in &pairs[i + 1..] {
            assert_ne!(
                digest(a.pessimal),
                digest(b.pessimal),
                "`{}` and `{}` are the same pessimal program, so one of the two shapes ADR 0033 \
                 §10 reports a row for is not being measured",
                a.name,
                b.name,
            );
        }
    }

    let digests: Vec<(String, String)> = pairs
        .iter()
        .map(|p| (digest(p.canonical), digest(p.pessimal)))
        .collect();
    let got: Vec<(&str, &str, &str)> = pairs
        .iter()
        .zip(&digests)
        .map(|(p, d)| (p.name, d.0.as_str(), d.1.as_str()))
        .collect();

    // Taken on this tree 2026-08-31. The second and third rows share a
    // canonical digest on purpose: the compounding pair is the record-field
    // canonical with only the *outer* node moved, which is the whole of what
    // ADR 0025 §Context row five says.
    let expected: [(&str, &str, &str); EXPECTED_PAIRS] = [
        ("call argument", "b3:2a168234d2a1", "b3:dc587bf7bfb2"),
        ("record field", "b3:031f75989bca", "b3:a52314fd4b69"),
        (
            "compounding: field last, record first",
            "b3:031f75989bca",
            "b3:adf0a898c53a",
        ),
        (
            "let binding against parameter",
            "b3:e242c2b60c30",
            "b3:5c6b5d559209",
        ),
        (
            "fold closure accumulator",
            "b3:9416752c4c7a",
            "b3:71e06e69eacd",
        ),
    ];

    assert_eq!(
        got.as_slice(),
        expected.as_slice(),
        "a corpus member is not the program it was pinned as; re-pin it here if you edited it \
         deliberately, and if you did not, a member moved under you — see this test's \
         documentation"
    );
}

// ------------------------------------------------------------------ the pin

/// One pinned member: `(in place, total, sites)`.
type Member = (u64, u64, usize);
/// One pinned row: the pair's name, then its canonical member and its pessimal
/// one.
type Row = (&'static str, Member, Member);

/// What each pair costs on this tree, held to the digit.
///
/// `region_kind_inference::the_split_over_the_repositorys_own_examples` is the
/// model — a number a later change is expected to move, printed so a reader can
/// see it and held so a reader cannot miss it moving. The difference is that
/// that census asserts only that it measured something, because a threshold on
/// it would be a threshold on `examples/`; here the corpus is this file's own
/// and every count in it is a property of the evaluator, so the pin is exact.
///
/// **When this fails, that is the event it exists for.** Read the direction:
/// pessimal counts rising towards their canonical partners is ADR 0033 §11 S4
/// working, and the response is to re-pin here, run
/// [`the_same_computation_costs_the_same_in_either_order`] with `--ignored`,
/// and — if it is green — delete `docs/GUIDE.md` §6.7 and its §19 gotcha per
/// §11. Anything else moving is a regression until shown otherwise.
///
/// **What it does not hold is which program produced a number**, because the
/// numbers do not distinguish the members: that is
/// [`the_corpus_is_the_five_shapes_it_says_it_is`]'s job, and neither test
/// covers the other.
#[test]
fn every_pair_is_pinned_to_what_it_costs_today() {
    let rows = measure_corpus(&Criteria::default());
    print_table(&rows);

    // `(in place, total, sites)` for the canonical member and then the pessimal
    // one, measured on this tree 2026-08-31. Every pair reads 200 of 200
    // against 0 of 200 over one append site: the gap is 1.000 and G1's bar is
    // 0.02.
    let expected: [Row; EXPECTED_PAIRS] = [
        ("call argument", (200, 200, 1), (0, 200, 1)),
        ("record field", (200, 200, 1), (0, 200, 1)),
        (
            "compounding: field last, record first",
            (200, 200, 1),
            (0, 200, 1),
        ),
        // Moved by ADR 0033 §11 S3 (ADR 0025 P2) from `(0, 200, 1)`: a parameter
        // may now appear in a `Dead` set, so the accumulator threaded as a
        // parameter is reused exactly as the one threaded as a `let` is. This is
        // the **first** pair to meet G1, and it met it without a slot frame,
        // which is why S3 was kept in the sequence after S4 was known to subsume
        // it. The other four are positional and are S4's to move.
        (
            "let binding against parameter",
            (200, 200, 1),
            (200, 200, 1),
        ),
        ("fold closure accumulator", (200, 200, 1), (0, 200, 1)),
    ];

    let got: Vec<Row> = rows
        .iter()
        .map(|(pair, m)| {
            (
                pair.name,
                (
                    m.canonical.count.in_place,
                    m.canonical.count.total(),
                    m.canonical.sites,
                ),
                (
                    m.pessimal.count.in_place,
                    m.pessimal.count.total(),
                    m.pessimal.sites,
                ),
            )
        })
        .collect();

    assert_eq!(
        got.as_slice(),
        expected.as_slice(),
        "the per-pair append counts moved; see this test's documentation for how to read the \
         direction"
    );
}
