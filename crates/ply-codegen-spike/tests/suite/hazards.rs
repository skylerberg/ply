//! Every hazard the R5 audit named, as a case that runs.

use ply_codegen_spike::entry::{admissible, enterable, refusals_over, scalar_signature};
use ply_codegen_spike::jit::Opts;
use ply_codegen_spike::measure::Harness;
use ply_codegen_spike::program::Loaded;
use ply_eval::{Machine, Value, compare_answers};
use ply_span::Span;
use std::path::PathBuf;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

fn hazards() -> &'static Loaded {
    Box::leak(Box::new(
        Loaded::project(&fixtures().join("hazards")).expect("the hazard fixtures load"),
    ))
}

/// A harness whose compiled unit is `pure` and nothing else, so every other module in the fixture
/// is interpreted code with native bodies underneath it.
fn pure_harness(loaded: &'static Loaded) -> Harness {
    let all = loaded.functions_in("pure");
    let accepted = admissible(loaded, &all).expect("`pure` classifies");
    assert_eq!(
        accepted.len(),
        all.len(),
        "the fragment refused part of `pure`, so the fixtures below would be measuring the \
         refusal rather than the hazard: {:?}",
        refusals_over(loaded, &all).expect("it classifies")
    );
    let names: Vec<&str> = accepted.iter().map(|s| s.as_str()).collect();
    Harness::over(loaded, &names, Opts::default(), None).expect("`pure` compiles")
}

/// The same, over a named module, without insisting the fragment accept it.
fn harness_over(loaded: &'static Loaded, modules: &[&str]) -> Harness {
    let mut all = Vec::new();
    for m in modules {
        all.extend(loaded.functions_in(m));
    }
    let accepted = admissible(loaded, &all).expect("the modules classify");
    let names: Vec<&str> = accepted.iter().map(|s| s.as_str()).collect();
    Harness::over(loaded, &names, Opts::default(), None).expect("the accepted set compiles")
}

fn refusal(loaded: &'static Loaded, module: &str, name: &str) -> Option<String> {
    let all = loaded.functions_in(module);
    refusals_over(loaded, &all)
        .expect("the module classifies")
        .into_iter()
        .find(|(f, _)| f == name)
        .map(|(_, why)| why)
}

/// Both engines over one call, compared on everything `differential` compares: the value, and on a
/// raise the code, the message, every label with its span, the notes, the observed footprint and
/// the cell arena.
fn agree(harness: &mut Harness, name: &str, args: &[Value]) -> Option<String> {
    let expected = harness.interpret_outcome(name, args);
    let hybrid = harness.hybrid_outcome(name, args);
    compare_answers(&harness.machine, &harness.hybrid, name, &expected, &hybrid)
        .map(|d| format!("with a backend attached, {d}"))
}

fn entries(harness: &Harness) -> u64 {
    harness.hybrid_counts().0
}

// -- 1. `Ctx` is one flat frame; a nested entry would alias the outer one -----

/// The guard declines and the interpreter answers, and the entry that follows is unaffected.
#[test]
fn an_entry_that_arrives_while_another_is_running_declines_and_the_machine_answers() {
    let loaded = hazards();
    let mut harness = pure_harness(loaded);
    harness.bodies.reset_counts();
    let bodies = harness.bodies.clone();
    let args = vec![Value::Int(5)];

    let expected = harness.interpret_outcome("pure.step", &args);
    let before = entries(&harness);
    let inside =
        bodies.while_entered(|| harness.hybrid.call("pure.step", args.clone(), Span::DUMMY));
    assert_eq!(
        entries(&harness),
        before,
        "an entry was taken while the context was already borrowed"
    );
    assert_eq!(
        bodies.declines().reentered,
        1,
        "the offer was declined for some reason other than reentrancy: {:?}",
        bodies.declines()
    );
    assert!(
        compare_answers(
            &harness.machine,
            &harness.hybrid,
            "pure.step",
            &expected,
            &inside
        )
        .is_none(),
        "the interpreter did not answer for itself while the backend was busy"
    );

    // And the provider is not left broken by having declined.
    let after = harness.run_hybrid("pure.step", &args).expect("it runs");
    assert!(
        matches!(after, Value::Int(16)),
        "`pure.step(5)` answered {} after a reentrant offer",
        after.render()
    );
    assert_eq!(
        entries(&harness),
        before + 1,
        "the entry after a reentrant decline was not taken"
    );
}

