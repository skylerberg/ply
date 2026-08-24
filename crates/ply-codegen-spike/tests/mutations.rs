//! Proof that the agreement checks bite: a backend that is wrong on purpose,
//! and the comparison catching it.
//!
//! R5 reported 2,396 generated cases over 29 functions with 0 disagreements.
//! A corpus that compares nothing reports the same number, and this project has
//! shipped a green result over unexplored space more than once. `hazards.rs`
//! answers "does the seam hold?"; this file answers the question underneath it:
//! **would we know if it did not?**
//!
//! Every test here follows the same three steps, and the middle one is the one
//! that is usually missing:
//!
//!   1. corrupt the backend in one specific way ([`ply_codegen_spike::wrong`]),
//!   2. assert the corruption actually *fired* — a mutation that never changed
//!      an answer proves nothing about the harness that did not catch it,
//!   3. assert `differential::compare_answers` — the same function the corpus in
//!      `src/bin/mcts.rs` and the hazard tests use — reports it.
//!
//! They pass in debug as well as release, which is not a formality: debug is
//! where `Machine::compiled_answer`'s `compiled_witness` assertion runs, and it
//! stays silent through every corruption below. A wrong backend produces a wrong
//! *answer*; it does not move machine state, because it is handed no route to
//! any.
//!
//! These tests are themselves checkable the same way: replacing every
//! `Mutation` below with [`Mutation::None`] fails nine of the eleven and leaves
//! exactly the control and the gate test green (run 2026-08-21). A test that
//! passes with the corruption removed was asserting nothing.
//!
//! Six of the seven corruptions the R5 audit named are caught here, plus one it
//! did not name — answering where the definition raises. The seventh —
//! accepting a call the machine must decline — cannot be demonstrated by a
//! backend at all, and [`a_backend_is_never_offered_an_effectful_definition`]
//! says why and pins the gate that makes it so.
//!
//! # The same corruptions at corpus scale
//!
//! These tests are seconds and one fixture. The agreement corpus in
//! `src/bin/mcts.rs` is 2,396 generated cases over 29 kernel functions, and it
//! was run against each corruption with `--mutate` on 2026-08-21. Every one of
//! them either went red or took the process down with it, and the first case
//! each run names is recorded here because a corpus nobody has seen fail is a
//! corpus with no measured sensitivity:
//!
//! | `--mutate` | disagreements | first case named |
//! | --- | --- | --- |
//! | `off-by-one` | 1,635 | `mcts.heap case 0: result value — left 0, right 1` |
//! | `inverted` | 174 | `mcts.terminal case 0: result value — left false, right true` |
//! | `stale` | 1,429 | `mcts.heap case 1: result value — left 15, right 0` |
//! | `wrong-type` | 1,911 | `mcts.heap case 0: result value — left 0, right false` |
//! | `off-by-one@work.zero` | 2 | `work.zero case 0: result value — left 0, right 1` — one corrupted entry, and the constant memo replays it |
//! | `unoffered` | 377 | `mcts.empty_node case 0: — left {action: -1, …}, right 0` |
//! | `answers=0@mcts.below` | 69 | `mcts.below case 24: verdict — left [E0502] remainder by zero, right passed` |
//! | `exceeds-budget=4` | 3 | `mcts.playouts case 23: diagnostic labels[0] — this call is too deeply nested, at two different spans` |
//! | `exceeds-budget` (no bound) | none | no case ran: `fatal runtime error: stack overflow`, exit 134 |
//!
//! The last two rows are the ones to read twice. Overrunning the budget by a
//! factor of four is caught, and caught on the axis this project added
//! deliberately: both engines raise `recursion limit of 10000 nested calls
//! exceeded`, and what separates them is the *label span* — a comparison that
//! scored `(Err, Err)` as agreement would have passed. Ignoring the budget
//! altogether is not caught at all, because it is not a wrong answer: the native
//! recursion overflows the stack and the process is gone before anything is
//! compared. The bounded, crash-free form of that same mistake is
//! [`a_backend_that_runs_past_its_budget_is_caught`].
//!
//! Every one of those was reported by one leg of the corpus and not the other,
//! and it is worth knowing which. `verify` compares the hybrid machine against
//! the machine with no backend, and separately compares the tree-walker against
//! the machine with no backend. Only the first can see a backend at all; the
//! tree-walker leg polices the *machine*, and the backend is covered
//! transitively. So every line in the table above reads "with a backend
//! attached", and a corpus that dropped the hybrid leg would keep its
//! independent oracle and lose all of this.
//!
//! # What a whole-kernel search can and cannot see
//!
//! R5's headline pairs 2,396 per-function cases with 24 whole-kernel searches,
//! and the second half is much weaker than it reads. Corrupting one compiled
//! function at a time and running exactly those 24 searches (2026-08-21):
//!
//! | corrupted, off by one | answers changed | searches that noticed |
//! | --- | --- | --- |
//! | `mcts.nth_move` | 372 | 20 of 24 |
//! | `mcts.apply_move` | 372 | 12 |
//! | `mcts.next_seed`, `mcts.rollout` | 372 | 11 |
//! | `mcts.turn` | 847 | 11 |
//! | `mcts.move_count` | 746 | 10 |
//! | `mcts.ucb` | 1,268 | **0** |
//! | the other twelve | **0** — never offered | 0 |
//!
//! Two separate blind spots, both pinned by
//! [`a_whole_kernel_search_is_a_weak_oracle_and_the_per_function_cases_are_not`].
//! A uniform off-by-one in `mcts.ucb` feeds an argmax and changes no move: 1,268
//! wrong scores, 24 identical answers. And twelve of the nineteen compiled
//! functions are offered to the backend **zero** times during those searches —
//! `below`, `terminal`, `heap`, `objects`, `winner`, `pack`, `playouts`,
//! `nim_sum`, `bit_xor`, `ilog2`, `isqrt`, `isqrt_step` — because each is
//! reached only from inside another compiled body, and the hook sees nothing
//! under an entered root. Only seven are offered at all: `next_seed`,
//! `rollout`, `ucb`, `turn`, `move_count`, `nth_move`, `apply_move`.
//!
//! # Which cases carry the detection
//!
//! `--mutate` prints the subjects whose cases reported the corruption, and over
//! the whole corpus that is a narrower list than the disagreement counts suggest
//! (2026-08-21, one function corrupted per run):
//!
//! | corrupted | subjects whose cases reported it |
//! | --- | --- |
//! | `ucb`, `bit_xor`, `ilog2`, `isqrt`, `isqrt_step`, `nim_sum`, `objects`, `winner`, `playouts`, `work.zero` | **only its own** |
//! | `heap` | its own, and `nth_move` |
//! | `pack`, `rollout` | its own, and one caller |
//! | `below`, `terminal`, `apply_move`, `turn`, `next_seed`, `move_count`, `nth_move` | its own, and 3–7 callers including `mcts.plan` |
//!
//! Every one of the twenty functions the corpus enters is caught — but for
//! **half of them the only witness is the function's own generated cases**, and
//! `mcts.ucb` is the one to keep in mind. It answers **2,156** wrong scores over
//! a corpus of 2,396 cases and 24 searches, and the only cases that report it
//! are the 84 generated directly against `mcts.ucb`: every caller above it, the
//! whole-kernel search included, answers exactly what it answered before. A
//! corpus of callers alone — which is what "24 whole-kernel searches" sounds
//! like — would not have noticed.
//!
//! One more thing that only shows up by doing this: `off-by-one@mcts.terminal`
//! never fires, because `terminal` answers a `Bool`. The run says so and refuses
//! to report anything (`the mutation never fired, so this run says nothing about
//! the corpus`) rather than printing a green agreement. `inverted@mcts.terminal`
//! is the one that fires, and it is caught by six subjects.
//!
//! `ply test --engine both` catches **none** of these, on any corpus: no
//! shipping command can install a backend (`Machine::set_compiled` has no CLI
//! caller), so the whole compiled path is invisible to it. What plays that role
//! is `crates/ply-eval/tests/differential_corpus.rs`, whose answering backend
//! over `examples/` and `tests/fixtures/` was corrupted the same four ways and
//! failed every time — `off-by-one` on
//! `a_backend_that_answers_correctly_agrees_over_every_corpus_on_disk` names
//! "a line total is price times quantity: expected 1000, found 1001".

