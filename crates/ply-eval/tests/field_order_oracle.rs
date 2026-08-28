//! `W0611` against the counter it predicts.
//!
//! A lint that agrees with no measurement is a claim, not a check —
//! `CONTRIBUTING.md` records seven of those across sixteen milestones, and they
//! are the most expensive defect this project finds. So the pass is not tested
//! against its own intent. It is tested against `ply_eval::rc::Stats`, taken on
//! the same definitions the pass judged, at two sizes.
//!
//! The arms and the bars are fixed in `PREREGISTRATION.md` §2 and §3, written
//! before this file existed and before any binary did. They are restated here
//! as constants so that moving one is a visible edit rather than a quiet
//! recalibration:
//!
//! - **R2** — every linear arm has `in_place >= 0.90`, every quadratic arm
//!   `<= 0.10`, at n = 200 and n = 400. Anything in the open interval between
//!   is a **reject**, reported with its number, not reclassified.
//! - **R3** — the set of definitions the lint fires in over each spike file is
//!   **exactly** the quadratic set. A miss in either direction is a defect.
//! - **R4** — a ratio moves by no more than 0.02 across the doubling, so it is
//!   measuring the trap rather than start-up.
//!
//! No wall clock anywhere in this file. That is the point of Part B: these are
//! counts, they reproduce to the digit on a loaded machine, and this test can
//! therefore live in the ordinary suite instead of in `ci-shards.sh`'s
//! `DEFERRED` table with the timing benchmarks.

use ply_eval::rc;
use ply_eval::{Machine, Value};
use ply_span::{SourceMap, Span};
use ply_syntax::ast::{Item, ModuleName, Program};
use ply_syntax::parse_program;
use ply_syntax::resolve::{Resolved, resolve};
use std::path::{Path, PathBuf};

/// R2's bars, from `PREREGISTRATION.md` §3.
const LINEAR_FLOOR: f64 = 0.90;
const QUADRATIC_CEILING: f64 = 0.10;
/// R4's tolerance across the doubling.
const DRIFT: f64 = 0.02;
/// R1's repeat count.
const REPEATS: usize = 3;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the crate sits two levels under the workspace root")
        .to_path_buf()
}

/// One `.ply` file plus whatever standard-library modules it imports, which is
/// all any arm here needs.
fn load(path: &Path) -> (Program, Resolved, SourceMap) {
    let text = std::fs::read_to_string(path).expect("the arm's file is in the repository");
    let stem = path.file_name().expect("a file").to_str().expect("utf-8");
    load_source(path.to_string_lossy().as_ref(), stem, &text)
}

fn load_source(display: &str, file_name: &str, text: &str) -> (Program, Resolved, SourceMap) {
    let mut map = SourceMap::new();
    let name = ModuleName::from_relative_path(Path::new(file_name)).expect("a module name");
    let text = text.to_string();
    let id = map.add(display, text.clone());
    let mut loaded = vec![(id, name, text)];

    let mut next = 0;
    while next < loaded.len() {
        let (id, name, text) = loaded[next].clone();
        next += 1;
        let Ok(module) = ply_syntax::parse_module(id, name, &text) else {
            continue;
        };
        for import in &module.imports {
            let m = import.module_name();
            if !ply_std::is_std(&m) || loaded.iter().any(|(_, n, _)| *n == m) {
                continue;
            }
            let Some(source) = ply_std::source(&m) else {
                continue;
            };
            let id = map.add(ply_std::pseudo_path(&m), source.to_string());
            loaded.push((id, m, source.to_string()));
        }
    }

    let inputs: Vec<_> = loaded
        .iter()
        .map(|(id, name, text)| (*id, name.clone(), text.as_str()))
        .collect();
    let mut program = parse_program(inputs).expect("the arm's file parses");
    assert!(
        ply_derive::expand_program(&mut program).is_empty(),
        "derive expansion failed"
    );
    let resolved = resolve(&program).expect("the arm's file resolves");
    (program, resolved, map)
}

