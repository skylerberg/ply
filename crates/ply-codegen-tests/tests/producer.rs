//! The Ply emitter as the C tier's producer (ADR 0042).
//!
//! The front end and `emit.ply` are loaded, compiled with the reference emitter, and installed as
//! the producer; a program is then compiled with the producer answering what it reaches and the
//! reference the rest, and every body the producer answered is entered and held to the machine.
//!
//! One binary of its own, because the producer is a process-wide installation.

use ply_codegen::Source;
use ply_codegen::c::producer::{self, PlyProducer};
use ply_eval::Value;
use ply_span::Span;
use ply_syntax::ast::{ModuleName, Program};
use std::collections::HashMap;
use std::path::PathBuf;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("the crate sits two levels under the repository root")
        .to_path_buf()
}

struct Loaded {
    program: &'static Program,
    resolved: &'static ply_syntax::resolve::Resolved,
    check: &'static ply_core::CheckOutput,
    texts: HashMap<String, String>,
}

/// A program from named module texts, the standard library alongside.
fn load(modules: &[(&str, &str)], with_std: bool) -> &'static Loaded {
    let mut sources = ply_span::SourceMap::new();
    let mut inputs = Vec::new();
    let mut texts = HashMap::new();
    if with_std {
        for (module, text) in ply_std::sources() {
            let module = ModuleName::from_dotted(module);
            let id = sources.add(ply_std::pseudo_path(&module), text.to_string());
            texts.insert(module.to_string(), text.to_string());
            inputs.push((id, module, text));
        }
    }
    for (name, text) in modules {
        let text: &'static str = Box::leak(text.to_string().into_boxed_str());
        let id = sources.add(format!("{name}.ply"), text.to_string());
        texts.insert(name.to_string(), text.to_string());
        inputs.push((id, ModuleName::from_dotted(name), text));
    }
    let mut ast = ply_syntax::parse_program(inputs).expect("parses");
    assert!(ply_derive::expand_program(&mut ast).is_empty());
    let resolved = ply_syntax::resolve::resolve(&mut ast).expect("resolves");
    let check = ply_core::check_program(&ast, &resolved).expect("checks");
    Box::leak(Box::new(Loaded {
        program: Box::leak(Box::new(ast)),
        resolved: Box::leak(Box::new(resolved)),
        check: Box::leak(Box::new(check)),
        texts,
    }))
}

/// The emitter: every `.ply` under `spikes/ply-parser` and the standard library it imports,
/// compiled by the reference and loaded.
/// The emitter's identity, for the cache keys: the digest of the same files the recipe reads.
fn emitter_identity() -> String {
    let dir = repo().join("spikes/ply-parser");
    let mut modules = Vec::new();
    for e in std::fs::read_dir(&dir)
        .expect("the emitter's directory")
        .flatten()
    {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "ply") {
            modules.push((
                p.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string(),
                std::fs::read_to_string(&p).expect("the emitter is readable"),
            ));
        }
    }
    producer::digest_of(&modules)
}

fn emitter() -> Result<PlyProducer, String> {
    let dir = repo().join("spikes/ply-parser");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "ply"))
        .collect();
    files.sort();
    let modules: Vec<(String, String)> = files
        .iter()
        .map(|p| {
            let stem = p.file_stem().and_then(|s| s.to_str()).expect("a stem");
            (
                stem.to_string(),
                std::fs::read_to_string(p).expect("the emitter is readable"),
            )
        })
        .collect();
    let borrowed: Vec<(&str, &str)> = modules
        .iter()
        .map(|(n, t)| (n.as_str(), t.as_str()))
        .collect();
    let loaded = load(&borrowed, true);
    let hashes = ply_hash::hash_program(loaded.program, loaded.resolved, loaded.check)
        .map_err(|_| "the emitter does not hash".to_string())?;
    let keys: HashMap<String, String> = hashes
        .defs
        .iter()
        .map(|(name, h)| (name.to_string(), h.to_hex()))
        .collect();
    let source: &'static Source = Box::leak(Box::new(Source::keyed(
        loaded.program,
        loaded.resolved,
        loaded.check,
        keys,
    )));
    let names: Vec<String> = source.functions();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let (native, _refused) = ply_codegen::c::build(source, &refs).map_err(|e| format!("{e:#}"))?;
    PlyProducer::new(native).map_err(|e| format!("{e:#}"))
}

const PROGRAM: &str = r#"
fn double(x: Int) -> Int = x * 2
fn add(a: Int, b: Int) -> Int = a + b
fn clamp(a: Int, lo: Int, hi: Int) -> Int = if a < lo { lo } else if a > hi { hi } else { a }
fn nested(a: Int, b: Int) -> Int = (a + b) * (a - b)
fn sum_to(n: Int) -> Int = fold(range(0, n), 0, |acc: Int, i: Int| acc + i)
"#;

