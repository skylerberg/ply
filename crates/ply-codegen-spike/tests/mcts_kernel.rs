//! ADR 0018 §1's kernel, and the claims the measurement rests on.
//!
//! The report `src/bin/mcts.rs` prints is a measurement and moves with the
//! machine. What must not move is what it is a measurement *of*: that the
//! kernel loads, that the fragment accepts the arithmetic and refuses the tree
//! by name, and that the compiled code answers what both shipped evaluators
//! answer. A ratio between two evaluators that disagree prices nothing.

use ply_codegen_spike::jit::{Jit, Opts};
use ply_codegen_spike::measure::Harness;
use ply_codegen_spike::program::Loaded;
use ply_eval::{Interp, Value, values_equal};
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

fn refusal(loaded: &'static Loaded, name: &str) -> Option<String> {
    match Jit::compile(loaded, &[name]) {
        Ok(_) => None,
        Err(e) => {
            let text = e.to_string();
            Some(
                text.rsplit_once(": ")
                    .map(|(_, r)| r.to_string())
                    .unwrap_or(text),
            )
        }
    }
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

#[test]
fn the_fragment_compiles_the_position_arithmetic_and_the_playout() {
    let loaded = loaded();
    for name in [
        "mcts.heap",
        "mcts.turn",
        "mcts.apply_move",
        "mcts.nth_move",
        "mcts.next_seed",
        "mcts.ilog2",
        "mcts.isqrt",
        "mcts.ucb",
        "mcts.rollout",
        "mcts.playouts",
        "mcts.search",
        "mcts.plan",
        "mcts.plan_753",
    ] {
        assert_eq!(
            refusal(loaded, name),
            None,
            "the fragment refused `{name}`, which the measurement counts as compiled"
        );
    }
}

/// The finding this whole milestone turns on, stated as an assertion: every
/// function that reads the tree is outside the fragment, and the fragment says
/// which construct took it out.
#[test]
fn the_fragment_refuses_every_function_that_touches_the_tree_and_names_why() {
    let loaded = loaded();
    for (name, why) in [
        ("mcts.node_at", "a field access"),
        ("mcts.put", "a field access"),
        ("mcts.select", "a field access"),
        ("mcts.expand", "a field access"),
        ("mcts.backprop", "a field access"),
        ("mcts.iterate", "a field access"),
        ("mcts.best_action", "a field access"),
        ("mcts.best_child", "a list pattern in a `match`"),
        ("mcts.most_visited", "a list pattern in a `match`"),
        ("mcts.fresh", "a list literal"),
        ("mcts.empty_node", "unary `-`"),
        ("mcts.root", "unary `-`"),
    ] {
        assert_eq!(
            refusal(loaded, name).as_deref(),
            Some(why),
            "`{name}` is not refused for the reason the ranked roadmap says it is"
        );
    }
}

#[test]
fn the_compiled_kernel_answers_what_both_evaluators_answer() {
    let loaded = loaded();
    let accepted: Vec<String> = loaded
        .functions_in("mcts")
        .into_iter()
        .filter(|n| refusal(loaded, n).is_none())
        .collect();
    let names: Vec<&str> = accepted.iter().map(|s| s.as_str()).collect();
    let mut harness =
        Harness::over(loaded, &names, Opts::default(), Some("work.zero")).expect("it compiles");
    let entry = harness
        .compiled
        .entry("mcts.plan")
        .expect("`mcts.plan` is in the compiled set");

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

        let machine = harness.interpret("mcts.plan", &args).expect("it runs");
        let compiled = harness.compiled_call(entry, &args).expect("it runs");
        assert!(
            values_equal(&machine, &compiled, Span::DUMMY).unwrap_or(false),
            "case {case}: heaps ({a},{b},{c}) seed {seed} × {iterations} iterations — \
             the machine answered {} and the compiled fragment answered {}",
            machine.render(),
            compiled.render()
        );

        let mut interp = Interp::new(&loaded.ast, &loaded.resolved, &loaded.check);
        let walked = interp
            .call("mcts.plan", args, Span::DUMMY)
            .expect("it runs");
        assert!(
            values_equal(&machine, &walked, Span::DUMMY).unwrap_or(false),
            "case {case}: the tree-walker answered {} and the machine answered {}",
            walked.render(),
            machine.render()
        );
    }
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
    let entry = harness.compiled.entry("mcts.playouts").expect("compiled");
    for seed in [1i64, 7, 4242, 20260821, 2_147_483_647] {
        for batch in [1i64, 5, 37] {
            let args = vec![
                Value::Int(7 + 5 * 16 + 3 * 256),
                Value::Int(seed),
                Value::Int(batch),
            ];
            let machine = harness.interpret("mcts.playouts", &args).expect("it runs");
            let compiled = harness.compiled_call(entry, &args).expect("it runs");
            assert!(
                values_equal(&machine, &compiled, Span::DUMMY).unwrap_or(false),
                "seed {seed} × {batch} playouts: the machine answered {} and the fragment \
                 answered {}",
                machine.render(),
                compiled.render()
            );
        }
    }
}

/// A crossing back into the machine is counted per target, which is what lets
/// the report say *which* function the fragment had to leave to reach rather
/// than only how often it left.
#[test]
fn a_hybrid_run_records_which_function_each_crossing_went_to() {
    let loaded = loaded();
    let names = ["mcts.plan_753", "mcts.plan", "mcts.search", "mcts.pack"];
    let mut harness =
        Harness::over(loaded, &names, Opts::default(), Some("work.zero")).expect("it compiles");
    harness.ctx.reset_counts();
    let entry = harness.compiled.entry("mcts.plan_753").expect("compiled");
    harness
        .compiled_call(entry, &[Value::Int(12)])
        .expect("it runs");

    let by_target: Vec<(String, u64)> = harness
        .ctx
        .targets
        .iter()
        .cloned()
        .zip(harness.ctx.machine_calls_by_target.iter().copied())
        .filter(|(_, n)| *n > 0)
        .collect();
    let iterate = by_target
        .iter()
        .find(|(name, _)| name == "mcts.iterate")
        .map(|(_, n)| *n);
    assert_eq!(
        iterate,
        Some(12),
        "a 12-iteration search should leave the fragment once per iteration; the crossings \
         were {by_target:?}"
    );
    assert_eq!(
        harness.ctx.machine_calls,
        harness.ctx.machine_calls_by_target.iter().sum::<u64>(),
        "the per-target tally and the total disagree"
    );
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
        refusal(loaded, "floaty.add"),
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

    let entry = harness.compiled.entry("floaty.add").expect("compiled");
    let compiled = harness.compiled_call(entry, &args);
    let message = match compiled {
        Ok(v) => panic!(
            "the compiled fragment answered {} for `1.5 + 2.25` on `Float`s; it has no `Float` \
             path, so answering at all would be worse than raising",
            v.render()
        ),
        Err(e) => e.to_string(),
    };
    assert!(
        message.contains("arithmetic on a Float"),
        "the fragment failed on `Float` arithmetic, but not where this test says it does: {message}"
    );

    std::fs::remove_dir_all(&dir).ok();
}