use ply_codegen_spike::entry::admissible;
use ply_codegen_spike::jit::Opts;
use ply_codegen_spike::measure::Harness;
use ply_codegen_spike::program::Loaded;
use ply_codegen_spike::wrong::{Mutant, Mutation};
use ply_eval::{Machine, Value, compare_answers};
use ply_span::Span;
use std::path::PathBuf;
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

/// The `pure` module compiled and offered, every other module interpreted — the
/// same shape `hazards.rs` measures, so a mutation here is a mutation of the
/// arrangement the audit already trusts.
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

/// Both engines over one call, compared on everything `differential` compares:
/// the value, and on a raise the code, the message, every label with its span,
/// the notes, the observed footprint and the cell arena.
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

/// The MCTS kernel of `benches/kernel`, with the fragment's largest compiled
/// subset offered — the arrangement `src/bin/mcts.rs` measures.
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

/// The control every other test is read against: the wrapper itself changes
/// nothing.
///
/// Without this, a red result below could be the wrapper rather than the
/// mutation.
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

/// 1. Off by one on a compiled arithmetic result.
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

    // And underneath an interpreted recursion, where it is one addend of many —
    // the shape a real off-by-one takes.
    caught(
        &mut harness,
        "deep.countdown",
        &[Value::Int(12), Value::Int(1)],
    );
    assert!(mutant.fired() > 0, "the mutation never fired");
}

