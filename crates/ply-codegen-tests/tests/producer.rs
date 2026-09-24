//! One binary of its own, because the producer is a process-wide installation.

use ply_codegen::Source;
use ply_codegen::c::producer::{self, PlyProducer, Sources};
use ply_eval::Value;
use ply_span::SourceId;
use std::collections::HashMap;
use std::path::PathBuf;

/// A root, its arguments, the binding it runs under, and what it answers or refuses with.
type HostCase = (
    &'static str,
    Vec<Value>,
    std::sync::Arc<ply_eval::HostBinding>,
    Result<Value, &'static str>,
);

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("the crate sits two levels under the repository root")
        .to_path_buf()
}

struct Loaded {
    front: &'static ply_ty::Front,
    texts: HashMap<String, String>,
}

fn load(modules: &[(&str, &str)]) -> &'static Loaded {
    let named: Vec<(String, String)> = modules
        .iter()
        .map(|(name, text)| (name.to_string(), text.to_string()))
        .collect();
    let ids: Vec<SourceId> = (0..named.len()).map(|i| SourceId(i as u32)).collect();
    let front = producer::checked_front(&named, &ids).expect("checks");
    Box::leak(Box::new(Loaded {
        front: Box::leak(Box::new(front)),
        texts: named.into_iter().collect(),
    }))
}

/// The emitter as production builds a working copy of it: `PLY_C_EMITTER=ply:<dir>`'s recipe.
fn emitter_sources() -> Sources {
    Sources::Directory(repo().join("crates/ply-compiler/ply"))
}

fn emitter() -> Result<PlyProducer, String> {
    producer::build(&emitter_sources())
}

fn emitter_identity() -> String {
    producer::identity_of(&emitter_sources())
}

const PROGRAM: &str = r#"
fn double(x: Int) -> Int = x * 2
fn add(a: Int, b: Int) -> Int = a + b
fn clamp(a: Int, lo: Int, hi: Int) -> Int = if a < lo { lo } else if a > hi { hi } else { a }
fn nested(a: Int, b: Int) -> Int = (a + b) * (a - b)
fn sum_to(n: Int) -> Int = fold(range(0, n), 0, |acc: Int, i: Int| acc + i)
"#;

/// The producer's mode is a process-wide flag, so the tests that set it take turns.
static MODE: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn built_and_checked() {
    let _turn = MODE.lock().unwrap_or_else(|e| e.into_inner());
    let _held = producer::hand_over(emitter().expect("the emitter builds"), emitter_identity());
    let loaded = load(&[("m", PROGRAM)]);
    let source: &'static Source = Box::leak(Box::new(
        Source::from_front(loaded.front, HashMap::new()).with_texts(loaded.texts.clone()),
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

    let cases: Vec<(&str, Vec<Value>, Value)> = vec![
        ("m.double", vec![Value::Int(21)], Value::Int(42)),
        ("m.add", vec![Value::Int(40), Value::Int(2)], Value::Int(42)),
        (
            "m.clamp",
            vec![Value::Int(-5), Value::Int(0), Value::Int(9)],
            Value::Int(0),
        ),
        (
            "m.clamp",
            vec![Value::Int(50), Value::Int(0), Value::Int(9)],
            Value::Int(9),
        ),
        (
            "m.nested",
            vec![Value::Int(7), Value::Int(3)],
            Value::Int(40),
        ),
        ("m.sum_to", vec![Value::Int(100)], Value::Int(4950)),
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
        assert_eq!(got, want, "`{name}{args:?}`");
    }
}

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
    let _held = producer::hand_over(emitter().expect("the emitter builds"), emitter_identity());
    let loaded = load(&[("m", EFFECTS)]);
    let source: &'static Source = Box::leak(Box::new(
        Source::from_front(loaded.front, HashMap::new()).with_texts(loaded.texts.clone()),
    ));
    let names: Vec<String> = source.functions();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let (native, refused) = ply_codegen::c::build(source, &refs).expect("the program builds");
    assert!(refused.is_empty(), "{refused:?}");
    let cases: Vec<(&str, Vec<Value>, Value)> = vec![
        ("m.counted", vec![Value::Int(3)], Value::Int(68)),
        ("m.nested", vec![Value::Int(5)], Value::Int(111)),
        ("m.guarded", vec![Value::Int(4)], Value::Int(1008)),
        ("m.guarded", vec![Value::Int(40)], Value::Int(-40)),
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
        assert_eq!(got, want, "`{name}{args:?}`");
    }
}

