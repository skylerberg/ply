use ply_codegen::Unit;
use ply_eval::{Machine, Value};
use ply_span::{Span, Symbol};
use ply_syntax::ast::{ModuleName, Program};
use std::collections::HashMap;
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
    pub check: &'static ply_ty::CheckOutput,
    /// Each module's text by name: what the Ply emitter re-parses to produce.
    pub texts: HashMap<String, String>,
}

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
    let texts: HashMap<String, String> = inputs
        .iter()
        .map(|(_, module, text)| (module.to_string(), (*text).to_string()))
        .collect();
    let named: Vec<(String, String)> = inputs
        .iter()
        .map(|(_, module, text)| (module.to_string(), (*text).to_string()))
        .collect();
    let ids: Vec<_> = inputs.iter().map(|(id, _, _)| *id).collect();
    let mut ast = ply_syntax::parse_program(inputs).map_err(|d| d.to_vec())?;
    let expanded = ply_derive::expand_program(&mut ast);
    assert!(expanded.is_empty(), "{expanded:?}");
    let resolved = ply_syntax::resolve::resolve(&mut ast).map_err(|d| d.to_vec())?;
    ply_codegen::c::producer::ensure_default();
    let front = ply_codegen::c::producer::front(&named, &ids).expect("the port answers");
    if front.has_error() {
        return Err(front.diagnostics);
    }
    let check = front.check;
    Ok(Loaded {
        program: Box::leak(Box::new(ast)),
        resolved: Box::leak(Box::new(resolved)),
        check: Box::leak(Box::new(check)),
        texts,
    })
}

fn hazards() -> &'static Loaded {
    Box::leak(Box::new(
        load(&fixtures().join("hazards")).expect("the hazard fixtures load"),
    ))
}

struct Harness {
    unit: &'static Unit,
    bodies: Rc<ply_codegen::Bodies>,
    machine: Machine<'static>,
}

fn harness(loaded: &'static Loaded) -> Harness {
    let unit: &'static Unit =
        Unit::over_with_texts(loaded.program, loaded.resolved, loaded.texts.clone())
            .expect("this host has a C compiler");
    let bodies = unit.bodies().expect("the unit builds");
    let mut machine = Machine::new(loaded.program, loaded.resolved, loaded.check);
    machine.set_compiled(bodies.clone());
    Harness {
        unit,
        bodies,
        machine,
    }
}

impl Harness {
    fn run(&mut self, name: &str, args: &[Value]) -> Value {
        let unit = self.unit;
        self.machine
            .call(name, args.to_vec(), Span::DUMMY)
            .unwrap_or_else(|d| {
                panic!(
                    "`{name}` raised: {}; the port's refusal: {:?}",
                    d.message,
                    refusal(unit, name)
                )
            })
    }

    /// The machine's own diagnostic, which a compiled failure has to arrive as.
    fn raises(&mut self, name: &str, args: &[Value], message: &str) {
        let raised = self
            .machine
            .call(name, args.to_vec(), Span::DUMMY)
            .expect_err(name);
        assert_eq!(
            raised.code,
            ply_span::codes::RUNTIME_ERROR,
            "`{name}{args:?}` raised {raised}"
        );
        assert!(
            raised.message.contains(message),
            "`{name}{args:?}` raised {raised}, not {message:?}"
        );
    }

    fn entered(&self) -> u64 {
        self.bodies.entered()
    }

    fn declines(&self) -> ply_codegen::Declines {
        self.bodies.declines()
    }
}

fn refusal(unit: &Unit, name: &str) -> Option<String> {
    unit.refusals()
        .iter()
        .find(|(f, _)| f == name)
        .map(|(_, why)| why.clone())
}

#[test]
fn a_definition_that_opens_its_own_region_runs_compiled_and_gives_the_arena_back() {
    let mut h = harness(hazards());
    // `pure.step(n) + pure.mix(n, 2)`, with `%` truncating.
    for (n, want) in [(0i64, 35), (1, 69), (7, 273), (-3, -67)] {
        assert_eq!(
            h.run("cells.counted", &[Value::Int(n)]),
            Value::Int(want),
            "`cells.counted({n})`"
        );
    }
    assert!(h.entered() > 0, "`cells.counted` never ran compiled");
    assert_eq!(h.declines().touched_cells, 0, "{:?}", h.declines());
}

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

/// Nothing in `tripled`'s `Int -> Int` signature says a callback is under it.
#[test]
fn a_higher_order_builtin_answers_under_the_tier() {
    let mut h = harness(hazards());
    // `pure.step(n) + pure.step(n + 1)`.
    for (n, want) in [(0, 5), (1, 11), (5, 35)] {
        assert_eq!(
            h.run("callbacks.tripled", &[Value::Int(n)]),
            Value::Int(want),
            "`callbacks.tripled({n})`"
        );
    }
    for (n, want) in [(0, 0), (3, 3)] {
        let xs = Value::list((0..n).map(Value::Int).collect());
        assert_eq!(
            h.run("callbacks.total", &[xs]),
            Value::Int(want),
            "`callbacks.total` over {n} elements"
        );
    }
}

