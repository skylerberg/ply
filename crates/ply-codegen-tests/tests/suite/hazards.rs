//! Every hazard the compiled seam has to answer, against the shipping code generator.
//!
//! These were `crates/ply-codegen-spike`'s, and they tested *that* crate's seam: a third code
//! generator with a runtime of its own, written before this one existed. A hazard demonstrated
//! against a program nothing ships is a demonstration about the program, so porting them here is
//! not preserving coverage -- it is acquiring it, over the seam a `--backend` run actually crosses.
//!
//! The fixtures are the spike's, unchanged, and each says in its own header which hazard it exists
//! for and why the obvious smaller program does not reach it.

use ply_codegen::Cranelift;
use ply_eval::{Machine, Value, compare_answers};
use ply_span::Span;
use ply_syntax::ast::{ModuleName, Program};
use std::path::{Path, PathBuf};
use std::rc::Rc;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

pub struct Loaded {
    pub program: &'static Program,
    pub resolved: &'static ply_syntax::resolve::Resolved,
    pub check: &'static ply_core::CheckOutput,
}

/// Every `.ply` under `dir` as a module named after its stem, plus the shipped standard library.
///
/// `Err` carries the front end's own diagnostics, because two of the hazards below are checker
/// refusals: the program that would reach them does not typecheck, and *that* is the finding.
fn load(dir: &Path) -> Result<Loaded, Vec<ply_span::Diagnostic>> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "ply"))
        .collect();
    files.sort();

    let mut sources = ply_span::SourceMap::new();
    let mut inputs = Vec::new();
    for (module, text) in ply_std::sources() {
        let text: &'static str = text;
        let module = ModuleName::from_dotted(module);
        let id = sources.add(ply_std::pseudo_path(&module), text.to_string());
        inputs.push((id, module, text));
    }
    for path in &files {
        let stem = path.file_stem().and_then(|s| s.to_str()).expect("a stem");
        let text: &'static str = Box::leak(
            std::fs::read_to_string(path)
                .expect("the fixture is readable")
                .into_boxed_str(),
        );
        let id = sources.add(path.clone(), text.to_string());
        inputs.push((id, ModuleName::from_dotted(stem), text));
    }
    let mut ast = ply_syntax::parse_program(inputs).map_err(|d| d.to_vec())?;
    let expanded = ply_derive::expand_program(&mut ast);
    assert!(expanded.is_empty(), "{expanded:?}");
    let resolved = ply_syntax::resolve::resolve(&mut ast).map_err(|d| d.to_vec())?;
    let check = ply_core::check_program(&ast, &resolved).map_err(|d| d.to_vec())?;
    Ok(Loaded {
        program: Box::leak(Box::new(ast)),
        resolved: Box::leak(Box::new(resolved)),
        check: Box::leak(Box::new(check)),
    })
}

fn hazards() -> &'static Loaded {
    Box::leak(Box::new(
        load(&fixtures().join("hazards")).expect("the hazard fixtures load"),
    ))
}

/// Two machines over one program: the interpreter as shipped, and the same interpreter with
/// compiled bodies under it. Every hazard here is a claim about the difference.
struct Harness {
    unit: &'static Cranelift,
    bodies: Rc<ply_codegen::Bodies>,
    machine: Machine<'static>,
    hybrid: Machine<'static>,
}

fn harness(loaded: &'static Loaded) -> Harness {
    let unit: &'static Cranelift = Cranelift::over(loaded.program, loaded.resolved, loaded.check)
        .expect("this host has a cranelift backend");
    let bodies = unit.bodies().expect("the unit builds");
    let machine = Machine::new(loaded.program, loaded.resolved, loaded.check);
    let mut hybrid = Machine::new(loaded.program, loaded.resolved, loaded.check);
    hybrid.set_compiled(bodies.clone());
    Harness {
        unit,
        bodies,
        machine,
        hybrid,
    }
}

impl Harness {
    /// Both engines over one call, compared on the value and -- on a raise -- the code, the
    /// message, every label with its span, and the notes. `None` is agreement.
    fn agree(&mut self, name: &str, args: &[Value]) -> Option<String> {
        let expected = self.machine.call(name, args.to_vec(), Span::DUMMY);
        let actual = self.hybrid.call(name, args.to_vec(), Span::DUMMY);
        compare_answers(&self.machine, &self.hybrid, name, &expected, &actual)
            .map(|d| format!("with a backend attached, {d}"))
    }

