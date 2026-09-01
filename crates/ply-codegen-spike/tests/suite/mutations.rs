//! Proof that the agreement checks bite: a backend that is wrong on purpose, and the comparison
//! catching it.

use ply_codegen_spike::entry::admissible;
use ply_codegen_spike::jit::Opts;
use ply_codegen_spike::measure::Harness;
use ply_codegen_spike::program::Loaded;
use ply_codegen_spike::wrong::{Ended, Mutant, Mutation, run_guarded};
use ply_eval::{Machine, Value, compare_answers};
use ply_span::Span;
use std::path::PathBuf;
use std::process::Command;
use std::rc::Rc;

fn hazards() -> &'static Loaded {
    Box::leak(Box::new(
        Loaded::project(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests")
                .join("fixtures")
                .join("hazards"),
        )
        .expect("the hazard fixtures load"),
    ))
}

/// The `pure` module compiled and offered, every other module interpreted — the same shape
/// `hazards.rs` measures, so a mutation here is a mutation of the arrangement the audit already
/// trusts.
fn pure_harness(loaded: &'static Loaded) -> Harness {
    let all = loaded.functions_in("pure");
    let accepted = admissible(loaded, &all).expect("`pure` classifies");
    assert_eq!(
        accepted.len(),
        all.len(),
        "the fragment refused part of `pure`"
    );
    let names: Vec<&str> = accepted.iter().map(|s| s.as_str()).collect();
    Harness::over(loaded, &names, Opts::default(), None).expect("`pure` compiles")
}

/// A harness whose hybrid machine is driven by `mutation`.
fn mutated(harness: &mut Harness, mutation: Mutation, target: Option<&str>) -> Rc<Mutant> {
    let mutant = match target {
        Some(name) => Mutant::over(harness.bodies.clone(), mutation, name),
        None => Mutant::new(harness.bodies.clone(), mutation),
    };
    harness.set_backend(mutant.clone());
    mutant
}

/// Both engines over one call, compared on everything `differential` compares: the value, and on a
/// raise the code, the message, every label with its span, the notes, the observed footprint and
/// the cell arena.
fn disagreement(harness: &mut Harness, name: &str, args: &[Value]) -> Option<String> {
    let expected = harness.interpret_outcome(name, args);
    let hybrid = harness.hybrid_outcome(name, args);
    compare_answers(&harness.machine, &harness.hybrid, name, &expected, &hybrid)
        .map(|d| d.to_string())
}

#[track_caller]
fn caught(harness: &mut Harness, name: &str, args: &[Value]) -> String {
    match disagreement(harness, name, args) {
        Some(d) => d,
        None => panic!(
            "`{name}` agreed with a backend that was wrong on purpose: the comparison the corpus \
             runs did not report it"
        ),
    }
}

/// The MCTS kernel of `benches/kernel`, with the fragment's largest compiled subset offered — the
/// arrangement `src/bin/mcts.rs` measures.
fn kernel() -> &'static Loaded {
    Box::leak(Box::new(
        Loaded::project(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("..")
                .join("benches")
                .join("kernel"),
        )
        .expect("the kernel loads"),
    ))
}

fn kernel_harness(loaded: &'static Loaded) -> Harness {
    let all = loaded.functions_in("mcts");
    let accepted = admissible(loaded, &all).expect("the module classifies");
    let names: Vec<&str> = accepted.iter().map(|s| s.as_str()).collect();
    Harness::over(loaded, &names, Opts::default(), Some("work.zero")).expect("it compiles")
}

/// The control every other test is read against: the wrapper itself changes nothing.
#[test]
fn the_wrapper_with_no_mutation_changes_no_answer() {
    let loaded = hazards();
    let mut harness = pure_harness(loaded);
    let mutant = mutated(&mut harness, Mutation::None, None);

    for n in [0i64, 1, 7, 998, 100_000] {
        for subject in ["pure.step", "pure.even"] {
            if let Some(d) = disagreement(&mut harness, subject, &[Value::Int(n)]) {
                panic!("`{subject}({n})` disagreed with an honest backend: {d}");
            }
        }
        if let Some(d) = disagreement(
            &mut harness,
            "deep.countdown",
            &[Value::Int(n % 40), Value::Int(1)],
        ) {
            panic!("`deep.countdown({n})` disagreed with an honest backend: {d}");
        }
    }
    assert_eq!(mutant.fired(), 0, "the honest wrapper changed an answer");
    assert!(
        mutant.offered() > 0 && harness.hybrid_counts().0 > 0,
        "nothing was entered, so the tests below would be mutating a seam nobody reaches: \
         {} offers, {:?} counts",
        mutant.offered(),
        harness.hybrid_counts()
    );
}