// -- 2. `failed` is sticky; `take_failure` clears the diagnostic, not the flag --

/// A raise inside compiled code must not make the *next* entry answer its own first argument, and
/// must not panic when there is no first argument to answer.
#[test]
fn a_failed_entry_does_not_poison_the_one_after_it() {
    let loaded = hazards();
    let mut harness = pure_harness(loaded);
    harness.bodies.reset_counts();

    for (name, args) in [
        ("pure.mix", vec![Value::Int(i64::MAX), Value::Int(1)]),
        ("pure.share", vec![Value::Int(1), Value::Int(0)]),
    ] {
        assert!(
            harness.compiled_call(name, &args).is_err(),
            "`{name}` was supposed to fail inside the fragment and did not"
        );
        // A body with arguments: the wrong answer would be argument 0.
        let good = harness
            .compiled_call("pure.mix", &[Value::Int(5), Value::Int(7)])
            .expect("the fragment answers after a failure");
        assert!(
            matches!(good, Value::Int(274)),
            "after `{name}` failed, `pure.mix(5, 7)` answered {} — handle 0 is argument 0",
            good.render()
        );
        // And a body with none: `slots` is empty, so the failure block's handle 0 has nothing to
        // read.
        assert!(
            harness.compiled_call(name, &args).is_err(),
            "`{name}` stopped failing"
        );
        let nullary = harness
            .compiled_call("pure.seeded", &[])
            .expect("a nullary body answers after a failure");
        assert!(
            matches!(nullary, Value::Int(80)),
            "after `{name}` failed, `pure.seeded()` answered {}",
            nullary.render()
        );
    }

    // The same sequence through the machine, which is where it would matter.
    let raised = agree(
        &mut harness,
        "pure.mix",
        &[Value::Int(i64::MAX), Value::Int(1)],
    );
    assert_eq!(raised, None, "{raised:?}");
    let before = entries(&harness);
    let after = harness
        .run_hybrid("pure.mix", &[Value::Int(5), Value::Int(7)])
        .expect("it runs");
    assert!(
        matches!(after, Value::Int(274)),
        "through the machine, `pure.mix(5, 7)` answered {} after a raise",
        after.render()
    );
    assert_eq!(
        entries(&harness),
        before + 1,
        "the call after a raise was not entered, so this proves nothing about the flag"
    );
}

// -- 3. Every crossing used to go through `Machine::call`, which resets ------

/// A native body running underneath a live handler stack, a cell and a resume.
#[test]
fn a_native_body_runs_under_a_live_handler_stack() {
    let loaded = hazards();
    let mut harness = pure_harness(loaded);
    harness.bodies.reset_counts();
    for n in [0i64, 1, 7, 41, 1000] {
        let args = vec![Value::Int(n)];
        let before = entries(&harness);
        if let Some(d) = agree(&mut harness, "effects.handled", &args) {
            panic!("`effects.handled({n})`: {d}");
        }
        assert!(
            entries(&harness) > before,
            "`effects.handled({n})` entered no compiled code, so nothing ran under the handler"
        );
    }
    assert_eq!(
        harness.hybrid.compiled_refusals(),
        0,
        "the boundary refused an answer, which is a backend bug"
    );
    // Pinned, so that a fixture whose handler stopped running would fail here rather than agree
    // with itself: 7 is the handled `base`, 22 is the `pure::step(7)` the `note` clause writes into
    // the cell, and both are in the answer.
    let answered = harness
        .run_hybrid("effects.handled", &[Value::Int(7)])
        .expect("it runs");
    assert!(
        matches!(answered, Value::Int(10790)),
        "`effects.handled(7)` answered {} — the handler, the cell write, or the native body \
         under them did not run",
        answered.render()
    );
}