    fn run(&mut self, name: &str, args: &[Value]) -> Value {
        self.hybrid
            .call(name, args.to_vec(), Span::DUMMY)
            .unwrap_or_else(|d| panic!("`{name}` raised: {}", d.message))
    }

    fn entered(&self) -> u64 {
        self.bodies.entered()
    }

    fn declines(&self) -> ply_codegen::Declines {
        self.bodies.declines()
    }
}

/// Why a definition was refused, if it was.
fn refusal(unit: &Cranelift, name: &str) -> Option<String> {
    unit.refusals()
        .iter()
        .find(|(f, _)| f == name)
        .map(|(_, why)| why.clone())
}

// -- the fragment refuses, before anything runs -------------------------------

/// A definition whose *published* row is empty and which opens a region anyway.
///
/// `pure_by_published_row` admits it and the machine offers it, which `memo.rs` says out loud, so
/// something has to hold the arena hazard. It used to be the fragment, by refusing the body. It is
/// now the seam, by measuring: an entry that does not give back the regions and the slots it took
/// is declined, and the machine answers the call itself.
///
/// The stronger claim is the one worth testing, so this asserts the answer rather than the
/// refusal: compiled and interpreted agree, the body actually ran compiled, and no entry was
/// declined for leaving the arena unbalanced. A body that opened a region and did not close it
/// would fail the third of those, and one that got the cell's counts wrong would fail the first.
#[test]
fn a_definition_that_opens_its_own_region_runs_compiled_and_gives_the_arena_back() {
    let mut h = harness(hazards());
    for n in [0i64, 1, 7, -3] {
        if let Some(difference) = h.agree("cells.counted", &[Value::Int(n)]) {
            panic!("`cells.counted({n})`: {difference}");
        }
    }
    assert!(
        h.entered() > 0,
        "`cells.counted` never ran compiled, so this proves nothing about the arena"
    );
    assert_eq!(
        h.declines().touched_cells,
        0,
        "an entry was declined for leaving the arena unbalanced, so the region it opened was not \
         closed on the way out"
    );
}

/// The smaller shape the hazard audit named, refused by the checker rather than by the fragment.
#[test]
fn a_cell_cannot_be_a_parameter_of_a_function_that_reads_it() {
    let diagnostics = load(&fixtures().join("cell_parameter"))
        .err()
        .expect("`held.ply` must not typecheck: a cell outside its binder is E0304");
    assert!(
        diagnostics.iter().any(|d| d.code == "E0304"),
        "the refusal was not E0304: {:?}",
        diagnostics.iter().map(|d| &d.code).collect::<Vec<_>>()
    );
}

/// Ordering on `String` never reaches a backend, because it never reaches a well-typed program.
#[test]
fn ordering_on_a_string_is_refused_before_any_backend_sees_it() {
    let diagnostics = load(&fixtures().join("string_ordering"))
        .err()
        .expect("`ordered.ply` must not typecheck: `<` on `String` is E0201");
    assert!(
        diagnostics.iter().any(|d| d.code == "E0201"),
        "the refusal was not E0201: {:?}",
        diagnostics.iter().map(|d| &d.code).collect::<Vec<_>>()
    );
}

/// A higher-order builtin under an `Int -> Int` signature answers what the interpreter answers.
///
/// **The spike refused these by name and this tier compiles them, which is the point of porting
/// rather than copying.** Nothing in `tripled`'s type says a callback is under it, so a filter on
/// the signature cannot see one; the spike's fragment had no way to run a callback and so had to
/// refuse by name, and a hazard was the only available answer. This tier runs them, so the
/// property worth asserting is the one that was always the real one: the answer is the machine's.
#[test]
fn a_higher_order_builtin_answers_what_the_interpreter_answers() {
    let mut h = harness(hazards());
    for n in [0, 1, 5] {
        if let Some(d) = h.agree("callbacks.tripled", &[Value::Int(n)]) {
            panic!("`callbacks.tripled({n})`: {d}");
        }
    }
    for n in [0, 3] {
        let xs = Value::list((0..n).map(Value::Int).collect());
        if let Some(d) = h.agree("callbacks.total", &[xs]) {
            panic!("`callbacks.total` over {n} elements: {d}");
        }
    }
}

