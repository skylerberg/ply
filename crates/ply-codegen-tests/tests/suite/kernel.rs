use ply_codegen::Unit;
use ply_eval::{Provider, Symbol, Value};

/// As `ply test benches/kernel` loads it: the project's own `.ply` files, and no standard library.
fn kernel() -> (&'static ply_eval::Analysis, &'static Unit) {
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
    let files: Vec<(String, String)> = files
        .iter()
        .map(|path| {
            let name = path.file_name().and_then(|s| s.to_str()).expect("a name");
            let text = std::fs::read_to_string(path).expect("the kernel is readable");
            (name.to_string(), text)
        })
        .collect();
    let answer = crate::fixture::made(&files);
    let front: &'static ply_eval::Analysis = Box::leak(Box::new(answer.front.answer));
    assert!(
        !front.has_error(),
        "the kernel checks: {:?}",
        front.diagnostics
    );
    let unit = Unit::handed(front, answer.unit).expect("this host has a C compiler");
    (front, unit)
}

#[test]
fn the_whole_kernel_is_inside_the_fragment() {
    let (_, unit) = kernel();
    assert!(
        unit.refusals().is_empty(),
        "the fragment refused part of the kernel: {:?}",
        unit.refusals()
    );
    // Forty-nine definitions and the kernel's eight tests, each a root.
    assert_eq!(
        unit.compiled().len(),
        57,
        "the kernel changed size; update this number deliberately rather than loosening it"
    );
    // Every compiled definition is registered; the seam admits each call by its carried types.
    assert_eq!(unit.len(), 57, "enterable definitions");
}

#[test]
fn the_search_answers_through_compiled_code() {
    let (front, unit) = kernel();
    let backend = unit.attach();
    assert!(backend.describes(front.hashes_digest));
    let answer = backend.enter(&Symbol::new("mcts.plan_753"), &[Value::Int(200)], 10_000);
    assert!(
        matches!(answer, Some(Value::Int(_))),
        "the compiled search declined or answered something the seam cannot carry: {answer:?}"
    );
    // An answer independent of the search's internals, so a wrong `Int` above is not read as a pass.
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

/// The packing `mcts.pack` performs, spelled out so the assertion checks the kernel's arithmetic.
fn mcts_state(a: i64, b: i64, c: i64) -> i64 {
    a + b * 16 + c * 256
}