/// 2. An inverted compiled comparison.
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

/// 3. A stale answer: this call gets the previous call's result.
///
/// The one mutation that is invisible to a single call — every answer it gives
/// was a correct answer to *some* call, so what catches it is a corpus that
/// varies its arguments.
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

/// 4. The right information in the wrong kind: `Bool` where an `Int` belongs.
///
/// The seam checks a *kind* and carries `Bool` and `Int` both, so it does not
/// refuse this — `compiled_refusals` stays zero and the wrong-kinded value
/// reaches the program. That is the boundary behaving as documented, and it is
/// why the check has to be downstream of it.
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

    // And where the caller does arithmetic on it: the interpreter raises a type
    // error the machine with no backend never sees.
    caught(
        &mut harness,
        "deep.countdown",
        &[Value::Int(3), Value::Int(1)],
    );
    assert!(mutant.fired() > 0, "the mutation never fired");
}

/// 5. Accepting a call the machine must decline.
///
/// **This one cannot be done from a backend**, and that is the finding rather
/// than a gap. `Machine::compiled_answer` refuses any definition whose published
/// effect row is non-empty, so `effects.measured` — which performs `tally.base`
/// and `tally.note` — is never offered to anything. The mutant below stands
/// ready to answer it with the value it really does return under the handler in
/// `effects.handled`, and is never asked.
///
/// The corruption that *would* accept it is a change to the machine, not to a
/// backend. Removing that one line in `Machine::compiled_answer` and re-running
/// this fixture is what prices the gate, and it was run (2026-08-21): the
/// mutant is offered `effects.measured` on the first call, answers 6,688, and
/// `compare_answers` reports
///
/// ```text
/// effects.handled: observed footprint — left {effects.tally.read[log],
///                  effects.tally.write[log]}, right {}
/// ```
///
/// — the footprint axis, because the value was chosen to match. Three named unit
/// tests in `ply_eval::compiled` go red in the same build:
/// `a_definition_whose_published_row_is_not_empty_is_never_offered`,
/// `a_definition_that_opens_its_own_simulate_region_is_never_offered` and
/// `a_machine_with_no_check_output_offers_nothing`.
///
/// Nothing else notices. In that same build `hazards.rs` and `mcts_kernel.rs`
/// stay green — 25 tests, including the one that runs a native body under a live
/// handler stack — because an honest `SpikeBodies` has no body for an effectful
/// name and declines it whether or not the machine asks. So does
/// `ply-eval`'s own `differential_corpus.rs`, whose backend declines for the
/// same reason. So does `ply test --engine both` over all 186 tests in
/// `examples/`, because no shipping command installs a backend at all. The
/// kernel corpus in `src/bin/mcts.rs` *cannot* notice: `benches/kernel` declares
/// no effect, so every definition in it publishes an empty row and the gate has
/// nothing to refuse.
///
/// That experiment cannot be a standing test, because with the gate in place
/// there is nothing to compare — so what stands here is the offer count, which
/// is the fact the gate makes true.
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
    // 6,688 is chosen so that `effects.handled(1)` answers exactly what it
    // answers with no backend at all — `mix(6688, 0)` is `mix(mix(7, 1),
    // step(1))` — which leaves the perform and the cell write as the *only*
    // difference. A backend that accepted this call would be answering the right
    // number and still be wrong.
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