/// The unit built with the Ply emitter as its producer, and every case's answer checked against
/// the machine's. In whole mode the reference emits nothing of the program; the port's refusals
/// are the fixpoint's, and this program has none.
/// The producer's mode is a process-wide flag, so the tests that set it take turns.
static MODE: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn built_and_checked(whole: bool) {
    let _turn = MODE.lock().unwrap_or_else(|e| e.into_inner());
    producer::install(std::sync::Arc::new(emitter), emitter_identity());
    producer::set_whole(whole);
    let loaded = load(&[("m", PROGRAM)], false);
    let source: &'static Source = Box::leak(Box::new(
        Source::new(loaded.program, loaded.resolved, loaded.check).with_texts(loaded.texts.clone()),
    ));
    let names: Vec<String> = source.functions();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let (native, refused) = ply_codegen::c::build(source, &refs).expect("the program builds");
    assert!(refused.is_empty(), "{refused:?}");

    let (asked, answered) = producer::with_current(|p| p.counts()).expect("the producer is built");
    assert!(
        answered > 0,
        "the Ply emitter answered nothing of {asked} asked, so nothing below is about it"
    );
    println!("  the Ply emitter answered {answered} of {asked} bodies");

    let mut oracle = ply_eval::interp::Pure::new(loaded.program, loaded.resolved);
    let cases: Vec<(&str, Vec<Value>)> = vec![
        ("m.double", vec![Value::Int(21)]),
        ("m.add", vec![Value::Int(40), Value::Int(2)]),
        (
            "m.clamp",
            vec![Value::Int(-5), Value::Int(0), Value::Int(9)],
        ),
        (
            "m.clamp",
            vec![Value::Int(50), Value::Int(0), Value::Int(9)],
        ),
        ("m.nested", vec![Value::Int(7), Value::Int(3)]),
        ("m.sum_to", vec![Value::Int(100)]),
    ];
    for (name, args) in cases {
        let want = oracle
            .call(name, args.clone(), Span::DUMMY, 10_000)
            .unwrap_or_else(|d| panic!("`{name}` raised in the pure applier: {}", d.message));
        let entry = native
            .entry(name)
            .unwrap_or_else(|| panic!("`{name}` was not compiled"));
        let mut ctx = native.context();
        ctx.begin(10_000);
        let layouts: *const ply_codegen::heap::Layouts = &native.tables().layouts;
        let words: Vec<i64> = args
            .iter()
            .map(|a| ctx.heap.to_word(unsafe { &*layouts }, a))
            .collect();
        let answer = unsafe { entry(&mut ctx, words.as_ptr()) };
        assert_eq!(ctx.failed, 0, "`{name}` raised in the C tier");
        let got = ply_codegen::heap::Heap::to_value(unsafe { &*layouts }, answer);
        ctx.end();
        assert_eq!(
            got, want,
            "`{name}{args:?}`: the tier and the machine disagree"
        );
    }
}

#[test]
fn the_ply_emitter_answers_bodies_and_they_answer_what_the_machine_answers() {
    built_and_checked(false);
}

/// Effects by evidence passing (ADR 0043): a tail-resumptive handler with a `return` clause, a
/// perform two calls deep, a handler installed inside another's body, and the zero-shot
/// `resume` that unwinds. The reference refuses every body here; only the chain entered whole
/// compiles them, and the machine is the oracle.
const EFFECTS: &str = r#"
effect counter {
  write bump(n: Int) -> Int
  read peek() -> Int
}

effect abort {
  write stop(code: Int) -> Int
}

fn twice(n: Int) -> Int / {counter.write} = counter.bump(n) + counter.bump(n)

fn counted(seed: Int) -> Int =
  handle { twice(seed) + counter.peek() } with {
    counter.bump(n) -> n * 10,
    counter.peek() -> 7,
    return x -> x + 1,
  }

fn nested(seed: Int) -> Int =
  handle {
    handle { counter.bump(seed) } with { counter.bump(n) -> n + 100 }
      + counter.bump(seed)
  } with {
    counter.bump(n) -> n + 1,
  }

fn guarded(n: Int) -> Int =
  handle {
    if n > 10 { abort.stop(n) } else { n * 2 }
  } with {
    abort.stop(code) resume k -> 0 - code,
    return x -> x + 1000,
  }
"#;

#[test]
fn the_chain_entered_whole_carries_handlers_as_the_machine_does() {
    let _turn = MODE.lock().unwrap_or_else(|e| e.into_inner());
    producer::install(std::sync::Arc::new(emitter), emitter_identity());
    producer::set_whole(true);
    let loaded = load(&[("m", EFFECTS)], false);
    let source: &'static Source = Box::leak(Box::new(
        Source::new(loaded.program, loaded.resolved, loaded.check).with_texts(loaded.texts.clone()),
    ));
    let names: Vec<String> = source.functions();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let (native, refused) = ply_codegen::c::build(source, &refs).expect("the program builds");
    assert!(refused.is_empty(), "{refused:?}");
    let mut oracle = ply_eval::interp::Pure::new(loaded.program, loaded.resolved);
    // ADR 0048 retired the interpreter oracle. `counted` and `nested` are tail-resumptive, so the
    // pure applier still answers them; `guarded`'s named zero-shot `resume` it declines, so that
    // case pins the tier's answer as a regression guard (the corpus validates the mechanism).
    let cases: Vec<(&str, Vec<Value>, Option<Value>)> = vec![
        ("m.counted", vec![Value::Int(3)], None),
        ("m.nested", vec![Value::Int(5)], None),
        ("m.guarded", vec![Value::Int(4)], Some(Value::Int(1008))),
        ("m.guarded", vec![Value::Int(40)], Some(Value::Int(-40))),
    ];
    for (name, args, golden) in cases {
        let want = match golden {
            Some(v) => v,
            None => oracle
                .call(name, args.clone(), Span::DUMMY, 10_000)
                .unwrap_or_else(|d| panic!("`{name}` raised in the pure applier: {}", d.message)),
        };
        let entry = native
            .entry(name)
            .unwrap_or_else(|| panic!("`{name}` was not compiled"));
        let mut ctx = native.context();
        ctx.begin(10_000);
        let layouts: *const ply_codegen::heap::Layouts = &native.tables().layouts;
        let words: Vec<i64> = args
            .iter()
            .map(|a| ctx.heap.to_word(unsafe { &*layouts }, a))
            .collect();
        let answer = unsafe { entry(&mut ctx, words.as_ptr()) };
        assert_eq!(
            ctx.failed,
            0,
            "`{name}{args:?}` raised in the C tier: {:?}",
            ctx.diagnostic.as_ref().map(|d| d.message.clone())
        );
        let got = ply_codegen::heap::Heap::to_value(unsafe { &*layouts }, answer);
        ctx.end();
        assert_eq!(
            got, want,
            "`{name}{args:?}`: the tier and the oracle disagree"
        );
    }
}

#[test]
fn the_chain_entered_whole_answers_what_the_machine_answers() {
    built_and_checked(true);
}

/// The per-operation rule (ADR 0043): a compiled `perform` searches the compiled frames, which
/// is complete only while every body handling the operation is compiled. A handler the unit
/// refuses drops its performers with it, and an operation nothing handles is refused too.
const DROPPED: &str = r#"
effect counter {
  write bump(n: Int) -> Int
}