/// `Value::Secret` is the value the secret invariant arms the argument pool against, and it must
/// not be reachable from compiled code at all.
#[test]
fn a_secret_never_reaches_the_fragment() {
    let h = harness(hazards());
    assert!(
        refusal(h.unit, "callbacks.keyed").is_some()
            || !h.unit.compiled().iter().any(|c| c == "callbacks.keyed"),
        "`callbacks.keyed` mints a `Secret` and was compiled anyway"
    );
}

/// A `Float` or `Decimal` in the signature never produces a wrong answer.
///
/// The spike refused these at registration. This tier registers them and the *value boundary* is
/// what declines -- two places the same call can be stopped, and `compiled.rs` §"What polices this
/// seam" is deliberate that neither depends on the other running first. So the assertion is on the
/// answer rather than on which of the two stopped it, and it fails if either stops declining.
#[test]
fn a_float_or_decimal_signature_is_never_a_wrong_answer() {
    let mut h = harness(hazards());
    for (name, args) in [
        ("numerics.fadd", vec![Value::Float(0.1), Value::Float(0.2)]),
        ("numerics.fless", vec![Value::Float(1.5), Value::Float(1.5)]),
    ] {
        if let Some(d) = h.agree(name, &args) {
            panic!("`{name}`: {d}");
        }
    }
}

// -- the fragment compiles it, and the answer has to be the machine's ---------

/// A non-scalar *literal inside* an `Int -> Int` body, which neither filter can see.
///
/// Nothing about `float_inside`'s type says it compares two `Float`s. If the fragment answered
/// rather than failing on one, the answer would be wrong and no boundary could tell -- so this
/// compares against the interpreter rather than asking what was refused.
#[test]
fn a_float_or_decimal_literal_inside_an_int_body_is_never_a_wrong_answer() {
    let mut h = harness(hazards());
    for name in [
        "numerics.float_inside",
        "numerics.decimal_inside",
        "numerics.float_arith_inside",
    ] {
        for n in [0, 1, 7] {
            if let Some(d) = h.agree(name, &[Value::Int(n)]) {
                panic!("`{name}({n})`: {d}");
            }
        }
    }
}

/// A nullary constructor written bare is `PatternKind::Var` in the AST -- the parser cannot tell
/// `None` from a binder -- so only the constructor table tells the two apart. A lowering that
/// binds `None` makes the first arm match everything.
#[test]
fn a_nullary_constructor_pattern_is_a_test_and_not_a_binding() {
    let mut h = harness(hazards());
    assert_eq!(h.run("pure.tagged", &[Value::Int(1)]), Value::Int(7));
    assert_eq!(h.run("pure.tagged", &[Value::Int(2)]), Value::Int(99));
    for n in [0, 1, 2, 3] {
        if let Some(d) = h.agree("pure.tagged", &[Value::Int(n)]) {
            panic!("`pure.tagged({n})`: {d}");
        }
    }
}

/// Both failures the fragment can reach arrive as the machine's own diagnostic rather than as a
/// bare `RUNTIME_ERROR` at `Span::DUMMY`: `mix` overflows, `share` divides by zero.
#[test]
fn a_compiled_failure_arrives_as_the_machines_own_diagnostic() {
    let mut h = harness(hazards());
    for (name, args) in [
        ("pure.mix", vec![Value::Int(i64::MAX), Value::Int(1)]),
        ("pure.share", vec![Value::Int(1), Value::Int(0)]),
    ] {
        if let Some(d) = h.agree(name, &args) {
            panic!("`{name}` failed differently under the backend: {d}");
        }
    }
}

/// A raise inside compiled code must not make the *next* entry answer wrongly, and must not panic
/// where there is no argument to answer with. `pure.seeded` is the nullary half.
#[test]
fn a_failed_entry_does_not_poison_the_one_after_it() {
    let mut h = harness(hazards());
    for (name, args) in [
        ("pure.mix", vec![Value::Int(i64::MAX), Value::Int(1)]),
        ("pure.share", vec![Value::Int(1), Value::Int(0)]),
    ] {
        let _ = h.hybrid.call(name, args, Span::DUMMY);
        assert_eq!(h.run("pure.seeded", &[]), Value::Int(80));
        assert_eq!(h.run("pure.step", &[Value::Int(5)]), Value::Int(16));
    }
}