/// 6. An answer for a definition the backend was never given a body for.
///
/// The machine offers every pure, scalar-argument call it makes, so most of what
/// a backend sees are names it has nothing to say about. Declining is the whole
/// of its contract there.
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

/// 6b. Answering where the definition would raise.
///
/// Not on the audit's list and it belongs there: the fragment's own failures
/// (an overflow, a division by zero, a `match` with no arm) are what
/// `Declines::failed` counts, and a backend that answered instead of declining
/// would turn a diagnostic into a number. `(Err, Err)` scored as agreement until
/// R5 for exactly this shape of comparison, which is why the check is on the
/// verdict rather than on "both sides were unhappy".
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

/// 7. Running past the call budget instead of declining.
///
/// `budget` is the machine's remaining nested calls, and a body that would
/// outrun it must answer `None` so the machine can raise the bound both engines
/// raise. A backend that runs anyway answers where the machine raises, which is
/// the difference this catches.
///
/// The bound is 600 here so the native recursion that ignores it is 1,000 frames
/// rather than unbounded. Unlimited fuel over a runaway recursion is not a wrong
/// answer at all — it is a native stack overflow, and no comparison sees it
/// because the process is gone.
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

/// The corpus's own subject, at cargo-test speed: a wrong backend under a whole
/// interpreted search.
///
/// The table at the top of this file records manual `--mutate` runs, and a
/// record is not a check. This is the standing version over the same kernel:
/// corrupt one leaf the search drops into, and the move it answers changes.
///
/// `mcts.nth_move` is the target because it is the most sensitive one there is
/// — 20 of these 24 searches notice it — and even that is not all of them, which
/// is the point of counting rather than asserting a single case.
#[test]
fn a_whole_kernel_search_notices_a_wrong_leaf() {
    let loaded = kernel();
    let mut honest = kernel_harness(loaded);
    let control = mutated(&mut honest, Mutation::None, None);
    let mut wrong = kernel_harness(loaded);
    let mutant = mutated(&mut wrong, Mutation::OffByOne, Some("mcts.nth_move"));

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
        "24 whole-kernel searches ran with `mcts.nth_move` off by one and every one of them \
         answered the same move"
    );
}

/// The other half, and the reason the corpus needs its per-function cases: a
/// search is a weak oracle.
///
/// Two blind spots, both measured (2026-08-21) rather than reasoned about:
///
///   * **`mcts.ucb` off by one is invisible to a search.** It answers 1,268 wrong
///     scores over these 24 searches and every one of them still answers the
///     same move, because UCB feeds an argmax and adding one to every score
///     leaves the ranking alone.
///   * **`mcts.isqrt` is never offered by a search at all.** It is reached only
///     from inside `mcts.ucb`, which is compiled — so once the machine enters
///     `ucb`, everything under it is a native call the hook never sees. Twelve of
///     the nineteen compiled functions are in this position: `below`, `terminal`,
///     `heap`, `objects`, `winner`, `pack`, `playouts`, `nim_sum`, `bit_xor`,
///     `ilog2`, `isqrt`, `isqrt_step`.
///
/// Both are caught by a direct case on the function, which is what the corpus's
/// per-function leg is for, and this test asserts that too — a limitation is
/// only worth pinning next to the thing that covers it.
///
/// If a search ever *does* notice either of these, this test fails. That is good
/// news and the note above is what needs updating.
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
    assert!(
        ucb_mutant.fired() > 100,
        "`mcts.ucb` was corrupted {} times, so the blind spot above is not what was measured",
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

/// The 24 whole-kernel searches, in the shape `verify` generates them: a packed
/// three-heap position, a seed and an iteration count.
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