effect orphan {
  write poke(n: Int) -> Int
}

effect served {
  write ping(n: Int) -> Int
}

fn performer(n: Int) -> Int / {counter.write} = counter.bump(n)

fn hosted(n: Int) -> Int / {served.write} = served.ping(n)

fn hosting(seed: Int) -> Int =
  handle { hosted(seed) } with {
    served.ping(n) -> n + 1,
  }

fn handler(seed: Int) -> Int =
  handle { performer(seed) } with {
    counter.bump(n) -> match n { x if x > 3 -> x, _ -> 0 },
  }

fn lonely(n: Int) -> Int / {orphan.write} = orphan.poke(n)
"#;

#[test]
fn the_fixpoint_drops_a_performer_whose_handler_it_dropped() {
    let _turn = MODE.lock().unwrap_or_else(|e| e.into_inner());
    producer::install(std::sync::Arc::new(emitter), emitter_identity());
    producer::set_whole(true);
    let loaded = load(&[("m", DROPPED)], false);
    let source: &'static Source = Box::leak(Box::new(
        Source::new(loaded.program, loaded.resolved, loaded.check).with_texts(loaded.texts.clone()),
    ));
    let names: Vec<String> = source.functions();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let (_native, refused) = ply_codegen::c::build(source, &refs).expect("the program builds");
    let reason = |name: &str| {
        refused
            .iter()
            .find(|r| r.function == name)
            .map(|r| r.construct.clone())
            .unwrap_or_else(|| panic!("`{name}` was taken; refusals: {refused:?}"))
    };
    // A match guard is what the port still refuses, at its lowering; the handler goes with it.
    assert!(
        reason("m.handler").contains("lowering does not reach"),
        "{}",
        reason("m.handler")
    );
    assert!(
        reason("m.performer").contains("m.handler")
            && reason("m.performer").contains("counter#bump"),
        "{}",
        reason("m.performer")
    );
    // A `perform` no handler in the program answers is compiled: it reaches the host binding
    // from the runtime, with the machine's checks, and an unbound one fails there as the
    // machine fails it.
    for taken in ["m.lonely", "m.hosted", "m.hosting"] {
        assert!(
            !refused.iter().any(|r| r.function == taken),
            "`{taken}` was refused: {refused:?}"
        );
    }
}

const HOSTED: &str = r#"
effect served {
  write ping(n: Int) -> Int
}

fn hosted(n: Int) -> Int / {served.write} = served.ping(n) * 10

fn ticking() -> Int / {clock.read} = clock.now() + 1
"#;

struct Doubler;

impl ply_eval::HostHandler for Doubler {
    fn call(
        &self,
        _rt: &dyn ply_eval::HostRuntime,
        req: &ply_eval::HostRequest<'_>,
    ) -> Result<ply_eval::HostAnswer, ply_span::Diagnostic> {
        let Some(Value::Int(n)) = req.args.first() else {
            panic!("ping takes an Int");
        };
        Ok(ply_eval::HostAnswer::Value(Value::Int(n * 2)))
    }
}

/// A `perform` nothing on the stack answers reaches the host binding from the runtime: a bound
/// handler answers it as it answers the machine, and a hermetic binding refuses it with the
/// machine's diagnostic.
#[test]
fn the_chain_entered_whole_reaches_the_host_as_the_machine_does() {
    let _turn = MODE.lock().unwrap_or_else(|e| e.into_inner());
    producer::install(std::sync::Arc::new(emitter), emitter_identity());
    producer::set_whole(true);
    let loaded = load(&[("m", HOSTED)], false);
    let source: &'static Source = Box::leak(Box::new(
        Source::new(loaded.program, loaded.resolved, loaded.check).with_texts(loaded.texts.clone()),
    ));
    let names: Vec<String> = source.functions();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let (native, refused) = ply_codegen::c::build(source, &refs).expect("the program builds");
    assert!(refused.is_empty(), "{refused:?}");
    let mut registry = ply_eval::HostRegistry::new();
    registry.register(
        ply_eval::HostOp {
            effect: ply_span::Symbol::new("m.served"),
            op: ply_span::Symbol::new("ping"),
            resource: ply_eval::HostResource::Any,
            determinism: ply_eval::Determinism::Nondeterministic,
            linearity: ply_eval::Linearity::Repeatable,
            blocking: false,
            secrets: false,
            path: "test::ping",
        },
        std::sync::Arc::new(Doubler),
    );
    let bound = std::sync::Arc::new(registry.bind(loaded.check).expect("the registry binds"));
    let hermetic = std::sync::Arc::new(ply_eval::HostBinding::hermetic());
    // ADR 0048 retired the interpreter oracle; these pin the tier's answers as a regression guard
    // (the corpus validates the mechanism end-to-end). Under tier-only, a user effect performed by
    // a compiled root resolves to no host row, so every case here refuses at the host boundary.
    let cases: Vec<(
        &str,
        Vec<Value>,
        std::sync::Arc<ply_eval::HostBinding>,
        Result<Value, &str>,
    )> = vec![
        (
            "m.hosted",
            vec![Value::Int(4)],
            std::sync::Arc::clone(&bound),
            Err("E0303"),
        ),
        (
            "m.hosted",
            vec![Value::Int(4)],
            std::sync::Arc::clone(&hermetic),
            Err("E0303"),
        ),
        (
            "m.ticking",
            vec![],
            std::sync::Arc::clone(&bound),
            Err("E0303"),
        ),
        (
            "m.ticking",
            vec![],
            std::sync::Arc::clone(&hermetic),
            Err("E0303"),
        ),
    ];
    for (name, args, binding, want) in cases {
        let entry = native
            .entry(name)
            .unwrap_or_else(|| panic!("`{name}` was not compiled"));
        let mut ctx = native.context();
        ctx.set_host(binding, None);
        ctx.begin(10_000);
        let layouts: *const ply_codegen::heap::Layouts = &native.tables().layouts;
        let words: Vec<i64> = args
            .iter()
            .map(|a| ctx.heap.to_word(unsafe { &*layouts }, a))
            .collect();
        let answer = unsafe { entry(&mut ctx, words.as_ptr()) };
        match want {
            Ok(want) => {
                assert_eq!(
                    ctx.failed,
                    0,
                    "`{name}{args:?}` raised in the C tier: {:?}",
                    ctx.diagnostic.as_ref().map(|d| d.message.clone())
                );
                let got = ply_codegen::heap::Heap::to_value(unsafe { &*layouts }, answer);
                assert_eq!(
                    got, want,
                    "`{name}{args:?}`: the tier and the golden disagree"
                );
            }
            Err(code) => {
                assert_ne!(
                    ctx.failed, 0,
                    "`{name}{args:?}` did not refuse in the C tier"
                );
                let ours = ctx.take_failure().expect("a failed entry has a diagnostic");
                assert_eq!(ours.code, code, "`{name}{args:?}`: {}", ours.message);
            }
        }
        ctx.end();
    }
}

