//! ADR 0018 §1's kernel, and the claims the measurement rests on.
//!
//! The report `src/bin/mcts.rs` prints is a measurement and moves with the
//! machine. What must not move is what it is a measurement *of*: that the
//! kernel loads, that the fragment accepts the arithmetic and refuses the tree
//! by name, and that the compiled code answers what both shipped evaluators
//! answer. A ratio between two evaluators that disagree prices nothing.

use ply_codegen_spike::entry::{admissible, enterable, refusals_over, scalar_signature};
use ply_codegen_spike::jit::Opts;
use ply_codegen_spike::measure::Harness;
use ply_codegen_spike::program::Loaded;
use ply_eval::{Interp, Value, compare_answers, values_equal};
use ply_span::Span;
use std::path::PathBuf;

fn kernel_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("benches")
        .join("kernel")
}

fn loaded() -> &'static Loaded {
    Box::leak(Box::new(
        Loaded::project(&kernel_dir()).expect("the kernel loads"),
    ))
}

/// Why the fragment refused `name`, taken over the whole `mcts` module at once.
///
/// > **Corrected in R5.** This used to be `Jit::compile(loaded, &[name])` — one
/// > function on its own — and that is no longer the same question. A call to a
/// > function outside the compiled unit now refuses the caller, so a lone
/// > `mcts.rollout` is refused for calling `mcts.terminal` rather than compiled.
/// > Offering the whole module keeps the answer a statement about *constructs*,
/// > which is what the ADR 0018 §0 roadmap ranks.
fn refusal(loaded: &'static Loaded, name: &str) -> Option<String> {
    let all = loaded.functions_in("mcts");
    refusals_over(loaded, &all)
        .expect("the module classifies")
        .into_iter()
        .find(|(f, _)| f == name)
        .map(|(_, why)| why)
}

/// The same, for a program with one module of another name.
fn refusal_in(loaded: &'static Loaded, module: &str, name: &str) -> Option<String> {
    let all = loaded.functions_in(module);
    refusals_over(loaded, &all)
        .expect("the module classifies")
        .into_iter()
        .find(|(f, _)| f == name)
        .map(|(_, why)| why)
}

fn kernel_harness(loaded: &'static Loaded) -> (Harness, Vec<String>) {
    let all = loaded.functions_in("mcts");
    let accepted = admissible(loaded, &all).expect("the module classifies");
    let names: Vec<&str> = accepted.iter().map(|s| s.as_str()).collect();
    let harness =
        Harness::over(loaded, &names, Opts::default(), Some("work.zero")).expect("it compiles");
    (harness, accepted)
}

#[test]
fn the_kernel_loads_beside_the_shipped_standard_library() {
    let loaded = loaded();
    let names = loaded.functions_in("mcts");
    assert!(
        names.contains(&"mcts.plan_753".to_string()),
        "the kernel's entry point is missing; `mcts` holds {names:?}"
    );
    assert!(
        loaded.definition("std.http.read_line").is_some(),
        "the stdlib did not load beside the project, so a kernel could not call into it"
    );
}