/// The simple names of the definitions `W0611` fires in, sorted and deduplicated.
fn firing_names(program: &Program, resolved: &Resolved, source: &Path) -> Vec<String> {
    let wanted = program
        .modules
        .iter()
        .find(|m| {
            m.items.iter().any(|i| matches!(i, Item::Fn(_)))
                && source.file_name().is_some_and(|f| {
                    m.name.as_str() == f.to_string_lossy().trim_end_matches(".ply")
                })
        })
        .map(|m| m.name.clone());
    let mut names: Vec<String> = ply_core::fieldorder::firings(program, resolved)
        .into_iter()
        .filter(|f| match &wanted {
            // Only the arm's own file. A standard-library module pulled in by an
            // import is not this arm's to judge.
            Some(name) => f.definition.as_str().starts_with(&format!("{name}.")),
            None => true,
        })
        .map(|f| f.simple.as_str().to_string())
        .collect();
    names.sort();
    names.dedup();
    names
}

/// `updates_in_place / updates` over one call, engine pinned to the machine.
///
/// The machine and not the tree-walker because that is where the mechanism is:
/// all eight `rc::carry` call sites are in `machine.rs`, `frame.rs` and
/// `handler.rs`, and `interp.rs` calls it nowhere. A tree-walk figure would be
/// a number about a different evaluator.
fn ratio(program: &Program, resolved: &Resolved, entry: &str, n: i64) -> (f64, u64, u64) {
    let before = rc::stats();
    let mut machine = Machine::for_program(program, resolved);
    let answer = machine.call(entry, vec![Value::Int(n)], Span::DUMMY);
    assert!(answer.is_ok(), "`{entry}({n})` failed: {:?}", answer.err());
    let after = rc::stats();
    let updates = after.updates - before.updates;
    let in_place = after.updates_in_place - before.updates_in_place;
    assert!(
        updates > 0,
        "`{entry}({n})` performed no `push` at all, so the ratio would be vacuous"
    );
    (in_place as f64 / updates as f64, updates, in_place)
}

struct Arm {
    id: &'static str,
    entry: &'static str,
    definition: &'static str,
    quadratic: bool,
}

/// `spikes/ply-lexer-rc/fieldorder.ply`, `PREREGISTRATION.md` §2's Q1, L1, L2.
const FIELDORDER: &[Arm] = &[
    Arm {
        id: "Q1",
        entry: "fieldorder.slow",
        definition: "slow_step",
        quadratic: true,
    },
    Arm {
        id: "L1",
        entry: "fieldorder.fast",
        definition: "fast_step",
        quadratic: false,
    },
    Arm {
        id: "L2",
        entry: "fieldorder.bare",
        definition: "bare_step",
        quadratic: false,
    },
];

/// `spikes/ply-lexer-nesting/nesting.ply`, §2's Q2, Q3, Q4, L3, L4, L5.
const NESTING: &[Arm] = &[
    Arm {
        id: "Q2",
        entry: "nesting.first",
        definition: "first_step",
        quadratic: true,
    },
    Arm {
        id: "Q3",
        entry: "nesting.firstc",
        definition: "first_const_step",
        quadratic: true,
    },
    Arm {
        id: "Q4",
        entry: "nesting.grouped_one",
        definition: "push_step",
        quadratic: true,
    },
    Arm {
        id: "L3",
        entry: "nesting.tail",
        definition: "tail_step",
        quadratic: false,
    },
    Arm {
        id: "L4",
        entry: "nesting.last",
        definition: "last_step",
        quadratic: false,
    },
];

fn spike(name: &str) -> PathBuf {
    workspace_root().join("spikes").join(name)
}