/// 1.
#[test]
fn an_off_by_one_in_a_compiled_answer_is_caught() {
    let loaded = hazards();
    let mut harness = pure_harness(loaded);
    let mutant = mutated(&mut harness, Mutation::OffByOne, Some("pure.mix"));

    // Directly, where the wrong `Int` is the answer itself.
    let d = caught(&mut harness, "pure.mix", &[Value::Int(3), Value::Int(4)]);
    assert!(
        d.contains("value"),
        "the divergence was not on the value: {d}"
    );

    // And underneath an interpreted recursion, where it is one addend of many — the shape a real
    // off-by-one takes.
    caught(
        &mut harness,
        "deep.countdown",
        &[Value::Int(12), Value::Int(1)],
    );
    assert!(mutant.fired() > 0, "the mutation never fired");
}

/// 2.
#[test]
fn an_inverted_compiled_comparison_is_caught() {
    let loaded = hazards();
    let mut harness = pure_harness(loaded);
    let mutant = mutated(&mut harness, Mutation::Inverted, Some("pure.even"));

    for n in [0i64, 1, 2, 3] {
        caught(&mut harness, "pure.even", &[Value::Int(n)]);
    }
    assert!(mutant.fired() > 0, "the mutation never fired");
}

/// 3.
#[test]
fn a_stale_compiled_answer_is_caught() {
    let loaded = hazards();
    let mut harness = pure_harness(loaded);
    let mutant = mutated(&mut harness, Mutation::Stale, None);

    let mut disagreements = 0;
    for n in [1i64, 2, 3, 4, 5, 6] {
        if disagreement(&mut harness, "pure.step", &[Value::Int(n)]).is_some() {
            disagreements += 1;
        }
    }
    assert!(mutant.fired() > 0, "the mutation never fired");
    assert!(
        disagreements > 0,
        "six calls with six different arguments, every one of them answered with the previous \
         call's result, and nothing noticed"
    );
}

/// 4.
#[test]
fn a_bool_where_an_int_belongs_crosses_the_seam_and_is_caught_downstream() {
    let loaded = hazards();
    let mut harness = pure_harness(loaded);
    let mutant = mutated(&mut harness, Mutation::WrongType, Some("pure.mix"));

    let d = caught(&mut harness, "pure.mix", &[Value::Int(3), Value::Int(4)]);
    assert!(
        d.contains("value"),
        "the divergence was not on the value: {d}"
    );
    assert_eq!(
        harness.hybrid.compiled_refusals(),
        0,
        "the seam refused the wrong kind after all, which would make this a boundary check \
         rather than a marshalling one"
    );

    // And where the caller does arithmetic on it: the interpreter raises a type error the machine
    // with no backend never sees.
    caught(
        &mut harness,
        "deep.countdown",
        &[Value::Int(3), Value::Int(1)],
    );
    assert!(mutant.fired() > 0, "the mutation never fired");
}