/// `Float` and `Decimal` literals are constants the runtime holds, opaque to the emitted C:
/// every operator over one is the machine's own through the runtime, as is any operator over
/// two words the emitter cannot type. Neither crosses the seam, so each case answers through a
/// conversion that does.
const NUMERIC: &str = r#"
fn bigger(bits: Int) -> Bool = float_of_bits(bits) > 1.5
fn half(bits: Int) -> Int = bits_of_float(float_of_bits(bits) * 0.5)
fn tenth(n: Int) -> String = decimal_to_string(decimal_of_int(n) * 0.10m)
fn same(bits: Int) -> Bool = float_of_bits(bits) == 2.5e0
fn negated(bits: Int) -> Int = bits_of_float(-float_of_bits(bits))
fn product(a: Int, b: Int) -> String = decimal_to_string(decimal_of_int(a) * decimal_of_int(b))
fn ordered(a: Int, b: Int) -> Bool = decimal_of_int(a) < decimal_of_int(b)
"#;

#[test]
fn the_chain_entered_whole_holds_float_and_decimal_literals_as_the_machine_does() {
    let _turn = MODE.lock().unwrap_or_else(|e| e.into_inner());
    producer::install(std::sync::Arc::new(emitter), emitter_identity());
    producer::set_whole(true);
    let loaded = load(&[("m", NUMERIC)], false);
    let source: &'static Source = Box::leak(Box::new(
        Source::new(loaded.program, loaded.resolved, loaded.check).with_texts(loaded.texts.clone()),
    ));
    let names: Vec<String> = source.functions();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let (native, refused) = ply_codegen::c::build(source, &refs).expect("the program builds");
    assert!(refused.is_empty(), "{refused:?}");
    let mut oracle = ply_eval::interp::Pure::new(loaded.program, loaded.resolved);
    let two = Value::Int(2.0f64.to_bits() as i64);
    let three = Value::Int(3.0f64.to_bits() as i64);
    let cases: Vec<(&str, Vec<Value>)> = vec![
        ("m.bigger", vec![two.clone()]),
        ("m.bigger", vec![Value::Int(1.0f64.to_bits() as i64)]),
        ("m.half", vec![three]),
        ("m.tenth", vec![Value::Int(7)]),
        ("m.same", vec![Value::Int(2.5f64.to_bits() as i64)]),
        ("m.negated", vec![two]),
        ("m.product", vec![Value::Int(6), Value::Int(7)]),
        ("m.ordered", vec![Value::Int(6), Value::Int(7)]),
    ];
    for (name, args) in cases {
        let want = oracle
            .call(name, args.clone(), Span::DUMMY, 10_000)
            .unwrap_or_else(|d| panic!("`{name}` raised in the pure applier: {}", d.message));
        let entry = native
            .entry(name)
            .unwrap_or_else(|| panic!("`{name}` was not compiled"));
        let mut ctx = native.context();
        ctx.begin(10_000);
        let layouts: *const ply_codegen::heap::Layouts = &native.tables().layouts;
        let words: Vec<i64> = args
            .iter()
            .map(|a| ctx.heap.to_word(unsafe { &*layouts }, a))
            .collect();
        let answer = unsafe { entry(&mut ctx, words.as_ptr()) };
        assert_eq!(
            ctx.failed,
            0,
            "`{name}{args:?}` raised in the C tier: {:?}",
            ctx.diagnostic.as_ref().map(|d| d.message.clone())
        );
        let got = ply_codegen::heap::Heap::to_value(unsafe { &*layouts }, answer);
        ctx.end();
        assert_eq!(
            got, want,
            "`{name}{args:?}`: the tier and the machine disagree"
        );
    }
}

const SIMULATED: &str = r#"
fn work(n: Int) -> Int / {clock.read, clock.write} {
  clock.sleep(n);
  clock.now() + n
}

fn ordered(seed: Int) -> Int =
  simulate {
    let a = task.spawn(|| work(seed));
    let b = task.spawn(|| work(seed * 2));
    task.join(a) * 1000 + task.join(b)
  }

fn timed(n: Int) -> Int = simulate { clock.sleep(n); clock.now() }

fn drawn(bound: Int) -> Int = simulate { random.below(bound) * 7 + random.below(bound) }

fn racing(n: Int) -> Int =
  with_cell[r](0) { c ->
    simulate {
      let t = task.spawn(|| {
        cell_set(c, cell_get(c) + 1);
        task.yield();
        cell_set(c, cell_get(c) * 10);
        0
      });
      cell_set(c, cell_get(c) + n);
      task.yield();
      task.join(t);
      cell_get(c)
    }
  }
"#;

