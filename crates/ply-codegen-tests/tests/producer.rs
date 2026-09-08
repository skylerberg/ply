//! The Ply emitter as the C tier's producer (ADR 0042).
//!
//! The front end and `emit.ply` are loaded, compiled with the reference emitter, and installed as
//! the producer; a program is then compiled with the producer answering what it reaches and the
//! reference the rest, and every body the producer answered is entered and held to the machine.
//!
//! One binary of its own, because the producer is a process-wide installation.

use ply_codegen::Source;
use ply_codegen::c::producer::{self, PlyProducer};
use ply_eval::{Machine, Value};
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

    let mut machine = Machine::new(loaded.program, loaded.resolved, loaded.check);
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
        let want = machine
            .call(name, args.clone(), Span::DUMMY)
            .unwrap_or_else(|d| panic!("`{name}` raised in the machine: {}", d.message));
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
    let mut machine = Machine::new(loaded.program, loaded.resolved, loaded.check);
    let cases: Vec<(&str, Vec<Value>)> = vec![
        ("m.counted", vec![Value::Int(3)]),
        ("m.nested", vec![Value::Int(5)]),
        ("m.guarded", vec![Value::Int(4)]),
        ("m.guarded", vec![Value::Int(40)]),
    ];
    for (name, args) in cases {
        let want = machine
            .call(name, args.clone(), Span::DUMMY)
            .unwrap_or_else(|d| panic!("`{name}` raised in the machine: {}", d.message));
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
    counter.bump(n) -> match secret_of_string("hidden") { _ -> n + 1 },
  }

fn lonely(n: Int) -> Int / {orphan.write} = orphan.poke(n)
"#;

#[test]
fn the_fixpoint_drops_a_performer_whose_handler_it_dropped() {
    let _turn = MODE.lock().unwrap_or_else(|e| e.into_inner());
    producer::install(std::sync::Arc::new(emitter), emitter_identity());
    producer::set_whole(true);
    // An operation the run's host binding would answer stays the machine's, handler or not.
    producer::set_host_served(vec!["m.served#ping".to_string()]);
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
    assert!(
        reason("m.handler").contains("credential"),
        "{}",
        reason("m.handler")
    );
    assert!(
        reason("m.performer").contains("m.handler")
            && reason("m.performer").contains("counter#bump"),
        "{}",
        reason("m.performer")
    );
    assert!(
        reason("m.lonely").contains("nothing in the program"),
        "{}",
        reason("m.lonely")
    );
    assert!(
        reason("m.hosted").contains("the host"),
        "{}",
        reason("m.hosted")
    );
    // The handler calls the performer, so the cascade takes it too, naming the performer.
    assert!(
        reason("m.hosting").contains("m.hosted"),
        "{}",
        reason("m.hosting")
    );
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
    let mut machine = Machine::new(loaded.program, loaded.resolved, loaded.check);
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
        let want = machine
            .call(name, args.clone(), Span::DUMMY)
            .unwrap_or_else(|d| panic!("`{name}` raised in the machine: {}", d.message));
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
    let mut machine = Machine::new(loaded.program, loaded.resolved, loaded.check);
    let cases: Vec<(&str, Vec<Value>)> = vec![
        ("m.ordered", vec![Value::Int(3)]),
        ("m.timed", vec![Value::Int(1500)]),
        ("m.drawn", vec![Value::Int(100)]),
        ("m.racing", vec![Value::Int(5)]),
    ];
    for (name, args) in cases {
        let want = machine
            .call(name, args.clone(), Span::DUMMY)
            .unwrap_or_else(|d| panic!("`{name}` raised in the machine: {}", d.message));
        let theirs = machine
            .simulated()
            .expect("the machine ran a region")
            .clone();
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
        assert_eq!(
            got, want,
            "`{name}{args:?}`: the tier and the machine disagree"
        );
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
            shape(&ours),
            shape(&theirs),
            "`{name}{args:?}`: the schedules differ"
        );
        assert_eq!(
            ours.virtual_time, theirs.virtual_time,
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
    let mut machine = Machine::new(loaded.program, loaded.resolved, loaded.check);
    let cases: Vec<(&str, Vec<Value>)> = vec![
        ("m.later", vec![Value::Int(3)]),
        ("m.returned", vec![Value::Int(4)]),
        ("m.dropped", vec![Value::Int(9)]),
        ("m.dropped", vec![Value::Int(2)]),
        ("m.mixed", vec![Value::Int(5)]),
    ];
    for (name, args) in cases {
        let want = machine
            .call(name, args.clone(), Span::DUMMY)
            .unwrap_or_else(|d| panic!("`{name}` raised in the machine: {}", d.message));
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
    let mut machine = Machine::new(loaded.program, loaded.resolved, loaded.check);
    let cases: Vec<(&str, Vec<Value>)> = vec![
        ("m.both", vec![Value::Int(3)]),
        ("m.thrice", vec![Value::Int(7)]),
        ("m.joined", vec![Value::Str("a".into())]),
        ("m.shared", vec![Value::Int(0)]),
        ("m.siblings", vec![Value::Int(1)]),
        ("m.across", vec![Value::Int(0)]),
    ];
    for (name, args) in cases {
        let want = machine.call(name, args.clone(), Span::DUMMY);
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
                    "`{name}{args:?}`: the tier and the machine disagree"
                );
            }
            Err(theirs) => {
                assert_ne!(
                    ctx.failed, 0,
                    "`{name}{args:?}` raised in the machine ({}) and not in the C tier",
                    theirs.message
                );
                let ours = ctx.take_failure().expect("a failed entry has a diagnostic");
                assert_eq!(
                    ours.code, theirs.code,
                    "`{name}{args:?}`: {} vs {}",
                    ours.message, theirs.message
                );
            }
        }
        ctx.end();
    }
}