/// A compiled unit that leaves out a callee refuses the caller by name rather than trampolining
/// into a second `Machine`.
#[test]
fn a_call_leaving_the_compiled_unit_refuses_its_caller() {
    let loaded = hazards();
    let refusals = refusals_over(loaded, &["deep.countdown".to_string()]).expect("it classifies");
    let why = refusals
        .iter()
        .find(|(f, _)| f == "deep.countdown")
        .map(|(_, why)| why.clone())
        .unwrap_or_default();
    assert!(
        why.contains("pure.mix"),
        "`deep.countdown` was refused for `{why}`; a unit without `pure.mix` in it must name the \
         call it cannot make"
    );
    // And with the callee present it compiles, so the refusal above is about the missing callee
    // rather than about the body.
    let both = admissible(
        loaded,
        &["deep.countdown".to_string(), "pure.mix".to_string()],
    )
    .expect("it classifies");
    assert!(
        both.contains(&"deep.countdown".to_string()),
        "`deep.countdown` is refused even with its callee in the unit: {both:?}"
    );
}

// -- 4. Builtins from compiled code would run against a private arena --------

/// A definition that opens its own region is refused by the fragment, by name.
#[test]
fn a_definition_that_opens_its_own_region_is_refused_by_the_fragment() {
    let loaded = hazards();
    assert!(
        scalar_signature(loaded, "cells.counted"),
        "`cells.counted` stopped being `Int` -> `Int`, so the signature filter would refuse it \
         and this test would be checking the wrong thing"
    );
    let why = refusal(loaded, "cells", "cells.counted")
        .expect("`cells.counted` opens a region and must be refused by the fragment");
    assert!(
        !why.is_empty(),
        "`cells.counted` was refused and the reason was empty"
    );

    // And with the whole fixture compiled, the program still agrees and the private arena is never
    // touched.
    let mut harness = harness_over(loaded, &["pure", "cells"]);
    harness.bodies.reset_counts();
    assert!(
        harness.compiled().entry("cells.counted").is_none(),
        "`cells.counted` was compiled after all"
    );
    for n in [0i64, 3, 99, 1_000_000] {
        let args = vec![Value::Int(n)];
        let before = entries(&harness);
        if let Some(d) = agree(&mut harness, "cells.counted", &args) {
            panic!("`cells.counted({n})`: {d}");
        }
        assert!(
            entries(&harness) > before,
            "`cells.counted({n})` entered no compiled code, so no native body ever ran inside a \
             live region"
        );
    }
    assert_eq!(
        harness.bodies.declines().touched_cells,
        0,
        "a builtin allocated in the fragment's private arena"
    );
}

/// The audit's own shape for this hazard is not a program.
#[test]
fn a_cell_cannot_be_a_parameter_of_a_function_that_reads_it() {
    let err = Loaded::project(&fixtures().join("cell_parameter"))
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(
        err.contains("region"),
        "`fn bump(c) = cell_set(c, cell_get(c) + 1)` loaded, or was refused for something other \
         than its region: {err:?}"
    );
}

// -- 5. No `Float` path, and it is wider than ADR 0018 §0 states -------------