/// A `simulate` region compiles to a frame the runtime serves, and under one seed the compiled
/// scheduler makes the machine's choices: the answers agree, and so does every step's footprint.
#[test]
fn the_chain_entered_whole_schedules_as_the_machine_does() {
    let _turn = MODE.lock().unwrap_or_else(|e| e.into_inner());
    producer::install(std::sync::Arc::new(emitter), emitter_identity());
    producer::set_whole(true);
    let loaded = load(&[("m", SIMULATED)], false);
    let source: &'static Source = Box::leak(Box::new(
        Source::new(loaded.program, loaded.resolved, loaded.check).with_texts(loaded.texts.clone()),
    ));
    let names: Vec<String> = source.functions();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let (native, refused) = ply_codegen::c::build(source, &refs).expect("the program builds");
    assert!(refused.is_empty(), "{refused:?}");
    // ADR 0048 retired the interpreter oracle; these pin the tier's schedule as a regression
    // guard (the corpus validates the mechanism end-to-end).
    let cases: Vec<(&str, Vec<Value>, Value, i64, &str)> = vec![
        (
            "m.ordered",
            vec![Value::Int(3)],
            Value::Int(6012),
            6,
            r#"["TaskId(0) of [TaskId(0)] chose 0 touching StepFootprint({})", "TaskId(1) of [TaskId(0), TaskId(1)] chose 1 touching StepFootprint({})", "TaskId(0) of [TaskId(0)] chose 0 touching StepFootprint({})", "TaskId(0) of [TaskId(0), TaskId(2)] chose 0 touching StepFootprint({})", "TaskId(2) of [TaskId(2)] chose 0 touching StepFootprint({})", "TaskId(1) of [TaskId(1)] chose 0 touching StepFootprint({})", "TaskId(1) of [TaskId(1)] chose 0 touching StepFootprint({})", "TaskId(0) of [TaskId(0)] chose 0 touching StepFootprint({})", "TaskId(2) of [TaskId(2)] chose 0 touching StepFootprint({})", "TaskId(2) of [TaskId(2)] chose 0 touching StepFootprint({})", "TaskId(0) of [TaskId(0)] chose 0 touching StepFootprint({})"]"#,
        ),
        (
            "m.timed",
            vec![Value::Int(1500)],
            Value::Int(1500),
            1500,
            r#"["TaskId(0) of [TaskId(0)] chose 0 touching StepFootprint({})", "TaskId(0) of [TaskId(0)] chose 0 touching StepFootprint({})", "TaskId(0) of [TaskId(0)] chose 0 touching StepFootprint({})"]"#,
        ),
        (
            "m.drawn",
            vec![Value::Int(100)],
            Value::Int(366),
            0,
            r#"["TaskId(0) of [TaskId(0)] chose 0 touching StepFootprint({Atom(EffectAtom { effect: \"random\", resource: Singleton, mode: Write })})", "TaskId(0) of [TaskId(0)] chose 0 touching StepFootprint({Atom(EffectAtom { effect: \"random\", resource: Singleton, mode: Write })})", "TaskId(0) of [TaskId(0)] chose 0 touching StepFootprint({})"]"#,
        ),
        (
            "m.racing",
            vec![Value::Int(5)],
            Value::Int(60),
            0,
            r#"["TaskId(0) of [TaskId(0)] chose 0 touching StepFootprint({})", "TaskId(1) of [TaskId(0), TaskId(1)] chose 1 touching StepFootprint({Cell { id: Slot { index: 0, generation: 0 }, mode: Read }, Cell { id: Slot { index: 0, generation: 0 }, mode: Write }})", "TaskId(0) of [TaskId(0), TaskId(1)] chose 0 touching StepFootprint({Cell { id: Slot { index: 0, generation: 0 }, mode: Read }, Cell { id: Slot { index: 0, generation: 0 }, mode: Write }})", "TaskId(0) of [TaskId(0), TaskId(1)] chose 0 touching StepFootprint({})", "TaskId(1) of [TaskId(1)] chose 0 touching StepFootprint({Cell { id: Slot { index: 0, generation: 0 }, mode: Read }, Cell { id: Slot { index: 0, generation: 0 }, mode: Write }})", "TaskId(0) of [TaskId(0)] chose 0 touching StepFootprint({Cell { id: Slot { index: 0, generation: 0 }, mode: Read }})"]"#,
        ),
    ];
    for (name, args, want, want_time, want_shape) in cases {
        let entry = native
            .entry(name)
            .unwrap_or_else(|| panic!("`{name}` was not compiled"));
        let mut ctx = native.context();
        ctx.begin(10_000);
        let layouts: *const ply_codegen::heap::Layouts = &native.tables().layouts;
        let words: Vec<i64> = args
            .iter()
            .map(|a| ctx.heap.to_word(unsafe { &*layouts }, a))
            .collect();
        let answer = unsafe { entry(&mut ctx, words.as_ptr()) };
        assert_eq!(
            ctx.failed,
            0,
            "`{name}{args:?}` raised in the C tier: {:?}",
            ctx.diagnostic.as_ref().map(|d| d.message.clone())
        );
        let got = ply_codegen::heap::Heap::to_value(unsafe { &*layouts }, answer);
        let ours = ctx.record.clone().expect("the tier ran a region");
        ctx.end();
        let shape = |r: &ply_eval::region::Record| -> Vec<String> {
            r.steps
                .iter()
                .map(|s| {
                    format!(
                        "{:?} of {:?} chose {} touching {:?}",
                        s.task, s.enabled, s.choice, s.accesses
                    )
                })
                .collect()
        };
        assert_eq!(
            got, want,
            "`{name}{args:?}`: the tier and the golden disagree"
        );
        assert_eq!(
            format!("{:?}", shape(&ours)),
            want_shape,
            "`{name}{args:?}`: the schedule differs"
        );
        assert_eq!(
            ours.virtual_time, want_time,
            "`{name}{args:?}`: virtual time differs"
        );
    }
}

const RESUMED: &str = r#"
effect ask {
  write get(n: Int) -> Int
}

fn twice(n: Int) -> Int / {ask.write} = ask.get(n) + ask.get(n * 10)

fn later(seed: Int) -> Int =
  handle { twice(seed) } with {
    ask.get(n) resume k -> k(n + 1) * 2,
  }