/// 5.
#[test]
fn a_backend_is_never_offered_an_effectful_definition() {
    let loaded = hazards();
    let mut harness = pure_harness(loaded);
    assert!(
        !loaded.check.defs[&ply_span::Symbol::new("effects.measured")]
            .footprint
            .is_empty(),
        "the fixture is wrong: `effects.measured` publishes an empty row"
    );
    // 6,688 is chosen so that `effects.handled(1)` answers exactly what it answers with no backend
    // at all — `mix(6688, 0)` is `mix(mix(7, 1), step(1))` — which leaves the perform and the cell
    // write as the *only* difference.
    let mutant = mutated(
        &mut harness,
        Mutation::Answers(Value::Int(6_688)),
        Some("effects.measured"),
    );

    for n in [1i64, 5, 17] {
        if let Some(d) = disagreement(&mut harness, "effects.handled", &[Value::Int(n)]) {
            panic!("`effects.handled({n})` disagreed with a backend nothing ever asked: {d}");
        }
    }
    assert_eq!(
        mutant.offered_target(),
        0,
        "a definition whose published row is non-empty was offered to a backend"
    );
    assert!(
        mutant.offered() > 0,
        "the seam was never reached at all, so the count above proves nothing"
    );
    assert!(
        harness.hybrid_counts().0 > 0,
        "no native body ran under the handler, so this is not the arrangement it claims to be"
    );
}

/// 6.
#[test]
fn an_answer_for_a_definition_the_backend_has_no_body_for_is_caught() {
    let loaded = hazards();
    let mut harness = pure_harness(loaded);
    let mutant = mutated(&mut harness, Mutation::Unoffered, None);
    assert!(
        !harness
            .bodies
            .admits(&ply_span::Symbol::new("deep.countdown")),
        "`deep.countdown` is compiled after all, so it is the wrong subject"
    );

    caught(
        &mut harness,
        "deep.countdown",
        &[Value::Int(6), Value::Int(1)],
    );
    assert!(mutant.fired() > 0, "the mutation never fired");
}

/// 6b.
#[test]
fn answering_where_the_definition_raises_is_caught() {
    let loaded = hazards();
    let mut harness = pure_harness(loaded);
    let mutant = mutated(
        &mut harness,
        Mutation::Answers(Value::Int(0)),
        Some("pure.share"),
    );

    let d = caught(&mut harness, "pure.share", &[Value::Int(1), Value::Int(0)]);
    assert!(
        d.contains("verdict"),
        "the divergence was not on the verdict: {d}"
    );
    assert!(mutant.fired() > 0, "the mutation never fired");
}

/// 7.
#[test]
fn a_backend_that_runs_past_its_budget_is_caught() {
    let loaded = hazards();
    let harness = pure_harness(loaded);
    let mutant = Mutant::new(harness.bodies.clone(), Mutation::ExceedsBudget(None));

    let mut machine =
        Machine::new(&loaded.ast, &loaded.resolved, &loaded.check).with_max_calls(600);
    let mut hybrid = Machine::new(&loaded.ast, &loaded.resolved, &loaded.check).with_max_calls(600);
    hybrid.set_compiled(mutant.clone());

    let args = vec![Value::Int(1_000), Value::Int(1)];
    let expected = machine.call("pure.ladder", args.clone(), Span::DUMMY);
    let actual = hybrid.call("pure.ladder", args, Span::DUMMY);
    assert!(
        expected.as_ref().err().is_some_and(|d| d
            .message
            .contains("recursion limit of 600 nested calls exceeded")),
        "the machine did not reach its bound, so there is nothing for the backend to outrun"
    );
    assert!(mutant.fired() > 0, "the mutation never fired");
    assert!(
        compare_answers(&machine, &hybrid, "pure.ladder", &expected, &actual).is_some(),
        "a backend that ignored the machine's bound answered {} and nothing reported it",
        actual.map(|v| v.render()).unwrap_or_default()
    );
}

/// What tells this test binary that it is the child and must run the crash rather than guard it.
const CRASH_CHILD: &str = "PLY_SPIKE_CRASH_CHILD";

/// This binary's own name for the test `leaf`, derived rather than written down.
///
/// libtest names a test by its module path *inside the binary*, so moving this file into
/// `tests/suite/` renamed the test below and a spelled-out filter stopped matching it. That fails
/// in the worst direction: a filter matching nothing runs no test and exits **0**, so the child
/// comes back looking like a clean run rather than like a mismatch.
fn own_test_name(leaf: &str) -> String {
    match module_path!().split_once("::") {
        Some((_binary, module)) => format!("{module}::{leaf}"),
        None => leaf.to_string(),
    }
}