/// > **Corrected in R5.** This list used to end `"mcts.search", "mcts.plan",
/// > "mcts.plan_753"` and assert that the fragment compiled all three.
/// >
/// > It did, and every one of them left the fragment on its first iteration:
/// > `search` calls `iterate`, which reads a field, so the compiled `search` was
/// > a native loop around a trampoline back into a second `Machine`. ADR 0018 §0
/// > measured what that is worth end to end — 0.998× — and why: "the interpreter
/// > cannot call compiled code", so the twenty arithmetic functions under them
/// > were compiled and never entered.
/// >
/// > There is no trampoline now. A compiled set is closed under calls, so those
/// > three are refused, and the twenty arithmetic functions are the ones that
/// > run — entered from the interpreter at the leaves instead of driving it from
/// > the top. `the_interpreter_enters_compiled_code_at_the_leaves` is the test
/// > that says so.
#[test]
fn the_fragment_compiles_the_position_arithmetic_and_the_playout() {
    let loaded = loaded();
    let all = loaded.functions_in("mcts");
    let accepted = admissible(loaded, &all).expect("the module classifies");
    for name in [
        "mcts.heap",
        "mcts.turn",
        "mcts.apply_move",
        "mcts.nth_move",
        "mcts.next_seed",
        "mcts.ilog2",
        "mcts.isqrt",
        "mcts.isqrt_step",
        "mcts.ucb",
        "mcts.rollout",
        "mcts.playouts",
    ] {
        assert!(
            accepted.contains(&name.to_string()),
            "the fragment refused `{name}` ({:?}), which the measurement counts as compiled",
            refusal(loaded, name)
        );
        assert!(
            scalar_signature(loaded, name),
            "`{name}` is compiled and cannot be entered, so compiling it buys nothing"
        );
    }
    // > **Corrected (fragment widening, 2026-08-24).** This read:
    // >
    // > ```
    // > for name in ["mcts.search", "mcts.plan", "mcts.plan_753"] {
    // >     assert!(
    // >         !accepted.contains(&name.to_string()),
    // >         "`{name}` reaches the tree, so a compiled set holding it is not
    // >          closed under calls"
    // >     );
    // > }
    // > ```
    // >
    // > It was true because the tree half was outside the fragment, not because
    // > these three functions were special: a set holding them was not closed
    // > under calls *while* `node_at` and the rest were refused. Field access,
    // > list patterns, unary `-` and list literals now lower, so the closure
    // > includes them and the assertion inverts.
    for name in ["mcts.search", "mcts.plan", "mcts.plan_753"] {
        assert!(
            accepted.contains(&name.to_string()),
            "`{name}` is outside the fragment ({:?}), so the tree half did not close",
            refusal(loaded, name)
        );
    }
    // > **Corrected (fragment widening, 2026-08-24).** This asserted
    // > `enterable(loaded, &accepted).len() == accepted.len()`, "every accepted
    // > kernel function is `Int`/`Bool` throughout, so all of them are
    // > enterable". True while the fragment stopped at the tree: everything it
    // > accepted was arithmetic over `Int`.
    // >
    // > The tree half is accepted now and it is typed in `Tree` and `Node`, so
    // > accepted (34) and enterable (21) have come apart. That gap is not a
    // > defect — `entry::scalar_signature` registers `Int`/`Bool` throughout and
    // > a `Tree` argument has no boundary representation — but it is exactly the
    // > distinction ADR 0018 §0's census does not draw, so it is asserted as two
    // > numbers rather than one equality.
    let enterable_now = enterable(loaded, &accepted).len();
    assert_eq!(
        (accepted.len(), enterable_now),
        (34, 21),
        "the kernel compiles {} of 34 functions and the machine may enter {enterable_now} of \
         them; a change to either number is a change to what widening bought",
        accepted.len()
    );
    // The entry points that carry the win: scalar in and scalar out, with the
    // whole tree behind them.
    for name in ["mcts.plan", "mcts.plan_753"] {
        assert!(
            !enterable(loaded, &[name.to_string()]).is_empty(),
            "`{name}` is compiled but not enterable, so the tree half compiles and never runs"
        );
    }
}

/// Every function that reads the tree is now **inside** the fragment, and this
/// is the list ADR 0018 §0's ranked census was a census of.
///
/// > **Corrected (fragment widening, 2026-08-24).** This test was
/// > `the_fragment_refuses_every_function_that_touches_the_tree_and_names_why`,
/// > described as "the finding this whole milestone turns on, stated as an
/// > assertion: every function that reads the tree is outside the fragment, and
/// > the fragment says which construct took it out", and it asserted:
/// >
/// > ```
/// > ("mcts.node_at", "a field access"),
/// > ("mcts.put", "a field access"),
/// > ("mcts.select", "a field access"),
/// > ("mcts.expand", "a field access"),
/// > ("mcts.backprop", "a field access"),
/// > ("mcts.iterate", "a field access"),
/// > ("mcts.best_action", "a field access"),
/// > ("mcts.best_child", "a list pattern in a `match`"),
/// > ("mcts.most_visited", "a list pattern in a `match`"),
/// > ("mcts.fresh", "a list literal"),
/// > ("mcts.empty_node", "unary `-`"),
/// > ("mcts.root", "unary `-`"),
/// > ```
/// >
/// > Every row was accurate when written. The four constructs are lowered now,
/// > so every one of these compiles and the refusal set is empty. The list is
/// > kept, inverted, because it is the thing that has to stay true: a later
/// > change that puts any of them back outside the fragment should fail here
/// > naming the function, not silently shrink a census.
#[test]
fn every_function_that_touches_the_tree_is_inside_the_fragment() {
    let loaded = loaded();
    for name in [
        "mcts.node_at",
        "mcts.put",
        "mcts.select",
        "mcts.expand",
        "mcts.backprop",
        "mcts.iterate",
        "mcts.best_action",
        "mcts.best_child",
        "mcts.most_visited",
        "mcts.fresh",
        "mcts.empty_node",
        "mcts.root",
    ] {
        assert_eq!(
            refusal(loaded, name),
            None,
            "`{name}` is still outside the fragment"
        );
    }
    // The whole module, so that a construct nobody listed cannot hide.
    let all = loaded.functions_in("mcts");
    let refused = refusals_over(loaded, &all).expect("the module classifies");
    assert!(
        refused.is_empty(),
        "the kernel is not wholly inside the fragment: {refused:?}"
    );
}