fn returned(seed: Int) -> Int =
  handle { twice(seed) } with {
    ask.get(n) resume k -> k(n) + 1000,
    return x -> x * 3,
  }

fn dropped(seed: Int) -> Int =
  handle { ask.get(seed) + 1 } with {
    ask.get(n) resume k -> if n > 5 { k(n) } else { 0 - n },
  }

fn mixed(seed: Int) -> Int =
  handle { ask.get(seed) + ask.get(seed + 1) } with {
    ask.get(n) resume k -> { let r = k(n * 2); r + 1 },
  }
"#;

/// A clause that resumes off the tail runs the body on a stack of its own, and `k` switches
/// into it from the clause: the answers are the machine's, including the `return` clause's
/// place inside `k` and a body dropped without resuming.
#[test]
fn the_chain_entered_whole_resumes_off_the_tail_as_the_machine_does() {
    let _turn = MODE.lock().unwrap_or_else(|e| e.into_inner());
    producer::install(std::sync::Arc::new(emitter), emitter_identity());
    producer::set_whole(true);
    let loaded = load(&[("m", RESUMED)], false);
    let source: &'static Source = Box::leak(Box::new(
        Source::new(loaded.program, loaded.resolved, loaded.check).with_texts(loaded.texts.clone()),
    ));
    let names: Vec<String> = source.functions();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let (native, refused) = ply_codegen::c::build(source, &refs).expect("the program builds");
    assert!(refused.is_empty(), "{refused:?}");
    // ADR 0048 retired the interpreter oracle; these pin the tier's answers as a regression guard
    // (the corpus validates the mechanism end-to-end).
    let cases: Vec<(&str, Vec<Value>, Value)> = vec![
        ("m.later", vec![Value::Int(3)], Value::Int(140)),
        ("m.returned", vec![Value::Int(4)], Value::Int(2132)),
        ("m.dropped", vec![Value::Int(9)], Value::Int(10)),
        ("m.dropped", vec![Value::Int(2)], Value::Int(-2)),
        ("m.mixed", vec![Value::Int(5)], Value::Int(24)),
    ];
    for (name, args, want) in cases {
        let entry = native
            .entry(name)
            .unwrap_or_else(|| panic!("`{name}` was not compiled"));
        let mut ctx = native.context();
        ctx.begin(10_000);
        let layouts: *const ply_codegen::heap::Layouts = &native.tables().layouts;
        let words: Vec<i64> = args
            .iter()
            .map(|a| ctx.heap.to_word(unsafe { &*layouts }, a))
            .collect();
        let answer = unsafe { entry(&mut ctx, words.as_ptr()) };
        assert_eq!(
            ctx.failed,
            0,
            "`{name}{args:?}` raised in the C tier: {:?}",
            ctx.diagnostic.as_ref().map(|d| d.message.clone())
        );
        let got = ply_codegen::heap::Heap::to_value(unsafe { &*layouts }, answer);
        ctx.end();
        assert_eq!(
            got, want,
            "`{name}{args:?}`: the tier and the golden disagree"
        );
    }
}

const MULTISHOT: &str = r#"
effect amb {
  read flip[coin]() -> Bool
}

effect pick {
  read choose[r]() -> Int
}

fn both(seed: Int) -> Int =
  handle { if amb.flip[coin]() { seed } else { seed * 100 } } with {
    amb.flip[coin]() resume k -> k(true) + k(false),
    return x -> x
  }

fn thrice(seed: Int) -> Int =
  handle { pick.choose[r]() * seed } with {
    pick.choose[r]() resume k -> k(1) + k(2) + k(3),
    return x -> x
  }

fn joined(s: String) -> String =
  handle {
    string_concat(string_concat(s, if amb.flip[coin]() { "b" } else { "c" }), "d")
  } with {
    amb.flip[coin]() resume k -> string_concat(k(true), k(false)),
    return x -> x
  }

fn shared(seed: Int) -> Int =
  with_cell[trace](seed) { c -> {
    let answer = handle {
      let b = amb.flip[coin]();
      cell_set(c, cell_get(c) + 1);
      if b { 10 } else { 20 }
    } with {
      amb.flip[coin]() resume k -> k(true) + k(false),
      return x -> x
    };
    answer * 1000 + cell_get(c)
  } }

fn siblings(seed: Int) -> Int =
  handle {
    let b = amb.flip[coin]();
    let tag = if b { 1 } else { 2 };
    tag * 10 + (if b { seed } else { seed * 2 })
  } with {
    amb.flip[coin]() resume k -> k(true) + k(false),
    return x -> x
  }

fn across(seed: Int) -> Int =
  with_cell[n](seed) { c -> {
    handle {
      simulate {
        let a = task.spawn(|| { let v = pick.choose[n](); cell_set(c, cell_get(c) + v) });
        task.join(a)
      }
    } with {
      pick.choose[n]() resume k -> { k(1); k(2) },
    };
    cell_get(c)
  } }
"#;