/// 7b.
#[test]
fn a_backend_that_ignores_its_budget_kills_the_process_and_is_reported_from_outside_it() {
    if std::env::var_os(CRASH_CHILD).is_some() {
        the_run_that_does_not_come_back();
        // Reached only if the premise died: a native recursion of five million frames that did
        // *not* overflow the stack.
        std::process::exit(77);
    }

    let exe = std::env::current_exe().expect("a test binary knows where it is");
    let name = own_test_name(
        "a_backend_that_ignores_its_budget_kills_the_process_and_is_reported_from_outside_it",
    );
    let mut command = Command::new(exe);
    command
        .args(["--exact", name.as_str(), "--nocapture"])
        .env(CRASH_CHILD, "1");
    let (ended, output) = run_guarded(&mut command).expect("the child runs");

    let how = match &ended {
        Ended::Killed(how) => how.clone(),
        Ended::Exited(status) => panic!(
            "a backend running a five-million-deep native recursion with unlimited fuel came \
             back with {status:?}. Either the hazard this test is about is gone, or the child \
             never ran the test: it was filtered with `--exact {name}`, and a filter that \
             matches nothing exits 0 saying `running 0 tests`. The output says which:\n{output}"
        ),
    };
    assert!(
        output.contains("stack overflow"),
        "the child died ({how}) without overflowing its stack, so it died of something else \
         and this test is measuring that instead:\n{output}"
    );
    let reported = ended
        .as_disagreement()
        .expect("a killed run is a disagreement");
    assert!(
        reported.contains("took the process down"),
        "a killed run was reported as `{reported}`, which does not say what happened"
    );
}

/// The child's half: a backend with unlimited fuel under a machine whose bound is 600, on a
/// recursion deep enough that the native stack is what stops it.
fn the_run_that_does_not_come_back() {
    let loaded = hazards();
    let harness = pure_harness(loaded);
    let mutant = Mutant::new(harness.bodies.clone(), Mutation::ExceedsBudget(None));
    let mut hybrid = Machine::new(&loaded.ast, &loaded.resolved, &loaded.check).with_max_calls(600);
    hybrid.set_compiled(mutant);
    let _ = hybrid.call(
        "pure.ladder",
        vec![Value::Int(5_000_000), Value::Int(1)],
        Span::DUMMY,
    );
}

/// 7c.
#[test]
fn two_raises_that_differ_are_not_agreement_although_both_are_raises() {
    let loaded = hazards();
    let mut left = Machine::new(&loaded.ast, &loaded.resolved, &loaded.check);
    let mut right = Machine::new(&loaded.ast, &loaded.resolved, &loaded.check).with_max_calls(600);

    let divided = left.call(
        "pure.share",
        vec![Value::Int(1), Value::Int(0)],
        Span::DUMMY,
    );
    let too_deep = right.call(
        "pure.ladder",
        vec![Value::Int(100_000), Value::Int(1)],
        Span::DUMMY,
    );
    assert_eq!(
        divided.is_err(),
        too_deep.is_err(),
        "the fixture stopped raising on both sides, so there is no `(Err, Err)` to compare"
    );
    assert!(divided.is_err(), "neither side raised at all");
    assert!(
        compare_answers(&left, &right, "pure.share", &divided, &too_deep).is_some(),
        "a division by zero and a recursion limit were scored as agreement because both were \
         raises"
    );

    // And the control: the same diagnostic on both sides is agreement, so the assertion above is
    // discrimination rather than a comparison that reports everything.
    let mut same = Machine::new(&loaded.ast, &loaded.resolved, &loaded.check);
    let again = same.call(
        "pure.share",
        vec![Value::Int(1), Value::Int(0)],
        Span::DUMMY,
    );
    assert!(
        compare_answers(&left, &same, "pure.share", &divided, &again).is_none(),
        "one diagnostic compared against itself was reported as a divergence"
    );
}