/// > **Corrected in R5.** This used to enter `mcts.plan` as compiled code and
/// > compare its answer. `mcts.plan` is refused now — it reaches the tree — so
/// > the comparison runs the *interpreter* on both sides and attaches a backend
/// > to one of them. That is the shape R5 exists to produce, and it is a
/// > stronger check as well: `differential::compare_answers` compares the
/// > diagnostic field by field, the observed footprint and the cell arena, not
/// > only the value.
#[test]
fn a_search_the_interpreter_drives_answers_the_same_with_a_backend_attached() {
    let loaded = loaded();
    let (mut harness, _) = kernel_harness(loaded);

    // Positions drawn deterministically, so a failure here is reproducible from
    // the numbers in the message.
    let mut state = 0x2545F4914F6CDD1Du64;
    for case in 0..12 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let a = 1 + (state % 8) as i64;
        let b = ((state >> 8) % 8) as i64;
        let c = ((state >> 16) % 8) as i64;
        let seed = 1 + ((state >> 24) % 100_000) as i64;
        let iterations = 1 + ((state >> 40) % 24) as i64;
        let args = vec![
            Value::Int(a + b * 16 + c * 256),
            Value::Int(seed),
            Value::Int(iterations),
        ];

        let plain = harness.interpret_outcome("mcts.plan", &args);
        let hybrid = harness.hybrid_outcome("mcts.plan", &args);
        assert!(
            compare_answers(
                &harness.machine,
                &harness.hybrid,
                "mcts.plan",
                &plain,
                &hybrid
            )
            .is_none(),
            "case {case}: heaps ({a},{b},{c}) seed {seed} × {iterations} iterations — the \
             backend changed what the interpreter answered: {:?}",
            compare_answers(
                &harness.machine,
                &harness.hybrid,
                "mcts.plan",
                &plain,
                &hybrid
            )
            .map(|d| d.to_string())
        );

        let mut interp = Interp::new(&loaded.ast, &loaded.resolved, &loaded.check);
        let walked = interp.call("mcts.plan", args, Span::DUMMY);
        assert!(
            compare_answers(&interp, &harness.machine, "mcts.plan", &walked, &plain).is_none(),
            "case {case}: the tree-walker and the machine disagree, which is not about the \
             backend at all"
        );
    }
    let (entries, _) = harness.hybrid_counts();
    assert!(
        entries > 0,
        "the backend was attached and never entered, so this test compared the interpreter \
         with itself — which is exactly the null result R4 reported as 0.998x"
    );
}