/// A `Float` or `Decimal` in the signature is refused twice and offered never.
#[test]
fn float_and_decimal_signatures_are_never_offered() {
    let loaded = hazards();
    let mut harness = harness_over(loaded, &["pure", "numerics"]);
    harness.bodies.reset_counts();
    let cases: Vec<(&str, Vec<Value>, Value)> = vec![
        (
            "numerics.fadd",
            vec![Value::Float(1.5), Value::Float(2.25)],
            Value::Float(3.75),
        ),
        (
            "numerics.fless",
            vec![Value::Float(1.5), Value::Float(2.25)],
            Value::Bool(true),
        ),
        (
            "numerics.dadd",
            vec![
                Value::Decimal(ply_eval::Decimal::new(15, 1)),
                Value::Decimal(ply_eval::Decimal::new(225, 2)),
            ],
            Value::Decimal(ply_eval::Decimal::new(375, 2)),
        ),
        (
            "numerics.dless",
            vec![
                Value::Decimal(ply_eval::Decimal::new(15, 1)),
                Value::Decimal(ply_eval::Decimal::new(225, 2)),
            ],
            Value::Bool(true),
        ),
    ];
    for (name, args, want) in cases {
        assert!(
            enterable(loaded, &[name.to_string()]).is_empty(),
            "`{name}` was registered as enterable, so the fragment's missing `Float`/`Decimal` \
             path is one dynamic check away from a raise in a working program"
        );
        let before = (entries(&harness), harness.bodies.declines());
        if let Some(d) = agree(&mut harness, name, &args) {
            panic!("`{name}`: {d}");
        }
        let after = (entries(&harness), harness.bodies.declines());
        assert_eq!(
            (before.0, before.1.failed),
            (after.0, after.1.failed),
            "`{name}` was carried into a compiled body"
        );
        let answered = harness.run_hybrid(name, &args).expect("it runs");
        assert!(
            ply_eval::values_equal(&answered, &want, Span::DUMMY).unwrap_or(false),
            "`{name}` answered {} with a backend attached",
            answered.render()
        );
    }
}

/// The half no signature can refuse: a `Float`, `Decimal` or `String` **inside** an `Int` -> `Int`
/// body.
#[test]
fn a_float_or_decimal_literal_inside_an_int_body_is_never_a_wrong_answer() {
    let loaded = hazards();
    let mut harness = harness_over(loaded, &["pure", "numerics"]);
    harness.bodies.reset_counts();
    for name in [
        "numerics.float_inside",
        "numerics.decimal_inside",
        "numerics.float_arith_inside",
    ] {
        assert!(
            scalar_signature(loaded, name),
            "`{name}` stopped being `Int` -> `Int`, so nothing here is being tested"
        );
        assert!(
            harness.compiled().entry(name).is_none(),
            "`{name}` still compiles, so the `Float` literal inside it is still reaching \
             `rt_unbox_int` rather than being refused"
        );
        let why = refusal(loaded, "numerics", name)
            .unwrap_or_else(|| panic!("`{name}` was not refused, and not for a stated reason"));
        assert!(
            why.contains("literal, which the fragment has no path for"),
            "`{name}` was refused for some reason other than its non-`Int` literal: {why}"
        );
        // And it is still never a wrong answer through the boundary, which is the property the
        // refusal has to preserve rather than replace.
        for n in [0i64, 1, 7, -3, 1_000_000] {
            let args = vec![Value::Int(n)];
            if let Some(d) = agree(&mut harness, name, &args) {
                panic!("`{name}({n})`: {d}");
            }
        }
    }
    // And the most direct form of the same thing: a `Float` handed straight to a compiled body,
    // outside any machine.
    let direct = harness.compiled_call("numerics.fadd", &[Value::Float(1.5), Value::Float(2.25)]);
    let message = match direct {
        Ok(v) => panic!(
            "the fragment answered {} for `1.5 + 2.25` on `Float`s; it has no `Float` path, so \
             answering at all would be worse than raising",
            v.render()
        ),
        Err(e) => e.to_string(),
    };
    assert!(
        message.contains("an `Int` operation on a Float"),
        "the fragment failed on `Float` arithmetic somewhere other than the unbox: {message}"
    );
    assert_eq!(
        harness.hybrid.compiled_refusals(),
        0,
        "the backend answered a value this boundary refuses"
    );
}