/// **R3.** The firing set over a whole file, compared with the set fixed in
/// advance. Set equality, so a miss fails as loudly as a false alarm.
#[test]
fn the_lint_fires_on_exactly_the_quadratic_definitions() {
    for (file, arms, extra_silent) in [
        (
            spike("ply-lexer-rc/fieldorder.ply"),
            FIELDORDER,
            vec!["empty", "slow", "fast", "bare"],
        ),
        (
            spike("ply-lexer-nesting/nesting.ply"),
            NESTING,
            // L5 `node` is the fast spelling by the stated rule, and `keep`,
            // `keep_snd` and `keepl` return an argument untouched.
            vec!["node", "keep", "keep_snd", "keepl", "empty", "one_list"],
        ),
    ] {
        let (program, resolved, _) = load(&file);
        let fired = firing_names(&program, &resolved, &file);

        let mut expected: Vec<String> = arms
            .iter()
            .filter(|a| a.quadratic)
            .map(|a| a.definition.to_string())
            .collect();
        expected.sort();

        assert_eq!(
            fired,
            expected,
            "W0611's firing set over {} is not the set fixed in PREREGISTRATION.md §2",
            file.display()
        );
        for name in extra_silent {
            assert!(
                !fired.iter().any(|f| f == name),
                "W0611 fired in `{name}`, which is written the fast way"
            );
        }
    }
}

/// **R2 and R4.** Where the lint fires, the counter must show the copy.
#[test]
fn every_firing_is_a_measured_copy_and_every_silence_is_a_measured_reuse() {
    for (file, arms) in [
        (spike("ply-lexer-rc/fieldorder.ply"), FIELDORDER),
        (spike("ply-lexer-nesting/nesting.ply"), NESTING),
    ] {
        let (program, resolved, _) = load(&file);
        let fired = firing_names(&program, &resolved, &file);

        for arm in arms {
            let lint_fired = fired.iter().any(|f| f == arm.definition);
            assert_eq!(
                lint_fired, arm.quadratic,
                "{}: the lint and the pre-registration disagree before any number was taken",
                arm.id
            );

            let (small, u_small, p_small) = ratio(&program, &resolved, arm.entry, 200);
            let (large, u_large, p_large) = ratio(&program, &resolved, arm.entry, 400);
            println!(
                "{} {:<18} lint={:<5} n=200 {}/{} = {:.4}   n=400 {}/{} = {:.4}",
                arm.id,
                arm.definition,
                lint_fired,
                p_small,
                u_small,
                small,
                p_large,
                u_large,
                large
            );

            // R2. The bars are not moved by this test; a value between them is
            // a reject reported with its number.
            for (n, got) in [(200, small), (400, large)] {
                if arm.quadratic {
                    assert!(
                        got <= QUADRATIC_CEILING,
                        "{} `{}` at n={n}: in_place = {got:.4}, above the {QUADRATIC_CEILING} \
                         ceiling R2 fixed for a quadratic arm. The lint fires here; the counter \
                         does not agree.",
                        arm.id,
                        arm.definition
                    );
                } else {
                    assert!(
                        got >= LINEAR_FLOOR,
                        "{} `{}` at n={n}: in_place = {got:.4}, below the {LINEAR_FLOOR} floor \
                         R2 fixed for a linear arm. The lint is silent here; the counter does \
                         not agree.",
                        arm.id,
                        arm.definition
                    );
                }
            }

            // R4. A ratio that drifts with n is measuring start-up.
            assert!(
                (small - large).abs() <= DRIFT,
                "{} `{}`: in_place moved {:.4} across the doubling, more than R4's {DRIFT}",
                arm.id,
                arm.definition,
                (small - large).abs()
            );
        }
    }
}

/// **R1.** The oracle rests on the counters being a function of the program.
/// If they are not, no ratio above means anything, so this is checked rather
/// than assumed.
#[test]
fn the_counters_are_a_function_of_the_program_and_not_of_the_run() {
    let file = spike("ply-lexer-nesting/nesting.ply");
    let (program, resolved, _) = load(&file);
    for arm in NESTING {
        let mut seen = Vec::new();
        for _ in 0..REPEATS {
            seen.push(ratio(&program, &resolved, arm.entry, 200));
        }
        assert!(
            seen.windows(2).all(|w| w[0] == w[1]),
            "{} `{}`: {REPEATS} runs disagreed — {seen:?}. The counter is not deterministic and \
             every ratio in this file is void.",
            arm.id,
            arm.definition
        );
    }
}