/// The corpus's own subject, at cargo-test speed: a wrong backend under a whole interpreted search.
#[test]
fn a_whole_kernel_search_notices_a_wrong_leaf() {
    let loaded = kernel();
    let mut honest = kernel_harness(loaded);
    let control = mutated(&mut honest, Mutation::None, None);
    let mut wrong = kernel_harness(loaded);
    let mutant = mutated(&mut wrong, Mutation::OffByOne, Some("mcts.plan"));

    let mut noticed = 0;
    for args in searches() {
        if let Some(d) = disagreement(&mut honest, "mcts.plan", &args) {
            panic!("a search disagreed with an honest backend: {d}");
        }
        if disagreement(&mut wrong, "mcts.plan", &args).is_some() {
            noticed += 1;
        }
    }

    assert_eq!(control.fired(), 0, "the honest harness was not honest");
    assert!(mutant.fired() > 0, "the mutation never fired");
    assert!(
        noticed > 0,
        "24 whole-kernel searches ran with `mcts.plan` off by one and every one of them \
         answered the same move"
    );
    // And the withdrawn form, asserted rather than described: a leaf mutation is now invisible to a
    // search, because the search does not cross the boundary at the leaf any more.
    let mut leaf = kernel_harness(loaded);
    let leaf_mutant = mutated(&mut leaf, Mutation::OffByOne, Some("mcts.nth_move"));
    for args in searches() {
        if let Some(d) = disagreement(&mut leaf, "mcts.plan", &args) {
            panic!(
                "a search noticed a mutated `mcts.nth_move` after all — good news, and the \
                 correction on this test is now wrong: {d}"
            );
        }
    }
    assert_eq!(
        leaf_mutant.fired(),
        0,
        "`mcts.nth_move` was corrupted {} times from a search, so the search still crosses the \
         boundary at the leaves and this correction is wrong",
        leaf_mutant.fired()
    );
    // The direct case still reaches it, which is what keeps the leaf covered.
    caught(
        &mut leaf,
        "mcts.nth_move",
        &[Value::Int(7 + 5 * 16 + 3 * 256), Value::Int(1)],
    );
}

/// The other half, and the reason the corpus needs its per-function cases: a search is a weak
/// oracle.
#[test]
fn a_whole_kernel_search_is_a_weak_oracle_and_the_per_function_cases_are_not() {
    let loaded = kernel();

    let mut ucb = kernel_harness(loaded);
    let ucb_mutant = mutated(&mut ucb, Mutation::OffByOne, Some("mcts.ucb"));
    let mut isqrt = kernel_harness(loaded);
    let isqrt_mutant = mutated(&mut isqrt, Mutation::OffByOne, Some("mcts.isqrt"));

    for args in searches() {
        if let Some(d) = disagreement(&mut ucb, "mcts.plan", &args) {
            panic!(
                "a search noticed a uniformly wrong `mcts.ucb` after all — good news, and the \
                 note on this test is now wrong: {d}"
            );
        }
        if let Some(d) = disagreement(&mut isqrt, "mcts.plan", &args) {
            panic!(
                "a search noticed a wrong `mcts.isqrt` after all — good news, and the note on \
                 this test is now wrong: {d}"
            );
        }
    }
    assert_eq!(
        ucb_mutant.fired(),
        0,
        "`mcts.ucb` was corrupted {} times from a search. The withdrawn assertion here was \
         `fired() > 100`, when `ucb` was an entry point; if it fires again the fragment has \
         narrowed and the correction on this test is wrong",
        ucb_mutant.fired()
    );
    assert_eq!(
        isqrt_mutant.offered_target(),
        0,
        "`mcts.isqrt` was offered from a search after all, which is the claim this pins"
    );

    // And the leg that does catch them: one direct call each.
    caught(
        &mut ucb,
        "mcts.ucb",
        &[Value::Int(3), Value::Int(7), Value::Int(9)],
    );
    caught(&mut isqrt, "mcts.isqrt", &[Value::Int(17)]);
}

/// The 24 whole-kernel searches, in the shape `verify` generates them: a packed three-heap
/// position, a seed and an iteration count.
fn searches() -> Vec<Vec<Value>> {
    (0..24i64)
        .map(|case| {
            let state = (1 + (case * 3) % 8) + ((case * 5) % 8) * 16 + ((case * 7) % 8) * 256;
            vec![
                Value::Int(state),
                Value::Int(1 + case * 7919),
                Value::Int(4 + case % 37),
            ]
        })
        .collect()
}