/// The audit's third case for this hazard is not a program.
#[test]
fn ordering_on_a_string_is_refused_before_any_backend_sees_it() {
    let err = Loaded::project(&fixtures().join("string_ordering"))
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(
        err.contains("`<` is not defined on `String`"),
        "`a < b` on `String` loaded, or was refused for something else: {err:?}"
    );
}

// -- 6. Higher-order builtins accepted at compile time -----------------------

/// A callback reaching a builtin that calls user code is refused at compile time, including when
/// nothing in the signature says so.
#[test]
fn a_higher_order_builtin_is_refused_by_name() {
    let loaded = hazards();
    for name in ["callbacks.each", "callbacks.total", "callbacks.tripled"] {
        let why = refusal(loaded, "callbacks", name)
            .unwrap_or_else(|| panic!("`{name}` was compiled, and it calls user code"));
        assert!(
            !why.is_empty(),
            "`{name}` was refused and the reason was empty"
        );
    }
    assert!(
        scalar_signature(loaded, "callbacks.tripled"),
        "`callbacks.tripled` stopped being `Int` -> `Int`; the point of it is that its signature \
         says nothing about the `map`/`fold` under it"
    );

    let mut harness = harness_over(loaded, &["pure", "callbacks"]);
    harness.bodies.reset_counts();
    assert!(
        harness.compiled().entry("callbacks.tripled").is_none(),
        "`callbacks.tripled` was compiled after all"
    );
    for n in [0i64, 5, 998] {
        let args = vec![Value::Int(n)];
        if let Some(d) = agree(&mut harness, "callbacks.tripled", &args) {
            panic!("`callbacks.tripled({n})`: {d}");
        }
    }
}

// -- 7. Native recursion with no bound --------------------------------------

/// An interpreted recursion that drops into compiled code at every depth is bounded by the machine,
/// and answers the same diagnostic either way.
#[test]
fn an_interpreted_recursion_entering_compiled_code_at_every_depth_is_bounded() {
    let loaded = hazards();
    let harness = pure_harness(loaded);
    harness.bodies.reset_counts();
    let mut machine =
        Machine::new(&loaded.ast, &loaded.resolved, &loaded.check).with_max_calls(400);
    let mut hybrid = Machine::new(&loaded.ast, &loaded.resolved, &loaded.check).with_max_calls(400);
    hybrid.set_compiled(harness.bodies.clone());

    let args = vec![Value::Int(5_000_000), Value::Int(1)];
    let expected = machine.call("deep.countdown", args.clone(), Span::DUMMY);
    let actual = hybrid.call("deep.countdown", args, Span::DUMMY);
    let message = expected
        .as_ref()
        .err()
        .map(|d| d.message.clone())
        .unwrap_or_default();
    assert!(
        message.contains("recursion limit of 400 nested calls exceeded"),
        "the machine answered `{message}` rather than its own bound"
    );
    if let Some(d) = compare_answers(&machine, &hybrid, "deep.countdown", &expected, &actual) {
        panic!("the two sides did not raise the same diagnostic at the bound: {d}");
    }
    let (entered, _) = hybrid.compiled_counts();
    assert!(
        entered > 100,
        "only {entered} entries were taken on the way down, so the crossing was not exercised \
         at depth"
    );

    // And the same shape at the shipped bound, which is the number the guarantee is written in.
    let mut machine = Machine::new(&loaded.ast, &loaded.resolved, &loaded.check);
    let mut hybrid = Machine::new(&loaded.ast, &loaded.resolved, &loaded.check);
    hybrid.set_compiled(harness.bodies.clone());
    let args = vec![Value::Int(5_000_000), Value::Int(1)];
    let expected = machine.call("deep.countdown", args.clone(), Span::DUMMY);
    let actual = hybrid.call("deep.countdown", args, Span::DUMMY);
    assert!(
        expected.as_ref().err().is_some_and(|d| d
            .message
            .contains("recursion limit of 10000 nested calls exceeded")),
        "the machine's own bound moved"
    );
    if let Some(d) = compare_answers(&machine, &hybrid, "deep.countdown", &expected, &actual) {
        panic!("the two sides did not raise the same diagnostic at the shipped bound: {d}");
    }
}