#[test]
fn the_chain_entered_whole_answers_what_the_machine_answers() {
    built_and_checked();
}

const UNANSWERED: &str = r#"
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
    counter.bump(n) -> n,
  }

fn lonely(n: Int) -> Int / {orphan.write} = orphan.poke(n)
"#;

/// A `perform` nothing in the program answers compiles and reaches the host binding from the
/// runtime, so the fixpoint drops neither the performer nor its handler.
#[test]
fn a_perform_no_handler_in_the_program_answers_still_compiles() {
    let _turn = MODE.lock().unwrap_or_else(|e| e.into_inner());
    let _held = producer::hand_over(emitter().expect("the emitter builds"), emitter_identity());
    let loaded = load(&[("m", UNANSWERED)]);
    let source: &'static Source = Box::leak(Box::new(
        Source::from_front(loaded.front, HashMap::new()).with_texts(loaded.texts.clone()),
    ));
    let names: Vec<String> = source.functions();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let (native, refused) = ply_codegen::c::build(source, &refs).expect("the program builds");
    assert!(refused.is_empty(), "{refused:?}");
    for taken in [
        "m.performer",
        "m.lonely",
        "m.hosted",
        "m.hosting",
        "m.handler",
    ] {
        assert!(
            native.entry(taken).is_some(),
            "`{taken}` is not in the unit"
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

#[test]
fn the_chain_entered_whole_reaches_the_host_as_the_machine_does() {
    let _turn = MODE.lock().unwrap_or_else(|e| e.into_inner());
    let _held = producer::hand_over(emitter().expect("the emitter builds"), emitter_identity());
    let loaded = load(&[("m", HOSTED)]);
    let source: &'static Source = Box::leak(Box::new(
        Source::from_front(loaded.front, HashMap::new()).with_texts(loaded.texts.clone()),
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
    let bound = std::sync::Arc::new(
        registry
            .bind(&loaded.front.check)
            .expect("the registry binds"),
    );
    let hermetic = std::sync::Arc::new(ply_eval::HostBinding::hermetic());
    // A user effect performed by a compiled root resolves to no host row, so every case refuses at the host boundary.
    let cases: Vec<HostCase> = vec![
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

/// `Float` and `Decimal` do not cross the seam, so each case answers through a conversion that does.
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
    let _held = producer::hand_over(emitter().expect("the emitter builds"), emitter_identity());
    let loaded = load(&[("m", NUMERIC)]);
    let source: &'static Source = Box::leak(Box::new(
        Source::from_front(loaded.front, HashMap::new()).with_texts(loaded.texts.clone()),
    ));
    let names: Vec<String> = source.functions();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let (native, refused) = ply_codegen::c::build(source, &refs).expect("the program builds");
    assert!(refused.is_empty(), "{refused:?}");
    let bits = |f: f64| Value::Int(f.to_bits() as i64);
    let cases: Vec<(&str, Vec<Value>, Value)> = vec![
        ("m.bigger", vec![bits(2.0)], Value::Bool(true)),
        ("m.bigger", vec![bits(1.0)], Value::Bool(false)),
        ("m.half", vec![bits(3.0)], bits(1.5)),
        ("m.tenth", vec![Value::Int(7)], Value::str("0.70")),
        ("m.same", vec![bits(2.5)], Value::Bool(true)),
        ("m.negated", vec![bits(2.0)], bits(-2.0)),
        (
            "m.product",
            vec![Value::Int(6), Value::Int(7)],
            Value::str("42"),
        ),
        (
            "m.ordered",
            vec![Value::Int(6), Value::Int(7)],
            Value::Bool(true),
        ),
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
        assert_eq!(got, want, "`{name}{args:?}`");
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

#[test]
fn the_chain_entered_whole_schedules_as_the_machine_does() {
    let _turn = MODE.lock().unwrap_or_else(|e| e.into_inner());
    let _held = producer::hand_over(emitter().expect("the emitter builds"), emitter_identity());
    let loaded = load(&[("m", SIMULATED)]);
    let source: &'static Source = Box::leak(Box::new(
        Source::from_front(loaded.front, HashMap::new()).with_texts(loaded.texts.clone()),
    ));
    let names: Vec<String> = source.functions();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let (native, refused) = ply_codegen::c::build(source, &refs).expect("the program builds");
    assert!(refused.is_empty(), "{refused:?}");
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
            r#"["TaskId(0) of [TaskId(0)] chose 0 touching StepFootprint({Atom(EffectAtom { effect: \"random\", resource: Singleton, mode: Write, op: None })})", "TaskId(0) of [TaskId(0)] chose 0 touching StepFootprint({Atom(EffectAtom { effect: \"random\", resource: Singleton, mode: Write, op: None })})", "TaskId(0) of [TaskId(0)] chose 0 touching StepFootprint({})"]"#,
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

#[test]
fn the_chain_entered_whole_resumes_off_the_tail_as_the_machine_does() {
    let _turn = MODE.lock().unwrap_or_else(|e| e.into_inner());
    let _held = producer::hand_over(emitter().expect("the emitter builds"), emitter_identity());
    let loaded = load(&[("m", RESUMED)]);
    let source: &'static Source = Box::leak(Box::new(
        Source::from_front(loaded.front, HashMap::new()).with_texts(loaded.texts.clone()),
    ));
    let names: Vec<String> = source.functions();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let (native, refused) = ply_codegen::c::build(source, &refs).expect("the program builds");
    assert!(refused.is_empty(), "{refused:?}");
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

#[test]
fn the_chain_entered_whole_resumes_more_than_once_as_the_machine_does() {
    let _turn = MODE.lock().unwrap_or_else(|e| e.into_inner());
    let _held = producer::hand_over(emitter().expect("the emitter builds"), emitter_identity());
    let loaded = load(&[("m", MULTISHOT)]);
    let source: &'static Source = Box::leak(Box::new(
        Source::from_front(loaded.front, HashMap::new()).with_texts(loaded.texts.clone()),
    ));
    let names: Vec<String> = source.functions();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let (native, refused) = ply_codegen::c::build(source, &refs).expect("the program builds");
    assert!(refused.is_empty(), "{refused:?}");
    // `across` captures under a task whose region has ended by the second resumption, so the tier refuses it.
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

