//! **ADR 0034 §10 G1 — position invariance, registered before the measurement.**

use ply_eval::rc;
use ply_eval::{Machine, TaskRegions};
use ply_span::SourceMap;
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::parse_program;
use ply_syntax::resolve::{Resolved, resolve};

/// The thresholds, pinned before the numbers exist.
#[derive(Clone, Copy, Debug)]
struct Criteria {
    /// ADR 0034 §10 G1, first bullet: `|in_place_rate(canonical) −
    /// in_place_rate(pessimal)| ≤ 0.02` for **every** pair. Two spellings of one
    /// computation may not differ in cost by more than measurement noise, and
    /// there is no noise in an append count, so this is nearly an equality.
    max_position_gap: f64,
    /// ADR 0034 §10 G1, second bullet: `in_place_rate(canonical) ≥ 0.95` for
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
/// a parameter there.
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
        "the corpus is {} pairs where {EXPECTED_PAIRS} is what ADR 0034 §10's table and §14 \
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

/// **ADR 0034 §10 G1.** Two spellings of one computation cost the same.
#[test]
#[ignore = "ADR 0034 §10 G1: red until §11 S4 (slot frames) lands, and armed by having been \
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
        "ADR 0034 §10 G1 is not met on {} of {} pairs:\n  - {}",
        failures.len(),
        rows.len(),
        failures.join("\n  - "),
    );
}

/// The corpus is five shapes, each pair is two different programs, and each is
/// the program it was pinned as.
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
                "`{}` and `{}` are the same pessimal program, so one of the two shapes ADR 0034 \
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

/// One pinned member: `(in place, total, sites)`.
type Member = (u64, u64, usize);
/// One pinned row: the pair's name, then its canonical member and its pessimal
/// one.
type Row = (&'static str, Member, Member);

/// What each pair costs on this tree, held to the digit.
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
        // `(200, 200, 1)` under `PLY_ADR0033_PROBE=1`, which is ADR 0025's P2: a parameter may
        // then appear in a `Dead` set. Off by default, because P2 costs more allocations on the
        // request path than it saves there — ADR 0034 §11 S3 — so this pin is the default tree's.
        ("let binding against parameter", (200, 200, 1), (0, 200, 1)),
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
