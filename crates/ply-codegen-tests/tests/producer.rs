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
fn built_and_checked(whole: bool) {
    producer::install(std::sync::Arc::new(emitter));
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

#[test]
fn the_chain_entered_whole_answers_what_the_machine_answers() {
    built_and_checked(true);
}

/// `Float` and `Decimal` literals are constants the runtime holds, opaque to the emitted C:
/// every operator over one is the machine's own through the runtime. Neither crosses the seam,
/// so each case answers through a conversion that does.
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
    producer::install(std::sync::Arc::new(emitter));
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
        ("m.negated", vec![Value::Int(2.0f64.to_bits() as i64)]),
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