/// **Q5's counter cross-check, and the one place R2's band does not apply.**
///
/// `escape_runs` contains **two** `push`es — `push(push(acc, run), escaped)` —
/// and the lint fires on exactly one of them, the inner. The outer receives the
/// list the inner just produced, which has one owner, so it rewrites in place.
/// A definition-level `updates_in_place / updates` therefore sits at **0.50**,
/// measured at k = 100, 200 and 400: 0.5025, 0.5012, 0.5006.
///
/// **That is a REJECT under `PREREGISTRATION.md` §3's R2**, whose band is
/// `<= 0.10` for a quadratic arm, and the bar is not moved and the arm is not
/// reclassified. What is recorded instead is *why* the statistic cannot decide
/// it: `in_place` is a fraction of `push` **operations** over a whole run, and
/// `rc::note_update` carries no span, so a definition where one site copies and
/// another does not averages the two. The ten spike arms each have a single
/// `push` and land at exactly 0.0 or exactly 1.0; this one cannot.
///
/// The claim that *can* be checked, and is checked here, is sharper than a band
/// anyway: the number of copying updates is **exactly k**, one per escape, at
/// every size. One copy per escape is one copy per firing.
///
/// Making the band work here needs `note_update` to carry the site's `Span` so
/// counters can be attributed per push. That is a real change to `rc.rs` and it
/// is named rather than smuggled in.
#[test]
fn the_shipped_quadratic_copies_exactly_once_per_escape() {
    const SOURCE: &str = r#"
import std.json (Json, Str, to_string)

fn quotes(k: Int) -> String =
  fold(range(0, k), "", |a: String, i: Int| string_concat(a, "\""))

pub fn escapes(k: Int) -> Int = string_len(to_string(Str(quotes(k))))
"#;
    let (program, resolved, _) = load_source("<escapes>", "escapes.ply", SOURCE);

    let mut seen = Vec::new();
    for k in [100i64, 200, 400] {
        let before = rc::stats();
        let mut machine = Machine::for_program(&program, &resolved);
        let answer = machine.call("escapes.escapes", vec![Value::Int(k)], Span::DUMMY);
        assert!(answer.is_ok(), "`escapes({k})` failed: {:?}", answer.err());
        let after = rc::stats();
        let updates = after.updates - before.updates;
        let in_place = after.updates_in_place - before.updates_in_place;
        let copies = updates - in_place;
        println!("k={k:<4} updates={updates:<5} in_place={in_place:<5} copies={copies}");
        assert_eq!(
            copies, k as u64,
            "`escape_runs` copied {copies} times for {k} escapes. The lint names one site and              the run must show one copy per escape at it."
        );
        seen.push((updates, in_place));
    }

    // The two pushes per escape, which is where the 0.50 comes from. Asserted
    // so that a future `escape_runs` with a different shape cannot leave the
    // explanation above standing while the code has moved.
    for ((updates, in_place), k) in seen.iter().zip([100u64, 200, 400]) {
        assert_eq!(*updates, 2 * k + 1);
        assert_eq!(*in_place, k + 1);
    }
}

/// **Q5, the shipped defect.** `crates/ply-std/ply/json.ply`'s `escape_runs`
/// builds its accumulator at argument 0 of 2 of the outer `push`.
///
/// **Cited, not repaired.** A separate workstream owns that fix; this test
/// needs it unfixed, and will fail loudly rather than quietly pass if it lands
/// — which is the right way round, because the counter check below is what
/// would have to be re-taken.
#[test]
fn the_lint_fires_on_the_shipped_quadratic_in_the_json_serializer() {
    let json = ply_std::modules()
        .find(|m| m.as_str().ends_with("json"))
        .expect("`std.json` ships with the compiler");
    let source = ply_std::source(&json).expect("a shipped module has its source");
    let mut map = SourceMap::new();
    let id = map.add(ply_std::pseudo_path(&json), source.to_string());
    let mut program = parse_program(vec![(id, json, source)]).expect("the standard library parses");
    assert!(ply_derive::expand_program(&mut program).is_empty());
    let resolved = resolve(&program).expect("the standard library resolves");

    let firings = ply_core::fieldorder::firings(&program, &resolved);
    let in_escape_runs: Vec<_> = firings
        .iter()
        .filter(|f| f.simple.as_str() == "escape_runs")
        .collect();
    assert_eq!(
        in_escape_runs.len(),
        1,
        "expected exactly the inner `push(acc, ...)` of `escape_runs` to fire, got {:?}",
        firings
            .iter()
            .map(|f| f.simple.as_str())
            .collect::<Vec<_>>()
    );

    // The site is the *inner* push, not the outer one — the outer is the last
    // argument of the recursive call and is fine. Checked against the text so
    // that a firing on the wrong one of the two cannot pass.
    let span = in_escape_runs[0].span;
    let text = &source[span.start as usize..span.end as usize];
    assert!(
        text.starts_with("push(acc,"),
        "W0611 points at `{text}`, which is not the inner `push`"
    );
}

