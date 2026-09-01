//! What the region model is worth on the repository's own programs, counted.

use ply_eval::Machine;
use ply_span::{SourceId, SourceMap};
use ply_syntax::ast::ModuleName;
use ply_syntax::parse_program;
use ply_syntax::resolve::resolve;

#[derive(Default, Debug)]
struct Census {
    tests_run: usize,
    tests_refused: usize,
    allocations: u64,
    peak_live: usize,
    closes_freed: u64,
    closes_deferred: u64,
    pins_taken: u64,
    reclaimed_late: u64,
}

#[test]
fn what_the_examples_reclaim_and_what_they_have_to_hold() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the crate sits two levels under the workspace root")
        .join("examples");
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&root)
        .expect("the repository ships examples")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "ply"))
        .collect();
    files.sort();

    let mut map = SourceMap::new();
    let mut loaded: Vec<(SourceId, ModuleName, String)> = Vec::new();
    for path in &files {
        let text = std::fs::read_to_string(path).expect("an example is readable");
        let relative = path.strip_prefix(&root).unwrap_or(path);
        let name = ModuleName::from_relative_path(relative).expect("an example names a module");
        let id = map.add(path, text.clone());
        loaded.push((id, name, text));
    }
    // Demand-driven, exactly as `ply`'s own loader is.
    let mut next = 0;
    while next < loaded.len() {
        let (id, name, text) = loaded[next].clone();
        next += 1;
        let Ok(module) = ply_syntax::parse_module(id, name, &text) else {
            continue;
        };
        for wanted in module.imports.iter().map(|i| i.module_name()) {
            if !ply_std::is_std(&wanted) || loaded.iter().any(|(_, n, _)| *n == wanted) {
                continue;
            }
            let Some(source) = ply_std::source(&wanted) else {
                continue;
            };
            let id = map.add(ply_std::pseudo_path(&wanted), source.to_string());
            loaded.push((id, wanted, source.to_string()));
        }
    }
    let inputs: Vec<_> = loaded
        .iter()
        .map(|(id, name, text)| (*id, name.clone(), text.as_str()))
        .collect();
    let mut program = parse_program(inputs).expect("the examples parse");
    assert!(
        ply_derive::expand_program(&mut program).is_empty(),
        "the examples expand"
    );
    let resolved = resolve(&mut program).expect("the examples resolve");
    let check = ply_core::check_program(&program, &resolved).expect("the examples typecheck");

    let mut census = Census::default();
    let mut machine = Machine::new(&program, &resolved, &check);
    for index in 0..machine.test_count() {
        // A test that reaches a real socket or a real database is refused hermetically, which is
        // the default this project ships; it still ran whatever it ran before the refusal, so its
        // regions count.
        match machine.eval_test(index) {
            Ok(()) => census.tests_run += 1,
            Err(_) => census.tests_refused += 1,
        }
    }
    let stats = machine.cells().stats();
    census.allocations = stats.allocations;
    census.peak_live = stats.peak_live;
    census.closes_freed = stats.closes_freed;
    census.closes_deferred = stats.closes_deferred;
    census.pins_taken = stats.pins_taken;
    census.reclaimed_late = stats.slots_reclaimed_late;

    let closes = census.closes_freed + census.closes_deferred;
    println!(
        "\n  examples/: {} tests ran, {} refused\n  \
         {} slot bumps, peak {} live\n  \
         {closes} region closes: {} freed at the close, {} deferred to a live continuation \
         ({:.1}% freed)\n  \
         {} pins taken, {} slots reclaimed late",
        census.tests_run,
        census.tests_refused,
        census.allocations,
        census.peak_live,
        census.closes_freed,
        census.closes_deferred,
        100.0 * census.closes_freed as f64 / closes.max(1) as f64,
        census.pins_taken,
        census.reclaimed_late,
    );

    assert!(closes > 0, "no region closed, so nothing here is wired");
    assert!(
        census.closes_freed * 2 > closes,
        "most closes must reclaim, or `shared` really does cost what the static split implies: \
         {census:#?}"
    );
    assert!(
        census.peak_live < census.allocations as usize,
        "a run whose peak equals its bumps reclaimed nothing: {census:#?}"
    );
    assert_eq!(
        machine.cells().live(),
        0,
        "the last entry point left nothing behind"
    );
}