/// A native body runs with the interpreter's handler stack, trail and region generations intact:
/// the machine performs, handles and resumes with compiled bodies running inside the handled block.
#[test]
fn a_native_body_runs_under_a_live_handler_stack() {
    let mut h = harness(hazards());
    for n in [0, 3, 11] {
        if let Some(d) = h.agree("effects.handled", &[Value::Int(n)]) {
            panic!("`effects.handled({n})`: {d}");
        }
    }
}

/// An interpreted recursion that drops into compiled code once per frame is bounded by the
/// machine, and by the machine's own diagnostic -- not by neither engine, which is what a crossing
/// through a second machine used to mean.
#[test]
fn an_interpreted_recursion_entering_compiled_code_at_every_depth_is_bounded() {
    let mut h = harness(hazards());
    let deep = Value::Int(1_000_000);
    let expected = h.machine.call(
        "deep.countdown",
        vec![deep.clone(), Value::Int(0)],
        Span::DUMMY,
    );
    let actual = h
        .hybrid
        .call("deep.countdown", vec![deep, Value::Int(0)], Span::DUMMY);
    assert!(
        expected.is_err(),
        "the fixture no longer outruns the bound, so it tests nothing"
    );
    assert!(
        actual.is_err(),
        "the recursion was bounded without a backend and unbounded with one"
    );
    let (a, b) = (expected.unwrap_err(), actual.unwrap_err());
    assert_eq!(
        (&a.code, &a.message),
        (&b.code, &b.message),
        "the bound reported differently with a backend attached"
    );
}

/// Compiled recursion, not in tail position so neither engine can turn it into a loop, outrunning
/// its own budget: the machine's diagnostic, not a crash and not a wrong answer.
#[test]
fn a_compiled_recursion_that_outruns_its_budget_is_the_machines_diagnostic() {
    let mut h = harness(hazards());
    if let Some(d) = h.agree("pure.ladder", &[Value::Int(1_000_000), Value::Int(0)]) {
        panic!("`pure.ladder` past its budget: {d}");
    }
}

// -- the guard nothing else can reach ----------------------------------------

/// `Ctx` is one flat frame, so an entry arriving while another runs would alias the outer one's
/// words. The guard declines, the interpreter answers for itself, and the provider is not left
/// broken by having declined.
#[test]
fn an_entry_that_arrives_while_another_is_running_declines_and_the_machine_answers() {
    let mut h = harness(hazards());
    // Warm first, so the count below is this call and not the compilation behind it.
    assert_eq!(h.run("pure.step", &[Value::Int(5)]), Value::Int(16));
    h.bodies.reset_counts();

    let bodies = Rc::clone(&h.bodies);
    let before = h.entered();
    let inside =
        bodies.while_entered(|| h.hybrid.call("pure.step", vec![Value::Int(5)], Span::DUMMY));
    assert_eq!(
        h.entered(),
        before,
        "an entry was taken while the context was already borrowed"
    );
    assert_eq!(
        h.bodies.declines().reentered,
        1,
        "the offer was declined for some reason other than reentrancy: {:?}",
        h.bodies.declines()
    );
    assert_eq!(
        inside.expect("the interpreter answers for itself"),
        Value::Int(16),
        "the interpreter did not answer while the backend was busy"
    );

    let after = h.run("pure.step", &[Value::Int(5)]);
    assert_eq!(after, Value::Int(16));
    assert_eq!(
        h.entered(),
        before + 1,
        "the entry after a reentrant decline was not taken"
    );
}

/// The hook is off inside a `simulate` region, so every `Access` a partial-order search reads is
/// the interpreter's: the answer, the schedule and the footprint are what the same machine
/// produces with no backend at all.
#[test]
fn the_hook_is_off_inside_a_simulate_region() {
    let mut h = harness(hazards());
    for n in [1, 4] {
        if let Some(d) = h.agree("raced.raced", &[Value::Int(n)]) {
            panic!("`raced.raced({n})`: {d}");
        }
    }
}