// --- the round-2 probe table -------------------------------------------------

/// One probe: a program whose stepping definition is the shape under test.
struct Probe {
    id: &'static str,
    /// What the shape is, for the failure message.
    shape: &'static str,
    source: &'static str,
    /// The definition `W0611` is expected to speak about, or not.
    definition: &'static str,
    /// The entry point's body, which folds `step` `n` times and reads the
    /// answer out. Spelled per probe rather than inferred, because a shape the
    /// harness guesses wrong is a probe that measures something else.
    tail: &'static str,
    /// `true` when the counter is expected in R2's quadratic band.
    quadratic: bool,
    /// A shape the pass is known **not** to decide, in either direction. The
    /// band still applies — the counter is the counter — but the lint is
    /// expected to disagree with it, and the disagreement is asserted so that
    /// closing the gap is a red test rather than a silent improvement nobody
    /// records.
    disagrees: bool,
}

/// Every shape the round-2 rebuild of `W0611` rests on, with the number that
/// decided it.
///
/// The findings that refuted the round-1 pass are `B1`–`B3` (silent, measured
/// 0.0) and `D1`/`E1` (fired, measured 1.0). `F1`, `K1` and `M1` are the limits
/// that remain, asserted as disagreements so that closing one is a red test.
const PROBES: &[Probe] = &[
    Probe {
        id: "A1",
        shape: "`push` in the last record field, nothing holding the list",
        source: "type S = { pos: Int, toks: List<Int> }
fn empty() -> S = {pos: 0, toks: []}
fn step(s: S, i: Int) -> S = { pos: i, toks: push(s.toks, i) }",
        definition: "step",
        tail: "len(fold(range(0, n), empty(), step).toks)",
        quadratic: false,
        disagrees: false,
    },
    Probe {
        id: "A2",
        shape: "`push` in field 1 of 3, so the scope is carried past it",
        source: "type S = { pos: Int, toks: List<Int>, tail: Int }
fn empty() -> S = {pos: 0, toks: [], tail: 0}
fn step(s: S, i: Int) -> S = { pos: i, toks: push(s.toks, i), tail: s.tail }",
        definition: "step",
        tail: "len(fold(range(0, n), empty(), step).toks)",
        quadratic: true,
        disagrees: false,
    },
    Probe {
        id: "B1",
        shape: "the LAST record field, with the first field holding the same list",
        source: "type S = { keep: List<Int>, toks: List<Int> }
fn empty() -> S = {keep: [], toks: []}
fn step(s: S, i: Int) -> S = { keep: s.toks, toks: push(s.toks, i) }",
        definition: "step",
        tail: "len(fold(range(0, n), empty(), step).toks)",
        quadratic: true,
        disagrees: false,
    },
    Probe {
        id: "B2",
        shape: "the LAST call argument, with the first argument holding the list",
        source: "fn empty() -> List<Int> = []
fn snd(a: List<Int>, b: List<Int>) -> List<Int> = b
fn step(s: List<Int>, i: Int) -> List<Int> = snd(s, push(s, i))",
        definition: "step",
        tail: "len(fold(range(0, n), empty(), step))",
        quadratic: true,
        disagrees: false,
    },
    Probe {
        id: "B3",
        shape: "the LAST list item, with the first item holding the list",
        source: "fn empty() -> List<Int> = []
fn last_of(ls: List<List<Int>>) -> List<Int> = fold(ls, [], |a: List<Int>, x: List<Int>| x)
fn step(xs: List<Int>, i: Int) -> List<Int> = last_of([xs, push(xs, i)])",
        definition: "step",
        tail: "len(fold(range(0, n), empty(), step))",
        quadratic: true,
        disagrees: false,
    },
    Probe {
        id: "C1",
        shape: "an earlier sibling that mentions the root and keeps nothing",
        source: "type S = { n: Int, toks: List<Int> }
fn empty() -> S = {n: 0, toks: []}
fn step(s: S, i: Int) -> S = { n: len(s.toks), toks: push(s.toks, i) }",
        definition: "step",
        tail: "len(fold(range(0, n), empty(), step).toks)",
        quadratic: false,
        disagrees: false,
    },
    Probe {
        id: "C2",
        shape: "an earlier sibling that is a different field of the same record",
        source: "type S = { pos: Int, toks: List<Int> }
fn empty() -> S = {pos: 0, toks: []}
fn step(s: S, i: Int) -> S = { pos: s.pos, toks: push(s.toks, i) }",
        definition: "step",
        tail: "len(fold(range(0, n), empty(), step).toks)",
        quadratic: false,
        disagrees: false,
    },
    Probe {
        id: "D1",
        shape: "a call at argument 0 of 2 whose only `push` is onto a fresh list",
        source: "fn empty() -> List<Int> = []
fn small(i: Int) -> Int = len(push([], i))
fn keepr(a: Int, b: List<Int>) -> List<Int> = b
fn step(s: List<Int>, i: Int) -> List<Int> = keepr(small(i), push(s, i))",
        definition: "step",
        tail: "len(fold(range(0, n), empty(), step))",
        quadratic: false,
        disagrees: false,
    },
    Probe {
        id: "E1",
        shape: "a call at argument 0 of 2 that folds onto its own fresh accumulator",
        source: "fn empty() -> Int = 0
fn build(k: Int) -> List<Int> = fold(range(0, k), [], |a: List<Int>, j: Int| push(a, j))
fn keepi(a: Int, b: Int) -> Int = a
fn step(a: Int, i: Int) -> Int = keepi(len(build(10)), i)",
        definition: "step",
        tail: "fold(range(0, n), empty(), step)",
        quadratic: false,
        disagrees: false,
    },
    Probe {
        id: "G3",
        shape: "a record literal evaluated in the order it is written, not alphabetically",
        source: "type S = { a: Int, b: List<Int> }
fn empty() -> S = {a: 0, b: []}
fn step(s: S, i: Int) -> S = { b: push(s.b, i), a: 0 }",
        definition: "step",
        tail: "len(fold(range(0, n), empty(), step).b)",
        quadratic: true,
        disagrees: false,
    },
    Probe {
        id: "G4",
        shape: "an earlier sibling whose value is a closure, which captured the scope",
        source: "type S = { f: (Int) -> Int, toks: List<Int> }
fn empty() -> S = {f: |z: Int| z, toks: []}
fn step(s: S, i: Int) -> S = { f: |z: Int| len(s.toks) + z, toks: push(s.toks, i) }",
        definition: "step",
        tail: "len(fold(range(0, n), empty(), step).toks)",
        quadratic: true,
        disagrees: false,
    },
    Probe {
        id: "G2",
        shape: "the same closure passed to a call, which is gone before the next field",
        source: "type S = { n: Int, toks: List<Int> }
fn empty() -> S = {n: 0, toks: []}
fn mk(f: (Int) -> Int) -> Int = f(1)
fn step(s: S, i: Int) -> S = { n: mk(|z: Int| len(s.toks) + z), toks: push(s.toks, i) }",
        definition: "step",
        tail: "len(fold(range(0, n), empty(), step).toks)",
        quadratic: false,
        disagrees: false,
    },
    Probe {
        id: "H1",
        shape: "a `push` under a field access, which holds neither scope nor value",
        source: "type W = { xs: List<Int> }
fn empty() -> List<Int> = []
fn wrap(a: List<Int>) -> W = { xs: a }
fn step(xs: List<Int>, i: Int) -> List<Int> = wrap(push(xs, i)).xs",
        definition: "step",
        tail: "len(fold(range(0, n), empty(), step))",
        quadratic: false,
        disagrees: false,
    },
    Probe {
        id: "I1",
        shape: "a growing call in the LAST argument, with the first holding what it grows",
        source: "fn empty() -> List<Int> = []
fn grow(xs: List<Int>, i: Int) -> List<Int> = push(xs, i)
fn snd(a: List<Int>, b: List<Int>) -> List<Int> = b
fn step(s: List<Int>, i: Int) -> List<Int> = snd(s, grow(s, i))",
        definition: "step",
        tail: "len(fold(range(0, n), empty(), step))",
        quadratic: true,
        disagrees: false,
    },
    Probe {
        id: "I2",
        shape: "the same, where the kept place is a field of what the call was handed",
        source: "type S = { pos: Int, toks: List<Int> }
fn empty() -> S = {pos: 0, toks: []}
fn node(s: S, i: Int) -> S = { pos: i, toks: push(s.toks, i) }
fn keep(a: List<Int>, b: S) -> S = b
fn step(s: S, i: Int) -> S = keep(s.toks, node(s, i))",
        definition: "step",
        tail: "len(fold(range(0, n), empty(), step).toks)",
        quadratic: true,
        disagrees: false,
    },
    Probe {
        id: "J1",
        shape: "an earlier sibling that wraps the list in a list literal",
        source: "type S = { keep: List<List<Int>>, toks: List<Int> }
fn empty() -> S = {keep: [], toks: []}
fn step(s: S, i: Int) -> S = { keep: [s.toks], toks: push(s.toks, i) }",
        definition: "step",
        tail: "len(fold(range(0, n), empty(), step).toks)",
        quadratic: true,
        disagrees: false,
    },
    Probe {
        id: "J2",
        shape: "a lambda parameter shadowing the definition's, folding onto `[]`",
        source: "fn empty() -> Int = 0
fn keepi(a: Int, b: Int) -> Int = a
fn build(xs: List<Int>) -> List<Int> = fold(range(0, 10), [], |xs: List<Int>, j: Int| push(xs, j))
fn step(a: Int, i: Int) -> Int = keepi(len(build([])), i)",
        definition: "step",
        tail: "fold(range(0, n), empty(), step)",
        quadratic: false,
        disagrees: false,
    },
    Probe {
        id: "M1",
        shape: "a carried `push` onto a call's result — a measured false positive",
        source: "fn empty() -> Int = 0
fn mk(i: Int) -> List<Int> = [i]
fn sink(a: List<Int>, b: Int) -> Int = len(a)
fn step(a: Int, i: Int) -> Int = sink(push(mk(i), i), i)",
        definition: "step",
        tail: "fold(range(0, n), empty(), step)",
        quadratic: false,
        disagrees: true,
    },
    Probe {
        id: "L1",
        shape: "a `push` onto a fresh list, in a carried position",
        source: "fn empty() -> Int = 0
fn sink(a: List<Int>, b: Int) -> Int = len(a)
fn step(a: Int, i: Int) -> Int = sink(push([], i), i)",
        definition: "step",
        tail: "fold(range(0, n), empty(), step)",
        quadratic: false,
        disagrees: false,
    },
    Probe {
        id: "K1",
        shape: "a `push` onto a list read out of a `Map` — the other known miss",
        source: "fn empty() -> Map<Int, List<Int>> = map_insert(map_new(), 0, [])
fn step(m: Map<Int, List<Int>>, i: Int) -> Map<Int, List<Int>> =
  match map_get(m, 0) {
    None -> m,
    Some(vs) -> map_insert(m, 0, push(vs, i)),
  }",
        definition: "step",
        tail: "match map_get(fold(range(0, n), empty(), step), 0) { None -> 0, Some(vs) -> len(vs) }",
        quadratic: true,
        disagrees: true,
    },
    Probe {
        id: "F1",
        shape: "an earlier sibling that holds the list through a call — the known miss",
        source: "type S = { n: List<Int>, toks: List<Int> }
fn empty() -> S = {n: [], toks: []}
fn id(x: List<Int>) -> List<Int> = x
fn step(s: S, i: Int) -> S = { n: id(s.toks), toks: push(s.toks, i) }",
        definition: "step",
        tail: "len(fold(range(0, n), empty(), step).toks)",
        quadratic: true,
        disagrees: true,
    },
];