/// The number ADR 0018 §0 says is the whole milestone.
///
/// > In the hybrid, compiled code is reached by exactly three functions:
/// > `mcts.iterate` (100 entries), `mcts.root` (1), `mcts.best_action` (1).
///
/// Those were *crossings out of* compiled code, taken because the search was
/// driven from the top.
///
/// > **Corrected (fragment widening, 2026-08-24).** This test was
/// > `the_interpreter_enters_compiled_code_at_the_leaves` and asserted 721
/// > entries spread over seven functions:
/// >
/// > ```
/// > (721, [("mcts.ucb", 375), ("mcts.turn", 105), ("mcts.move_count", 81),
/// >        ("mcts.apply_move", 40), ("mcts.next_seed", 40),
/// >        ("mcts.nth_move", 40), ("mcts.rollout", 40)])
/// > ```
/// >
/// > with the description "the interpreter drives, and drops into compiled code
/// > at the leaves — `ucb` on every child examined, `rollout` on every playout,
/// > the position arithmetic under both". That was exactly right for a fragment
/// > that stopped at the tree: `mcts.plan` was refused, so the interpreter had
/// > to drive the search and could only drop into the arithmetic underneath it.
/// >
/// > It is withdrawn because the leaves are no longer the boundary. `mcts.plan`
/// > is inside the fragment now, so the interpreter enters **once** and the
/// > whole search — selection, expansion, playout, backpropagation — runs
/// > natively. 721 crossings became 1. The old numbers are kept above because a
/// > future change that re-refuses any tree function would restore them, and a
/// > reader seeing 721 here again should know it means the fragment narrowed.
#[test]
fn the_interpreter_enters_compiled_code_once_for_the_whole_search() {
    let loaded = loaded();
    let (mut harness, _) = kernel_harness(loaded);
    harness.bodies.reset_counts();
    let args = vec![
        Value::Int(7 + 5 * 16 + 3 * 256),
        Value::Int(20260821),
        Value::Int(40),
    ];
    harness
        .run_hybrid("mcts.plan", &args)
        .expect("the search runs");

    let by_name = harness.bodies.entries_by_name();
    let (entries, declines) = harness.hybrid_counts();
    // Deterministic: one position, one seed, a fixed iteration count, and a
    // kernel with no clock and no randomness that is not the seed. Pinned
    // exactly rather than bounded, so a change to what the fragment accepts
    // shows up here as a number rather than as a still-passing inequality.
    assert_eq!(
        (entries, by_name.as_slice()),
        (1, [("mcts.plan".to_string(), 1)].as_slice()),
        "a 40-iteration search entered compiled code {entries} times. One is the whole \
         search running natively behind a single crossing; 721 spread over seven functions \
         is the fragment stopping at the tree again, and more than one entry to `mcts.plan` \
         is the interpreter re-driving a search it should have handed over once."
    );
    assert_eq!(
        harness.hybrid.compiled_refusals(),
        0,
        "the machine refused an answer at the boundary, which is a backend bug rather than a \
         fragment limit"
    );
    let d = harness.bodies.declines();
    assert_eq!(d.reentered, 0, "an entry began while another was running");
    assert_eq!(
        d.touched_cells, 0,
        "compiled code allocated in its private arena"
    );
    assert_eq!(
        d.arity, 0,
        "the machine offered a call with the wrong arity"
    );
    assert_eq!(
        d.failed + d.out_of_fuel,
        0,
        "a compiled body of this kernel failed and was silently re-evaluated"
    );
    assert_eq!(
        entries + declines,
        harness.bodies.entered() + d.total(),
        "the machine's counts and the provider's disagree about what was offered"
    );
    // Everything the machine declined, it declined because the name is not in
    // the compiled set — a caller the fragment refused, not a body that failed.
    assert_eq!(
        d.not_compiled, declines,
        "some calls were declined for a reason other than not being compiled: {d:?}"
    );
}