/// A *compiled* recursion that would outrun the budget declines and the machine raises its own
/// bound — the case that used to be `SIGABRT`.
#[test]
fn a_compiled_recursion_that_outruns_its_budget_is_the_machines_diagnostic() {
    let loaded = hazards();
    let harness = pure_harness(loaded);
    harness.bodies.reset_counts();
    let mut machine =
        Machine::new(&loaded.ast, &loaded.resolved, &loaded.check).with_max_calls(600);
    let mut hybrid = Machine::new(&loaded.ast, &loaded.resolved, &loaded.check).with_max_calls(600);
    hybrid.set_compiled(harness.bodies.clone());

    let args = vec![Value::Int(5_000_000), Value::Int(1)];
    let expected = machine.call("pure.ladder", args.clone(), Span::DUMMY);
    let actual = hybrid.call("pure.ladder", args, Span::DUMMY);
    assert!(
        expected.as_ref().err().is_some_and(|d| d
            .message
            .contains("recursion limit of 600 nested calls exceeded")),
        "the machine answered {expected:?} rather than its bound"
    );
    if let Some(d) = compare_answers(&machine, &hybrid, "pure.ladder", &expected, &actual) {
        panic!("a runaway compiled recursion did not answer the machine's own diagnostic: {d}");
    }
    assert!(
        harness.bodies.declines().out_of_fuel > 0,
        "no entry ran out of fuel, so the native half of the bound was never reached: {:?}",
        harness.bodies.declines()
    );

    // A recursion that fits is still entered and still answers.
    let mut hybrid = Machine::new(&loaded.ast, &loaded.resolved, &loaded.check);
    hybrid.set_compiled(harness.bodies.clone());
    let before = hybrid.compiled_counts().0;
    let fits = hybrid
        .call(
            "pure.ladder",
            vec![Value::Int(50), Value::Int(1)],
            Span::DUMMY,
        )
        .expect("a bounded ladder answers");
    let mut machine = Machine::new(&loaded.ast, &loaded.resolved, &loaded.check);
    let want = machine
        .call(
            "pure.ladder",
            vec![Value::Int(50), Value::Int(1)],
            Span::DUMMY,
        )
        .expect("and so does the machine");
    assert!(
        ply_eval::values_equal(&fits, &want, Span::DUMMY).unwrap_or(false),
        "a ladder that fits its budget answered {} against {}",
        fits.render(),
        want.render()
    );
    assert!(
        hybrid.compiled_counts().0 > before,
        "the ladder that fits was never entered"
    );
}

// -- 8. `Ctx.slots` is append-only for the life of an entry ------------------

/// The value arena does not grow with executed work.
#[test]
fn the_entry_arena_does_not_grow_with_executed_work() {
    let loaded = hazards();
    let mut harness = pure_harness(loaded);
    harness.bodies.reset_counts();
    for n in 0..100_000i64 {
        harness
            .run_hybrid("pure.mix", &[Value::Int(n), Value::Int(n % 7)])
            .expect("it runs");
    }
    let (len, capacity) = harness.bodies.slots();
    assert!(
        len < 64 && capacity <= 4096,
        "after 100,000 entries the value arena is {len} long with capacity {capacity}, so it \
         grows with work rather than with live data"
    );

    // A recursion the provider refuses ten thousand times over.
    let mut hybrid = Machine::new(&loaded.ast, &loaded.resolved, &loaded.check);
    hybrid.set_compiled(harness.bodies.clone());
    let _ = hybrid.call(
        "pure.ladder",
        vec![Value::Int(5_000_000), Value::Int(1)],
        Span::DUMMY,
    );
    harness
        .run_hybrid("pure.mix", &[Value::Int(1), Value::Int(2)])
        .expect("it runs");
    let (_, capacity) = harness.bodies.slots();
    assert!(
        capacity <= 4096,
        "a runaway entry left the value arena at capacity {capacity} for the life of the provider"
    );
}