/// The whole entry point a probe needs, folded `n` times and finished so the
/// counters describe one shape and nothing else.
fn probe_source(p: &Probe) -> String {
    format!("{}\npub fn run(n: Int) -> Int = {}\n", p.source, p.tail)
}

/// **The criterion this workstream exists to meet.** Every shape in the table,
/// at two sizes, with the lint's answer and the counter's answer compared.
///
/// A round-1 review reproduced five defects on programs of this size with these
/// counters as the oracle, so the table is the repair and the check at once. It
/// asserts R2's bands, R4's drift and R3's agreement in one loop, and it names
/// the one shape where agreement is not expected rather than leaving it out.
#[test]
fn every_probe_shape_agrees_with_the_counter_at_both_sizes() {
    for p in PROBES {
        let text = probe_source(p);
        let (program, resolved, _) = load_source(&format!("<{}>", p.id), "probe.ply", &text);

        let fired = ply_core::fieldorder::firings(&program, &resolved)
            .iter()
            .filter(|f| f.simple.as_str() == p.definition)
            .count();

        let (small, u_s, p_s) = ratio(&program, &resolved, "probe.run", 200);
        let (large, u_l, p_l) = ratio(&program, &resolved, "probe.run", 400);
        println!(
            "{:<3} n=200 {}/{} = {:.4}   n=400 {}/{} = {:.4}   fired={}   {}",
            p.id, p_s, u_s, small, p_l, u_l, large, fired, p.shape
        );

        for (n, got) in [(200, small), (400, large)] {
            if p.quadratic {
                assert!(
                    got <= QUADRATIC_CEILING,
                    "{} ({}) at n={n}: in_place = {got:.4}, above R2's {QUADRATIC_CEILING} \
                     ceiling for a quadratic shape",
                    p.id,
                    p.shape
                );
            } else {
                assert!(
                    got >= LINEAR_FLOOR,
                    "{} ({}) at n={n}: in_place = {got:.4}, below R2's {LINEAR_FLOOR} floor for a \
                     linear shape",
                    p.id,
                    p.shape
                );
            }
        }
        assert!(
            (small - large).abs() <= DRIFT,
            "{}: in_place moved {:.4} across the doubling, more than R4's {DRIFT}",
            p.id,
            (small - large).abs()
        );

        let expected = p.quadratic != p.disagrees;
        assert_eq!(
            fired > 0,
            expected,
            "{} ({}): the lint {} and the counter says {:.4}. {}",
            p.id,
            p.shape,
            if fired > 0 { "fires" } else { "is silent" },
            small,
            if p.disagrees {
                "This shape is recorded in `fieldorder.rs`'s module comment as one the pass does \
                 not decide; if it now agrees with the counter, take the row out of that table."
            } else {
                "One of the two is wrong and it is not the counter."
            }
        );
    }
}

/// **R1 over the round-2 table.** Three runs of every probe must agree exactly.
#[test]
fn every_probe_counts_the_same_thing_three_times() {
    for p in PROBES {
        let text = probe_source(p);
        let (program, resolved, _) = load_source(&format!("<{}>", p.id), "probe.ply", &text);
        let mut seen = Vec::new();
        for _ in 0..REPEATS {
            seen.push(ratio(&program, &resolved, "probe.run", 200));
        }
        assert!(
            seen.windows(2).all(|w| w[0] == w[1]),
            "{}: {REPEATS} runs disagreed — {seen:?}",
            p.id
        );
    }
}