#[test]
fn the_chain_entered_whole_opens_a_production_region_as_the_machine_does() {
    let _turn = MODE.lock().unwrap_or_else(|e| e.into_inner());
    let _held = producer::hand_over(emitter().expect("the emitter builds"), emitter_identity());
    let loaded = load(&[("m", PRODUCTION)]);
    let source: &'static Source = Box::leak(Box::new(
        Source::from_front(loaded.front, HashMap::new()).with_texts(loaded.texts.clone()),
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
    // Neither engine calls the handler: a region answers the scheduler's operations.
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
    let bound = std::sync::Arc::new(
        registry
            .bind(&loaded.front.check)
            .expect("the registry binds"),
    );
    let hermetic = std::sync::Arc::new(ply_eval::HostBinding::hermetic());
    // `parked`'s `slow.fetch` resolves to no host row and a hermetic binding refuses the region: both refuse at the host boundary.
    let cases: Vec<HostCase> = vec![
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

/// A law's guard and body and a definition's clauses are roots, entered with the bound names' values.
#[test]
#[allow(clippy::arc_with_non_send_sync)]
fn the_ply_emitter_answers_a_programs_propositions_as_roots() {
    let _turn = MODE.lock().unwrap_or_else(|e| e.into_inner());
    let _held = producer::hand_over(emitter().expect("the emitter builds"), emitter_identity());
    let loaded = load(&[("m", PROPOSITIONS)]);
    let source: &'static Source = Box::leak(Box::new(
        Source::from_front(loaded.front, HashMap::new()).with_texts(loaded.texts.clone()),
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

/// The standard library as a program of its own, and the definitions nothing in it reaches. The
/// emitter's program now carries only the shipped modules the compiler imports, so this is where
/// the rest is checked, and by the compiler these sources build rather than the bundle's.
fn standard_library() -> (&'static Source, Vec<String>) {
    let modules: Vec<(String, String)> = ply_std::sources()
        .map(|(name, text)| (name.to_string(), text.to_string()))
        .collect();
    let ids: Vec<SourceId> = (0..modules.len()).map(|i| SourceId(i as u32)).collect();
    let front = producer::checked_front(&modules, &ids).expect("the standard library checks");
    let unused: Vec<String> = front
        .diagnostics
        .iter()
        .filter(|d| d.code == ply_span::codes::UNUSED_DEFINITION)
        .map(|d| d.message.clone())
        .collect();
    let front: &'static ply_ty::Front = Box::leak(Box::new(front));
    let source: &'static Source = Box::leak(Box::new(
        Source::from_front(front, ply_codegen::emit_keys(front))
            .with_texts(modules.into_iter().collect()),
    ));
    (source, unused)
}

#[test]
fn the_standard_library_carries_no_definition_nothing_reaches() {
    let _turn = MODE.lock().unwrap_or_else(|e| e.into_inner());
    let _held = producer::hand_over(emitter().expect("the emitter builds"), emitter_identity());
    let (_, unused) = standard_library();
    assert!(
        unused.is_empty(),
        "the standard library carries definitions nothing reaches; delete them:\n  {}",
        unused.join("\n  ")
    );
}

#[test]
fn the_emitter_refuses_no_body_or_test_of_the_standard_library() {
    let _turn = MODE.lock().unwrap_or_else(|e| e.into_inner());
    let _held = producer::hand_over(emitter().expect("the emitter builds"), emitter_identity());
    let (source, _) = standard_library();
    let names: Vec<String> = source.functions();
    assert!(!names.is_empty(), "the standard library offered no root");
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let produced = ply_codegen::c::produce(source, &refs).expect("the standard library emits");
    assert!(
        produced.refused.is_empty(),
        "the emitter refuses part of the standard library: {:?}",
        produced.refused
    );
}

/// The emitter's program is closed by reading import lines in Rust, because its identity has to be
/// known before any compiler runs: `build` needs it to choose between the committed bundle, a
/// stage and emitting one. The front end reads the same imports when it pulls a user program's
/// shelf, and that is the definition; this holds the Rust reading to it.
#[test]
fn the_emitters_program_is_the_one_the_front_end_pulls() {
    let _turn = MODE.lock().unwrap_or_else(|e| e.into_inner());
    let _held = producer::hand_over(emitter().expect("the emitter builds"), emitter_identity());
    let own: Vec<(String, String)> = ply_compiler::sources()
        .map(|(name, text)| (name.to_string(), text.to_string()))
        .collect();
    let shelf: Vec<(String, String)> = ply_std::sources()
        .map(|(name, text)| (name.to_string(), text.to_string()))
        .collect();
    let pulled = producer::front_pulling_std(&own, &shelf).expect("the front end pulls the shelf");
    let program = producer::modules_of(&Sources::Embedded);
    let mut ours: Vec<&str> = program
        .iter()
        .map(|(name, _)| name.as_str())
        .filter(|name| ply_std::is_reserved(name))
        .collect();
    ours.sort_unstable();
    let mut theirs: Vec<&str> = pulled.modules.iter().map(String::as_str).collect();
    theirs.sort_unstable();
    assert_eq!(
        ours, theirs,
        "the shipped modules the emitter's program carries are not the ones the front end pulls \
         for it"
    );
    assert_eq!(
        program.len(),
        own.len() + theirs.len(),
        "the emitter's program is its own modules and the ones they import, and nothing else"
    );
}

/// `std.hash` is the one shipped module the compiler imports, so it is the one the bundle's
/// identity and every cache key still cover; the reference implementation says whether it is
/// BLAKE3, over the published vectors and either side of a block, a chunk and a two-chunk tree.
#[test]
fn the_shipped_blake3_is_blake3() {
    let _turn = MODE.lock().unwrap_or_else(|e| e.into_inner());
    let _held = producer::hand_over(emitter().expect("the emitter builds"), emitter_identity());
    let loaded = load(&[("std.bytes", ply_std::BYTES), ("std.hash", ply_std::HASH)]);
    let source: &'static Source = Box::leak(Box::new(
        Source::from_front(loaded.front, HashMap::new()).with_texts(loaded.texts.clone()),
    ));
    let names: Vec<String> = source.functions();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let (native, refused) = ply_codegen::c::build(source, &refs).expect("the module builds");
    assert!(refused.is_empty(), "{refused:?}");
    let entry = native
        .entry("std.hash.blake3")
        .expect("`std.hash.blake3` was not compiled");
    // `b""` and `b"\x00"` are the first two published vectors; the rest bracket 64 and 1024.
    for length in [0usize, 1, 2, 63, 64, 65, 127, 1023, 1024, 1025, 2048, 2049] {
        let input: Vec<u8> = (0..length).map(|i| (i % 251) as u8).collect();
        let mut ctx = native.context();
        ctx.begin(i64::MAX / 2);
        let layouts: *const ply_codegen::heap::Layouts = &native.tables().layouts;
        let words: [i64; 1] = [ctx
            .heap
            .to_word(unsafe { &*layouts }, &Value::bytes(&input))];
        let answer = unsafe { entry(&mut ctx, words.as_ptr()) };
        assert_eq!(
            ctx.failed, 0,
            "`std.hash.blake3` raised over {length} bytes"
        );
        let got = ply_codegen::heap::Heap::to_value(unsafe { &*layouts }, answer);
        ctx.end();
        assert_eq!(
            got,
            Value::bytes(blake3::hash(&input).as_bytes()),
            "`std.hash.blake3` is not BLAKE3 over {length} bytes"
        );
    }
}