/// The other half of hazard 8, which the test above cannot reach.
#[test]
fn one_large_entry_gives_the_arena_back_to_the_entry_after_it() {
    let loaded = hazards();
    let mut harness = pure_harness(loaded);
    harness.bodies.reset_counts();

    let before = harness.bodies.entered();
    harness
        .run_hybrid("pure.ladder", &[Value::Int(9_000), Value::Int(1)])
        .expect("a bounded ladder answers");
    let used = harness.bodies.arena_after_entry();
    let entries = harness.bodies.entered() - before;
    let fuel_declines = harness.bodies.declines().out_of_fuel;

    assert_eq!(
        (entries, fuel_declines),
        (1, 0),
        "the bounded ladder was meant to be one entry that runs; it was {entries} entries with          {fuel_declines} fuel declines, so this test is measuring the decline path and not the          arena"
    );
    assert!(
        used > 4096,
        "the bounded ladder used {used} slots, at or below the {} the arena is kept at anyway, so          the assertion below would hold without the arena ever having grown",
        4096
    );

    harness
        .run_hybrid("pure.mix", &[Value::Int(1), Value::Int(2)])
        .expect("it runs");

    let (_, capacity) = harness.bodies.slots();
    assert!(
        capacity <= 4096,
        "one entry used {used} slots and the entry after it still sees capacity {capacity}, so a          single large call pins the arena for the calls that follow it"
    );
}

/// A `Secret` is refused in both directions and never enters a constant pool.
#[test]
fn a_secret_never_reaches_the_fragment() {
    let loaded = hazards();
    let why = refusal(loaded, "callbacks", "callbacks.keyed")
        .expect("`secret_of_string` must be refused at compile time");
    assert!(
        !why.is_empty(),
        "`callbacks.keyed` was refused with no reason"
    );

    let mut harness = harness_over(loaded, &["pure", "callbacks"]);
    harness.bodies.reset_counts();
    assert_eq!(
        harness.compiled().tables().retains_a_handle(),
        None,
        "the constant pool holds a value that must not outlive the call that made it"
    );
    let secret = Value::secret(Value::Int(7));
    let before = (entries(&harness), harness.bodies.declines());
    if let Some(d) = agree(&mut harness, "pure.step", std::slice::from_ref(&secret)) {
        panic!("a `Secret` argument: {d}");
    }
    let after = (entries(&harness), harness.bodies.declines());
    assert_eq!(
        (before.0, before.1.failed),
        (after.0, after.1.failed),
        "a `Secret` was carried into a compiled body"
    );
}

// -- 9. Every compiled diagnostic is `RUNTIME_ERROR` at `Span::DUMMY` --------

/// A raise inside compiled code arrives as the machine's own diagnostic.
#[test]
fn a_compiled_failure_arrives_as_the_machines_own_diagnostic() {
    let loaded = hazards();
    let mut harness = pure_harness(loaded);
    harness.bodies.reset_counts();
    let cases: Vec<(&str, Vec<Value>)> = vec![
        ("pure.mix", vec![Value::Int(i64::MAX), Value::Int(1)]),
        ("pure.mix", vec![Value::Int(1), Value::Int(i64::MIN)]),
        ("pure.share", vec![Value::Int(7), Value::Int(0)]),
        ("pure.step", vec![Value::Int(i64::MAX)]),
    ];
    for (name, args) in &cases {
        let before = harness.bodies.declines().failed;
        if let Some(d) = agree(&mut harness, name, args) {
            panic!("`{name}`: {d}");
        }
        assert!(
            harness.bodies.declines().failed > before,
            "`{name}` did not fail inside the fragment, so this case never reached the boundary"
        );
        let d = harness
            .hybrid_outcome(name, args)
            .expect_err("it raises")
            .clone();
        assert_ne!(
            d.code, "E0500",
            "`{name}` raised the fragment's own `RUNTIME_ERROR` rather than the machine's"
        );
        assert!(
            !d.message.contains("in compiled code"),
            "`{name}` raised the fragment's message: {}",
            d.message
        );
        assert!(
            d.labels.iter().any(|l| l.span != Span::DUMMY),
            "`{name}` raised with no real span, which is what the fragment's diagnostic looks like"
        );
    }
}