#[test]
fn a_secret_never_leaves_the_fragments_entry() {
    let h = harness(hazards());
    assert!(
        h.bodies.admits("callbacks.keyed"),
        "`callbacks.keyed` mints a `Secret`, which compiles: {:?}",
        refusal(h.unit, "callbacks.keyed")
    );
    let args = [Value::str("hunter2")];
    assert!(
        ply_eval::Compiled::enter(&*h.bodies, &Symbol::new("callbacks.keyed"), &args, 10_000)
            .is_none(),
        "a `Secret` crossed the seam as a value"
    );
    assert_eq!(h.declines().answer, 1, "{:?}", h.declines());
}

/// Nothing about `float_inside`'s `Int -> Int` type says it compares two `Float`s.
#[test]
fn a_float_or_decimal_is_never_a_wrong_answer() {
    let mut h = harness(hazards());
    assert_eq!(
        h.run("numerics.fadd", &[Value::Float(0.1), Value::Float(0.2)]),
        Value::Float(0.1 + 0.2)
    );
    assert_eq!(
        h.run("numerics.fless", &[Value::Float(1.5), Value::Float(1.5)]),
        Value::Bool(false)
    );
    for n in [1, 7] {
        for (name, want) in [
            ("numerics.float_inside", n),
            ("numerics.decimal_inside", n),
            ("numerics.float_arith_inside", 2 * n),
        ] {
            assert_eq!(
                h.run(name, &[Value::Int(n)]),
                Value::Int(want),
                "`{name}({n})`"
            );
        }
    }
}

/// A bare nullary constructor is `PatternKind::Var` in the AST, so binding it would make the first arm match everything.
#[test]
fn a_nullary_constructor_pattern_is_a_test_and_not_a_binding() {
    let mut h = harness(hazards());
    assert_eq!(h.run("pure.tagged", &[Value::Int(1)]), Value::Int(7));
    assert_eq!(h.run("pure.tagged", &[Value::Int(2)]), Value::Int(99));
}

#[test]
fn a_compiled_failure_arrives_as_the_machines_own_diagnostic() {
    let mut h = harness(hazards());
    h.raises(
        "pure.mix",
        &[Value::Int(i64::MAX), Value::Int(1)],
        "integer overflow in multiplication",
    );
    h.raises(
        "pure.share",
        &[Value::Int(1), Value::Int(0)],
        "division by zero",
    );
}

/// `pure.seeded` is the nullary case: a raise there has no argument to answer with.
#[test]
fn a_failed_entry_does_not_poison_the_one_after_it() {
    let mut h = harness(hazards());
    for (name, args) in [
        ("pure.mix", vec![Value::Int(i64::MAX), Value::Int(1)]),
        ("pure.share", vec![Value::Int(1), Value::Int(0)]),
    ] {
        let _ = h.machine.call(name, args, Span::DUMMY);
        assert_eq!(h.run("pure.seeded", &[]), Value::Int(80));
        assert_eq!(h.run("pure.step", &[Value::Int(5)]), Value::Int(16));
    }
}

#[test]
fn a_native_body_runs_under_a_live_handler_stack() {
    let mut h = harness(hazards());
    for (n, want) in [(0, 6744), (3, 8478), (11, 13102)] {
        assert_eq!(
            h.run("effects.handled", &[Value::Int(n)]),
            Value::Int(want),
            "`effects.handled({n})`"
        );
    }
}

/// Not in tail position, so it cannot become a loop.
#[test]
fn a_compiled_recursion_that_outruns_its_budget_is_the_machines_diagnostic() {
    let mut h = harness(hazards());
    h.raises(
        "pure.ladder",
        &[Value::Int(1_000_000), Value::Int(0)],
        "recursion limit of",
    );
}

/// `Ctx` is one flat frame, so a nested entry would alias the outer one's words.
#[test]
fn an_entry_that_arrives_while_another_is_running_is_declined_and_reported() {
    let mut h = harness(hazards());
    // Warm first, so the count below is this call and not the compilation behind it.
    assert_eq!(h.run("pure.step", &[Value::Int(5)]), Value::Int(16));
    h.bodies.reset_counts();

    let bodies = Rc::clone(&h.bodies);
    let before = h.entered();
    let inside = bodies.while_entered(|| {
        h.machine
            .call("pure.step", vec![Value::Int(5)], Span::DUMMY)
    });
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
    let declined = inside.expect_err("a reentrant entry is declined, not served");
    assert_eq!(
        declined.code,
        ply_span::codes::RUNTIME_ERROR,
        "the decline arrived as something other than a runtime error: {declined}"
    );

    let after = h.run("pure.step", &[Value::Int(5)]);
    assert_eq!(after, Value::Int(16));
    assert_eq!(
        h.entered(),
        before + 1,
        "the entry after a reentrant decline was not taken"
    );
}

#[test]
fn a_raced_simulate_region_answers_and_records_the_race() {
    let mut h = harness(hazards());
    for (n, want) in [(1, 90), (4, 201)] {
        assert_eq!(h.run("raced.raced", &[Value::Int(n)]), Value::Int(want));
        let steps = &h
            .machine
            .simulated()
            .expect("the region left a record")
            .steps;
        let spawned: Vec<_> = steps
            .iter()
            .filter(|s| s.task != ply_eval::sched::ROOT)
            .collect();
        assert!(
            spawned.iter().any(|a| spawned
                .iter()
                .any(|b| a.task != b.task && a.accesses.conflicts_with(&b.accesses))),
            "`raced.raced({n})`: no two spawned tasks conflict on the cell: {steps:?}"
        );
    }
}