/// What the front end published for each `fn`, by program-wide name.
fn footprints(dump: &str, count: usize) -> std::collections::BTreeMap<String, String> {
    let ids: Vec<SourceId> = (0..count).map(|i| SourceId(i as u32)).collect();
    let front = ply_ty::read_front(dump, &ids).unwrap_or_else(|e| panic!("{e}"));
    front
        .check
        .defs
        .iter()
        .map(|(name, d)| (name.to_string(), ply_ty::print_footprint(&d.footprint)))
        .collect()
}

/// The recompute unit is the definition, not the module: handing the front end a row for every
/// definition and then editing one body must leave every definition whose hash did not move
/// published from its row, including the ones beside the edit and the ones importing them.
#[test]
fn an_edit_walks_the_definitions_that_depend_on_it_and_no_others() {
    let _turn = MODE.lock().unwrap_or_else(|e| e.into_inner());
    let _held = producer::hand_over(emitter().expect("the emitter builds"), emitter_identity());
    // A row nothing performs, so a definition published from its row is told from a walked one.
    const SENTINEL: &str = "base.probe.read";
    let base = |body: &str| {
        format!(
            "effect probe {{ read peek() -> Int }}\n\
             pub fn poke() -> Int / {{probe.read}} = probe.peek()\n\
             pub fn one() -> Int = {body}\n\
             pub fn two() -> Int = 2\n\
             pub fn three() -> Int = one() + 1\n"
        )
    };
    const APP: &str = "import base\n\
                       pub fn four() -> Int = base::one() + 3\n\
                       pub fn five() -> Int = base::two() + 4\n";
    let program = |body: &str| {
        vec![
            ("base".to_string(), base(body)),
            ("app".to_string(), APP.to_string()),
        ]
    };

    let before = producer::front_pulling_std(&program("1"), &[]).expect("the program checks");
    let ids: Vec<SourceId> = (0..2).map(|i| SourceId(i as u32)).collect();
    let checked = ply_ty::read_front(&before.dump, &ids).unwrap_or_else(|e| panic!("{e}"));
    let known: Vec<producer::KnownDef> = checked
        .hashes
        .defs
        .iter()
        .map(|(name, hash)| producer::KnownDef {
            name: name.to_string(),
            hash: *hash,
            witness: vec![(
                "base.probe".to_string(),
                checked.hashes.decls[&ply_span::Symbol::new("base.probe")],
            )],
            footprint: SENTINEL.to_string(),
            performed: SENTINEL.to_string(),
        })
        .collect();

    let kept = producer::front_pulling_std_with(
        &program("1"),
        &[],
        &known,
        &[],
        &producer::Packages::anonymous(String::new()),
    )
    .expect("the program checks");
    for (name, footprint) in footprints(&kept.dump, 2) {
        assert_eq!(footprint, SENTINEL, "`{name}` was walked, not taken");
    }

    // One body edited: `one` moves, and with it everything that reaches it, and nothing else.
    let after = producer::front_pulling_std_with(
        &program("11"),
        &[],
        &known,
        &[],
        &producer::Packages::anonymous(String::new()),
    )
    .expect("the program checks");
    let walked: Vec<String> = footprints(&after.dump, 2)
        .into_iter()
        .filter(|(_, footprint)| footprint != SENTINEL)
        .map(|(name, _)| name)
        .collect();
    assert_eq!(
        walked,
        vec![
            "app.four".to_string(),
            "base.one".to_string(),
            "base.three".to_string()
        ],
        "the recompute unit is not the definition"
    );
}
