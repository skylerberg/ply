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