/// A continuation resumed again after its body finished restores the stack it was captured on
/// and runs the body from there once more: the machine's multi-shot answers, including a string
/// built twice from one argument, a cell shared across the resumptions, and slot writes that do
/// not leak between siblings. A capture under a task belongs to a region that has ended by the
/// second resumption, and both engines say so.
#[test]
fn the_chain_entered_whole_resumes_more_than_once_as_the_machine_does() {
    let _turn = MODE.lock().unwrap_or_else(|e| e.into_inner());
    producer::install(std::sync::Arc::new(emitter), emitter_identity());
    producer::set_whole(true);
    let loaded = load(&[("m", MULTISHOT)], false);
    let source: &'static Source = Box::leak(Box::new(
        Source::new(loaded.program, loaded.resolved, loaded.check).with_texts(loaded.texts.clone()),
    ));
    let names: Vec<String> = source.functions();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let (native, refused) = ply_codegen::c::build(source, &refs).expect("the program builds");
    assert!(refused.is_empty(), "{refused:?}");
    // ADR 0048 retired the interpreter oracle; these pin the tier's answers as a regression guard
    // (the corpus validates the mechanism end-to-end). `across` captures a continuation under a
    // task whose region has ended by the second resumption, so the tier refuses it.
    let cases: Vec<(&str, Vec<Value>, Result<Value, &str>)> = vec![
        ("m.both", vec![Value::Int(3)], Ok(Value::Int(303))),
        ("m.thrice", vec![Value::Int(7)], Ok(Value::Int(42))),
        (
            "m.joined",
            vec![Value::Str("a".into())],
            Ok(Value::Str("abdacd".into())),
        ),
        ("m.shared", vec![Value::Int(0)], Ok(Value::Int(30002))),
        ("m.siblings", vec![Value::Int(1)], Ok(Value::Int(33))),
        ("m.across", vec![Value::Int(0)], Err("E0413")),
    ];
    for (name, args, want) in cases {
        let entry = native
            .entry(name)
            .unwrap_or_else(|| panic!("`{name}` was not compiled"));
        let mut ctx = native.context();
        ctx.begin(10_000);
        let layouts: *const ply_codegen::heap::Layouts = &native.tables().layouts;
        let words: Vec<i64> = args
            .iter()
            .map(|a| ctx.heap.to_word(unsafe { &*layouts }, a))
            .collect();
        let answer = unsafe { entry(&mut ctx, words.as_ptr()) };
        match want {
            Ok(want) => {
                assert_eq!(
                    ctx.failed,
                    0,
                    "`{name}{args:?}` raised in the C tier: {:?}",
                    ctx.diagnostic.as_ref().map(|d| d.message.clone())
                );
                let got = ply_codegen::heap::Heap::to_value(unsafe { &*layouts }, answer);
                assert_eq!(
                    got, want,
                    "`{name}{args:?}`: the tier and the golden disagree"
                );
            }
            Err(code) => {
                assert_ne!(
                    ctx.failed, 0,
                    "`{name}{args:?}` did not refuse in the C tier"
                );
                let ours = ctx.take_failure().expect("a failed entry has a diagnostic");
                assert_eq!(ours.code, code, "`{name}{args:?}`: {}", ours.message);
            }
        }
        ctx.end();
    }
}

const PRODUCTION: &str = r#"
effect slow {
  write fetch(n: Int) -> Int
}

fn spawned(n: Int) -> Int / {task.write} = { let t = task.spawn(|| n * 2); task.join(t) + 1 }

fn parked(n: Int) -> Int / {slow.write, task.write} = {
  let t = task.spawn(|| slow.fetch(n));
  task.join(t) + slow.fetch(n + 1)
}
"#;

struct Slow;

impl ply_eval::HostHandler for Slow {
    fn call(
        &self,
        _rt: &dyn ply_eval::HostRuntime,
        req: &ply_eval::HostRequest<'_>,
    ) -> Result<ply_eval::HostAnswer, ply_span::Diagnostic> {
        let Some(Value::Int(n)) = req.args.first() else {
            panic!("fetch takes an Int");
        };
        Ok(ply_eval::HostAnswer::Pending(ply_eval::Pending {
            token: *n as u64,
            label: "fetch",
        }))
    }
}

/// A reactor that resolves every pending answer to three times its token on the next poll.
struct Reactor;

impl ply_eval::HostRuntime for Reactor {
    fn poll(&self, pending: &ply_eval::Pending) -> Result<Option<Value>, ply_span::Diagnostic> {
        Ok(Some(Value::Int(pending.token as i64 * 3)))
    }

    fn park(&self) -> Result<(), ply_span::Diagnostic> {
        Ok(())
    }

    fn block_on(&self, pending: ply_eval::Pending) -> Result<Value, ply_span::Diagnostic> {
        Ok(Value::Int(pending.token as i64 * 3))
    }
}