// -- 10. The gating agreement result covered neither failures nor non-Ints --

/// The `simulate` half, which the corpus cannot reach: the hook is off inside a region, so every
/// `Access` a partial-order search reads is the interpreter's.
#[test]
fn the_hook_is_off_inside_a_simulate_region() {
    let loaded = hazards();
    let mut harness = pure_harness(loaded);
    harness.bodies.reset_counts();
    for n in [0i64, 2, 13, 500] {
        let args = vec![Value::Int(n)];
        let before = entries(&harness);
        if let Some(d) = agree(&mut harness, "raced.raced", &args) {
            panic!("`raced.raced({n})`: {d}");
        }
        assert_eq!(
            entries(&harness) - before,
            1,
            "`raced.raced({n})` took {} entries; exactly one call is outside the region, so any \
             other number means the hook fired inside one",
            entries(&harness) - before
        );
        // And the region ran: the answer is the cell after at least one task's read-modify-write,
        // which is strictly more than the value it opened at.
        let base = (n * 3 + 1) % 1000;
        let answered = harness.run_hybrid("raced.raced", &args).expect("it runs");
        assert!(
            matches!(answered, Value::Int(v) if v > base),
            "`raced.raced({n})` answered {} against an opening value of {base}, so no task in \
             the region ever wrote the cell",
            answered.render()
        );
    }
    assert_eq!(
        harness.bodies.declines().touched_cells,
        0,
        "a builtin allocated in the fragment's private arena"
    );
}

// -- 11. A bare nullary constructor is a test, not a binding ------------------

/// `match o { None -> d, Some(v) -> v }` answers through the arm the machine picks, not through the
/// first one.
#[test]
fn a_nullary_constructor_pattern_is_a_test_and_not_a_binding() {
    let loaded = hazards();
    let mut harness = pure_harness(loaded);
    harness.bodies.reset_counts();

    assert!(
        harness.compiled().entry("pure.or_else").is_some(),
        "`pure.or_else` was refused, so the pattern this is about never lowered"
    );
    assert!(
        scalar_signature(loaded, "pure.tagged"),
        "`pure.tagged` stopped being `Int` -> `Int`, so the boundary never offers it"
    );

    // 1 is in the map and 2 is not, so the two arms are both taken.
    for (n, expected) in [(1i64, 7i64), (2, 99), (3, 99), (-1, 99)] {
        let args = vec![Value::Int(n)];
        let before = entries(&harness);
        if let Some(d) = agree(&mut harness, "pure.tagged", &args) {
            panic!("`pure.tagged({n})`: {d}");
        }
        let answered = harness
            .run_hybrid("pure.tagged", &args)
            .expect("`pure.tagged` runs");
        assert_eq!(
            answered,
            Value::Int(expected),
            "`pure.tagged({n})` answered {} rather than {expected}, so the `None` arm matched a \
             `Some`",
            answered.render()
        );
        assert!(
            entries(&harness) > before,
            "`pure.tagged({n})` was never offered to compiled code, so agreement here is two \
             interpreters agreeing"
        );
    }

    // The direct form, outside any machine: the same call with no interpreter available to answer
    // it if the entry declines.
    let direct = harness
        .compiled_call(
            "pure.or_else",
            &[Value::ctor("Some", vec![Value::Int(7)]), Value::Int(99)],
        )
        .expect("`pure.or_else` runs in compiled code");
    assert_eq!(
        direct,
        Value::Int(7),
        "compiled `or_else(Some(7), 99)` answered {}, which is the `None` arm",
        direct.render()
    );
}
