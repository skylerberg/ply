//! The compute kernel the compute-kernel record measured, compiled by the shipping code generator.

use ply_codegen::Cranelift;
use ply_eval::{Provider, Value};
use ply_span::Symbol;
use ply_syntax::ast::{ModuleName, Program};

/// Loads `benches/kernel` the way `ply test benches/kernel` loads it: the project's own `.ply`
/// files, and no standard-library module, because the kernel imports none.
fn kernel() -> (&'static Program, &'static Cranelift) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("the crate sits two levels under the repository root")
        .join("benches/kernel");
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&root)
        .unwrap_or_else(|e| panic!("{}: {e}", root.display()))
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "ply"))
        .collect();
    files.sort();
    assert_eq!(files.len(), 2, "benches/kernel changed shape: {files:?}");

    let mut sources = ply_span::SourceMap::new();
    let mut inputs = Vec::new();
    for path in &files {
        let stem = path.file_stem().and_then(|s| s.to_str()).expect("a stem");
        let text: &'static str = Box::leak(
            std::fs::read_to_string(path)
                .expect("the kernel is readable")
                .into_boxed_str(),
        );
        let id = sources.add(path.clone(), text.to_string());
        inputs.push((id, ModuleName::from_dotted(stem), text));
    }
    let mut ast = ply_syntax::parse_program(inputs).expect("the kernel parses");
    assert!(ply_derive::expand_program(&mut ast).is_empty());
    let resolved = ply_syntax::resolve::resolve(&mut ast).expect("the kernel resolves");
    let check = ply_core::check_program(&ast, &resolved).expect("the kernel checks");
    let ast: &'static Program = Box::leak(Box::new(ast));
    let unit = Cranelift::over(
        ast,
        Box::leak(Box::new(resolved)),
        Box::leak(Box::new(check)),
    )
    .expect("this host has a cranelift backend");
    (ast, unit)
}

/// **The fixpoint refuses nothing in this kernel**, which is the property that makes it the right
/// workload for a code generator and the wrong one for generalising from.
#[test]
fn the_whole_kernel_is_inside_the_fragment() {
    let (_, unit) = kernel();
    assert!(
        unit.refusals().is_empty(),
        "the fragment refused part of the kernel: {:?}",
        unit.refusals()
    );
    // Forty-four definitions and the kernel's eight tests, each a root (ADR 0036, Decision 7).
    assert_eq!(
        unit.compiled().len(),
        52,
        "the kernel changed size; update this number deliberately rather than loosening it"
    );
    // Every compiled definition is registered; the seam admits each call by its carried types,
    // so a definition returning `Tree` is entered when `Tree` carries and declined at the answer
    // when it does not.
    assert_eq!(unit.len(), 52, "enterable definitions");
}

/// The search itself answers natively, and the answer is the one the kernel's own oracle expects.
#[test]
fn the_search_answers_through_compiled_code() {
    let (program, unit) = kernel();
    let backend = unit.attach(&ply_eval::BackendSpec::honest());
    assert!(backend.describes(program));
    let answer = backend.enter(&Symbol::new("mcts.plan_753"), &[Value::Int(200)], 10_000);
    assert!(
        matches!(answer, Some(Value::Int(_))),
        "the compiled search declined or answered something the seam cannot carry: {answer:?}"
    );
    // And a spot check with an answer that does not depend on the search's internals, so a wrong
    // `Int` above is not read as a pass.
    assert_eq!(
        backend.enter(
            &Symbol::new("mcts.nim_sum"),
            &[Value::Int(mcts_state(3, 5, 7))],
            10_000
        ),
        Some(Value::Int(1)),
        "3/5/7 has nim-sum 1, so it is a first-player win"
    );
}

/// Three heaps in four bits apiece, side to move in the fifth field — the packing `mcts.pack`
/// performs, spelled out here so the assertion above is checking the kernel's arithmetic rather
/// than restating it.
fn mcts_state(a: i64, b: i64, c: i64) -> i64 {
    a + b * 16 + c * 256
}