/// The playout batch is the part of the kernel the fragment covers end to end,
/// and it is the one the reported ratio is taken on — so it gets its own
/// agreement check rather than being covered only through `plan`.
#[test]
fn the_compiled_playout_batch_agrees_over_a_sweep_of_seeds() {
    let loaded = loaded();
    let names = [
        "mcts.playouts",
        "mcts.rollout",
        "mcts.terminal",
        "mcts.winner",
        "mcts.turn",
        "mcts.objects",
        "mcts.heap",
        "mcts.move_count",
        "mcts.nth_move",
        "mcts.apply_move",
        "mcts.next_seed",
        "mcts.below",
    ];
    let mut harness =
        Harness::over(loaded, &names, Opts::default(), Some("work.zero")).expect("it compiles");
    for seed in [1i64, 7, 4242, 20260821, 2_147_483_647] {
        for batch in [1i64, 5, 37] {
            let args = vec![
                Value::Int(7 + 5 * 16 + 3 * 256),
                Value::Int(seed),
                Value::Int(batch),
            ];
            let machine = harness.interpret("mcts.playouts", &args).expect("it runs");
            let compiled = harness
                .compiled_call("mcts.playouts", &args)
                .expect("it runs");
            assert!(
                values_equal(&machine, &compiled, Span::DUMMY).unwrap_or(false),
                "seed {seed} × {batch} playouts: the machine answered {} and the fragment \
                 answered {}",
                machine.render(),
                compiled.render()
            );
            // And through the hook, which is the path a program takes.
            let entered = harness.hybrid_counts().0;
            let hybrid = harness.run_hybrid("mcts.playouts", &args).expect("it runs");
            assert!(
                harness.hybrid_counts().0 > entered,
                "`mcts.playouts` is the fragment's own showcase and the interpreter did not \
                 enter it"
            );
            assert!(
                values_equal(&machine, &hybrid, Span::DUMMY).unwrap_or(false),
                "seed {seed} × {batch} playouts: the interpreter answered {} without a backend \
                 and {} with one",
                machine.render(),
                hybrid.render()
            );
        }
    }
}

/// > **Corrected in R5.** This test used to be
/// > `a_hybrid_run_records_which_function_each_crossing_went_to`: it compiled
/// > `mcts.plan_753` and asserted that a 12-iteration search crossed back into
/// > the machine 12 times, once per `mcts.iterate`. The crossing is gone with
/// > the trampoline, and so is the compiled `plan_753`.
/// >
/// > What replaced it is the claim those crossings were evidence *against*: the
/// > interpreter drives and enters compiled code, and it does so once per
/// > iteration of exactly the functions the search's inner loop reaches.
#[test]
fn a_hybrid_run_records_which_function_each_entry_went_to() {
    let loaded = loaded();
    let (mut harness, _) = kernel_harness(loaded);
    harness.bodies.reset_counts();
    harness
        .run_hybrid("mcts.plan_753", &[Value::Int(12)])
        .expect("it runs");
    let by_name = harness.bodies.entries_by_name();
    let count = |name: &str| {
        by_name
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, c)| *c)
            .unwrap_or(0)
    };
    // > **Corrected (fragment widening, 2026-08-24).** This asserted
    // > `(count("mcts.rollout"), count("mcts.next_seed")) == (12, 12)` — "a
    // > 12-iteration search should enter one playout and one reseed per
    // > iteration" — reasoning that "an entered `rollout` plays its whole game
    // > natively, so the sixty-odd `next_seed`, `nth_move` and `apply_move`
    // > calls inside it are native calls rather than entries. `next_seed` shows
    // > twelve because `iterate` reseeds once per iteration and those twelve are
    // > the only ones the interpreter makes."
    // >
    // > The reasoning was right and its conclusion has moved up one level. The
    // > same argument now applies to `iterate` itself: `plan_753` is inside the
    // > fragment, so the interpreter hands over the whole search in one entry
    // > and makes no `rollout` or `next_seed` call at all. Twelve became zero,
    // > and the total became one, for the same reason sixty became twelve.
    assert_eq!(
        (count("mcts.rollout"), count("mcts.next_seed")),
        (0, 0),
        "the interpreter entered a leaf of a search it should have handed over whole; the \
         entries were {by_name:?}"
    );
    assert_eq!(
        count("mcts.plan_753"),
        1,
        "the whole 12-iteration search should be one entry; the entries were {by_name:?}"
    );
    let total: u64 = by_name.iter().map(|(_, c)| *c).sum();
    assert_eq!(
        total,
        harness.bodies.entered(),
        "the per-function tally and the total disagree"
    );
    assert_eq!(
        total,
        harness.hybrid_counts().0,
        "the provider and the machine disagree about how many entries were taken"
    );
}