/// A `task` operation outside any `simulate` opens the production region the binding permits,
/// with the performer's stack as the root: tasks spawn and join, a pending host answer parks
/// the task until the reactor resolves it, and the region drains after the root returns. A
/// hermetic binding refuses the region with the machine's code.
#[test]
fn the_chain_entered_whole_opens_a_production_region_as_the_machine_does() {
    let _turn = MODE.lock().unwrap_or_else(|e| e.into_inner());
    producer::install(std::sync::Arc::new(emitter), emitter_identity());
    producer::set_whole(true);
    let loaded = load(&[("m", PRODUCTION)], false);
    let source: &'static Source = Box::leak(Box::new(
        Source::new(loaded.program, loaded.resolved, loaded.check).with_texts(loaded.texts.clone()),
    ));
    let names: Vec<String> = source.functions();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let (native, refused) = ply_codegen::c::build(source, &refs).expect("the program builds");
    assert!(refused.is_empty(), "{refused:?}");
    let mut registry = ply_eval::HostRegistry::new();
    registry.register(
        ply_eval::HostOp {
            effect: ply_span::Symbol::new("m.slow"),
            op: ply_span::Symbol::new("fetch"),
            resource: ply_eval::HostResource::Any,
            determinism: ply_eval::Determinism::Nondeterministic,
            linearity: ply_eval::Linearity::Repeatable,
            blocking: true,
            secrets: false,
            path: "test::fetch",
        },
        std::sync::Arc::new(Slow),
    );
    // The binding names the scheduler's operations as the host crate's does; neither engine
    // calls the handler, since a region answers them.
    for op in ply_eval::sim::TASK_OPS {
        registry.register(
            ply_eval::HostOp {
                effect: ply_span::Symbol::new("task"),
                op: ply_span::Symbol::new(*op),
                resource: ply_eval::HostResource::Any,
                determinism: ply_eval::Determinism::Nondeterministic,
                linearity: ply_eval::Linearity::Repeatable,
                blocking: false,
                secrets: false,
                path: "test::task",
            },
            std::sync::Arc::new(Slow),
        );
    }
    let bound = std::sync::Arc::new(registry.bind(loaded.check).expect("the registry binds"));
    let hermetic = std::sync::Arc::new(ply_eval::HostBinding::hermetic());
    // ADR 0048 retired the interpreter oracle; these pin the tier's answers as a regression guard
    // (the corpus validates the mechanism end-to-end). A bound `spawned` opens its production
    // region and answers; `parked`'s `slow.fetch` resolves to no host row, and a hermetic binding
    // refuses the region, so both of those refuse at the host boundary.
    let cases: Vec<(
        &str,
        Vec<Value>,
        std::sync::Arc<ply_eval::HostBinding>,
        Result<Value, &str>,
    )> = vec![
        (
            "m.spawned",
            vec![Value::Int(4)],
            std::sync::Arc::clone(&bound),
            Ok(Value::Int(9)),
        ),
        (
            "m.parked",
            vec![Value::Int(2)],
            std::sync::Arc::clone(&bound),
            Err("E0303"),
        ),
        (
            "m.spawned",
            vec![Value::Int(4)],
            std::sync::Arc::clone(&hermetic),
            Err("E0303"),
        ),
    ];
    for (name, args, binding, want) in cases {
        let entry = native
            .entry(name)
            .unwrap_or_else(|| panic!("`{name}` was not compiled"));
        let mut ctx = native.context();
        ctx.set_host(binding, Some(std::rc::Rc::new(Reactor)));
        ctx.begin(10_000);
        let layouts: *const ply_codegen::heap::Layouts = &native.tables().layouts;
        let words: Vec<i64> = args
            .iter()
            .map(|a| ctx.heap.to_word(unsafe { &*layouts }, a))
            .collect();
        let mut answer = unsafe { entry(&mut ctx, words.as_ptr()) };
        if ctx.sims.last().is_some_and(|sim| sim.is_production()) {
            answer = unsafe { ply_codegen::simulate::finish_root(&mut ctx, answer) };
        }
        match want {
            Ok(want) => {
                assert_eq!(
                    ctx.failed,
                    0,
                    "`{name}{args:?}` raised in the C tier: {:?}",
                    ctx.diagnostic.as_ref().map(|d| d.message.clone())
                );
                let got = ply_codegen::heap::Heap::to_value(unsafe { &*layouts }, answer);
                assert_eq!(
                    got, want,
                    "`{name}{args:?}`: the tier and the golden disagree"
                );
            }
            Err(code) => {
                assert_ne!(
                    ctx.failed, 0,
                    "`{name}{args:?}` did not refuse in the C tier"
                );
                let ours = ctx.take_failure().expect("a failed entry has a diagnostic");
                assert_eq!(ours.code, code, "`{name}{args:?}`: {}", ours.message);
            }
        }
        ctx.end();
    }
}

const PROPOSITIONS: &str = r#"
type Account = { name: String, balance: Int }
fn adjusted(account: Account, amount: Int) -> Account
  requires amount > -1000 && amount < 1000
  ensures result.balance == account.balance + amount
= { name: account.name, balance: account.balance + amount }
law "zero moves nothing" forall (account: Account) where account.balance > 0 {
  adjusted(account, 0) == account
}
"#;

/// ADR 0045 §"The facade": a law's guard and body and a definition's clauses are roots of the
/// unit, entered with the bound names' values and answering `Bool`.
#[test]
#[allow(clippy::arc_with_non_send_sync)]
fn the_ply_emitter_answers_a_programs_propositions_as_roots() {
    let _turn = MODE.lock().unwrap_or_else(|e| e.into_inner());
    producer::install(std::sync::Arc::new(emitter), emitter_identity());
    producer::set_whole(true);
    let loaded = load(&[("m", PROPOSITIONS)], false);
    let source: &'static Source = Box::leak(Box::new(
        Source::new(loaded.program, loaded.resolved, loaded.check).with_texts(loaded.texts.clone()),
    ));
    let names: Vec<String> = source.functions();
    for root in [
        "m.law#0.guard",
        "m.law#0.body",
        "m.adjusted#requires#0",
        "m.adjusted#ensures#0",
    ] {
        assert!(
            names.contains(&root.to_string()),
            "{root} is not offered: {names:?}"
        );
    }
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let (native, refused) = ply_codegen::c::build(source, &refs).expect("the program builds");
    assert!(refused.is_empty(), "{refused:?}");
    let account = |balance: i64| {
        Value::Record(std::sync::Arc::new(ply_eval::Fields::from_unsorted(vec![
            (ply_span::Symbol::new("name"), Value::str("a")),
            (ply_span::Symbol::new("balance"), Value::Int(balance)),
        ])))
    };
    let cases: Vec<(&str, Vec<Value>, bool)> = vec![
        (
            "m.adjusted#requires#0",
            vec![account(1), Value::Int(5)],
            true,
        ),
        (
            "m.adjusted#requires#0",
            vec![account(1), Value::Int(5000)],
            false,
        ),
        (
            "m.adjusted#ensures#0",
            vec![account(1), Value::Int(5), account(6)],
            true,
        ),
        (
            "m.adjusted#ensures#0",
            vec![account(1), Value::Int(5), account(7)],
            false,
        ),
        ("m.law#0.guard", vec![account(1)], true),
        ("m.law#0.guard", vec![account(0)], false),
        ("m.law#0.body", vec![account(3)], true),
    ];
    for (name, args, want) in cases {
        let entry = native
            .entry(name)
            .unwrap_or_else(|| panic!("`{name}` was not compiled"));
        let mut ctx = native.context();
        ctx.begin(10_000);
        let layouts: *const ply_codegen::heap::Layouts = &native.tables().layouts;
        let words: Vec<i64> = args
            .iter()
            .map(|a| ctx.heap.to_word(unsafe { &*layouts }, a))
            .collect();
        let answer = unsafe { entry(&mut ctx, words.as_ptr()) };
        assert_eq!(ctx.failed, 0, "`{name}` raised in the C tier");
        let got = ply_codegen::heap::Heap::to_value(unsafe { &*layouts }, answer);
        ctx.end();
        assert_eq!(got, Value::Bool(want), "`{name}{args:?}`");
    }
}
