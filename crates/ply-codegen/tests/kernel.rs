//! The compute kernel ADR 0018 measured, compiled by the shipping code
//! generator.
//!
//! `benches/kernel` is a Monte Carlo tree search over three-heap Nim, written
//! the way a bitboard search is written: a position is one `Int`, a move is one
//! `Int`, and the exploration term is integer fixed point because Ply ships no
//! `sqrt`. It is the workload ADR 0018 §0.5 reported **6.199×** on, through the
//! spike's own harness, on a crate no shipping command could reach.
//!
//! This file is the shipping half of that: the same source, through
//! `ply_codegen::Cranelift`, asserting what the fragment reaches. It is the
//! counterweight to `tests/fragment.rs`'s census over the standard library,
//! and the contrast between the two is the finding rather than either number
//! alone —
//!
//! | corpus | definitions | enterable | offers entered |
//! | --- | ---: | ---: | ---: |
//! | `benches/kernel` (this file) | 44 | 25 | 2,974 of 3,097 — **96.0%** |
//! | `examples/` with the standard library | 1,067 | 27 | 696 of 62,660 — **1.1%** |
//!
//! — measured through `ply test --backend cranelift --json` on 2026-08-31. A
//! compute loop is almost entirely inside the fragment and a program built out
//! of the standard library is almost entirely outside it, which is the same
//! sentence ADR 0030 reached from the other direction.

use ply_codegen::Cranelift;
use ply_eval::{Provider, Value};
use ply_span::Symbol;
use ply_syntax::ast::{ModuleName, Program};

/// Loads `benches/kernel` the way `ply test benches/kernel` loads it: the
/// project's own `.ply` files, and no standard-library module, because the
/// kernel imports none.
///
/// That is load-bearing rather than incidental. The candidate set a unit's
/// fixpoint starts from is the functions in the **program**, and a project that
/// imports `std.http` gets the whole of `std` in its program while this one
/// gets two modules. Loading the standard library here anyway would compile a
/// different program from the one the command compiles, and this file would be
/// reporting a number no user can reach — which is the defect the whole
/// milestone is about.
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

/// **The fixpoint refuses nothing in this kernel**, which is the property that
/// makes it the right workload for a code generator and the wrong one for
/// generalising from.
///
/// Every one of its definitions is arithmetic, comparison, `if`, `let`, a
/// record field read, a list or map operation, and calls to each other. The
/// standard-library census in `tests/fragment.rs` is the opposite case and the
/// two are quoted together wherever either is.
#[test]
fn the_whole_kernel_is_inside_the_fragment() {
    let (_, unit) = kernel();
    assert!(
        unit.refusals().is_empty(),
        "the fragment refused part of the kernel: {:?}",
        unit.refusals()
    );
    assert_eq!(
        unit.compiled().len(),
        44,
        "the kernel changed size; update this number deliberately rather than loosening it"
    );
    // Enterable is the subset whose whole signature is `Int` or `Bool`: the
    // machine can never be offered a call whose answer the seam cannot carry,
    // so a definition returning `Tree` is compiled — because a native body
    // reaching it is what makes the set closed — and never registered.
    assert_eq!(unit.len(), 25, "enterable definitions");
}

/// The search itself answers natively, and the answer is the one the kernel's
/// own oracle expects.
///
/// `mcts.plan_753` is the fixed-seed, fixed-iteration plan `benches/kernel`'s
/// tests pin, so this is the same claim the corpus makes, made through compiled
/// code instead of through the machine.
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
    // And a spot check with an answer that does not depend on the search's
    // internals, so a wrong `Int` above is not read as a pass. Nim's optimal
    // strategy is a closed form and `nim_sum` is it.
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

/// Three heaps in four bits apiece, side to move in the fifth field — the
/// packing `mcts.pack` performs, spelled out here so the assertion above is
/// checking the kernel's arithmetic rather than restating it.
fn mcts_state(a: i64, b: i64, c: i64) -> i64 {
    a + b * 16 + c * 256
}