/// The guarantee `ply_eval::limit` exists to keep, kept in the hybrid too.
///
/// ADR 0019 §5 item 6 records the gap: the machine answers `recursion limit of
/// 10000 nested calls exceeded` and compiled code `SIGABRT`s, because the
/// fragment compiles a self-call to a native call with no bound.
/// `benches/adr0018-mcts.json`'s `recursion_bound_compiled` field holds the
/// crash. This runs it as a subprocess so the claim is observed rather than
/// asserted — an in-process assertion cannot see its own `SIGABRT`.
#[test]
fn a_runaway_recursion_is_the_machines_diagnostic_and_not_a_crash() {
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_mcts"));
    let dir = kernel_dir();
    let mut says = Vec::new();
    for which in ["machine", "compiled"] {
        let out = std::process::Command::new(&exe)
            .args(["--dir", &dir.display().to_string(), "--probe", which])
            .output()
            .expect("the probe runs");
        assert!(
            out.status.success(),
            "the `{which}` probe died with {} — {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
        says.push(String::from_utf8_lossy(&out.stdout).trim().to_string());
    }
    for (which, said) in ["machine", "hybrid"].iter().zip(&says) {
        assert!(
            said.contains("recursion limit of 10000 nested calls exceeded"),
            "the {which} side answered `{said}` rather than the bound both engines owe"
        );
    }
}

/// ADR 0018 §2 wants `Int`, `Bool` **and `Float`** unboxed. ADR 0016 §3.2's
/// fragment has no `Float` path at all — and it does not say so at compile
/// time. It compiles `a + b` as `Int` arithmetic whatever the operands are, and
/// the mistake surfaces as a runtime failure out of `rt_unbox_int`.
///
/// That matters for the ordering: a kernel written over `Float` is not
/// *refused* by the census, it is accepted and then raises, so a census that
/// counted it as compiled would be counting a function that cannot run.
#[test]
fn the_fragment_accepts_float_arithmetic_and_then_fails_on_it_at_run_time() {
    let dir = std::env::temp_dir().join(format!("ply-float-probe-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    std::fs::write(
        dir.join("floaty.ply"),
        "pub fn add(a: Float, b: Float) -> Float = a + b\n",
    )
    .expect("the probe source");
    let loaded: &'static Loaded = Box::leak(Box::new(
        Loaded::project(&dir).expect("the probe program loads"),
    ));

    assert_eq!(
        refusal_in(loaded, "floaty", "floaty.add"),
        None,
        "the fragment refused `Float` arithmetic by name, which would make this test obsolete \
         and the census stricter than it is"
    );

    let mut harness =
        Harness::over(loaded, &["floaty.add"], Opts::default(), None).expect("it compiles");
    let args = vec![Value::Float(1.5), Value::Float(2.25)];
    let machine = harness.interpret("floaty.add", &args).expect("it runs");
    assert!(
        matches!(machine, Value::Float(f) if f == 3.75),
        "the machine answered {}",
        machine.render()
    );

    let compiled = harness.compiled_call("floaty.add", &args);
    let message = match compiled {
        Ok(v) => panic!(
            "the compiled fragment answered {} for `1.5 + 2.25` on `Float`s; it has no `Float` \
             path, so answering at all would be worse than raising",
            v.render()
        ),
        Err(e) => e.to_string(),
    };
    assert!(
        message.contains("an `Int` operation on a Float"),
        "the fragment failed on `Float` arithmetic, but not where this test says it does: {message}"
    );

    // And R5's half of it: the machine is never offered the call at all, so a
    // program that would have started raising at a call site nobody opted into
    // simply runs in the interpreter. Two independent refusals — the signature
    // filter at registration, and the boundary's own `crossable` on the values —
    // and the test asserts the outcome rather than either mechanism.
    assert!(
        enterable(loaded, &["floaty.add".to_string()]).is_empty(),
        "`floaty.add` was registered as enterable, so the fragment's missing `Float` path is \
         one dynamic check away from being a raise in a working program"
    );
    let before = harness.hybrid_counts();
    let hybrid = harness.run_hybrid("floaty.add", &args).expect("it runs");
    assert_eq!(
        harness.hybrid_counts(),
        before,
        "the boundary offered a `Float` call to a backend that has no `Float` path"
    );
    assert!(
        matches!(hybrid, Value::Float(f) if f == 3.75),
        "with a backend attached the interpreter answered {}",
        hybrid.render()
    );

    std::fs::remove_dir_all(&dir).ok();
}
