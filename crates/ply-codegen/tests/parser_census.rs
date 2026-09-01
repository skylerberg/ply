//! What the code generator refuses over the bootstrap target.
//!
//! `fragment.rs`'s census runs over the standard library plus one arithmetic
//! module and asks whether the fragment is neither empty nor everything. This
//! one runs over `spikes/ply-parser` — the front end Ply is being bootstrapped
//! onto, and the workload every end-to-end number in ADRs 0030 and 0031 is
//! taken on — and asks a different question: of the definitions the *seam* now
//! admits, how many does the code generator have a body for, and what does it
//! name as the reason for the rest.
//!
//! It exists because those two sets have parted company. The seam admits 84.0%
//! of body calls on this corpus (ADR 0031 §1); this backend registers 6
//! definitions of it. A census that only ever runs over arithmetic cannot see
//! that, and the gap between the two numbers is the whole of the remaining
//! limit.

use ply_eval::Provider;
use ply_syntax::ast::{ModuleName, Program};

struct Loaded {
    program: &'static Program,
    resolved: &'static ply_syntax::resolve::Resolved,
    check: &'static ply_core::CheckOutput,
}

/// The shipped standard library plus every `.ply` module in `dir`, each named
/// by its file stem — which is how the spike imports them (`import items ..`).
fn load_dir(dir: &str) -> Loaded {
    let mut sources = ply_span::SourceMap::new();
    let mut owned: Vec<(ModuleName, &'static str)> = ply_std::sources()
        .map(|(module, text)| (ModuleName::from_dotted(module), text))
        .collect();
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{dir} is readable: {e}"))
        .map(|e| e.expect("a readable entry").path())
        .filter(|p| p.extension().is_some_and(|x| x == "ply"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "{dir} holds no .ply modules");
    for path in &files {
        let stem = path.file_stem().expect("a named file").to_string_lossy();
        let text: &'static str = Box::leak(
            std::fs::read_to_string(path)
                .expect("a readable module")
                .into_boxed_str(),
        );
        owned.push((ModuleName::from_dotted(&stem), text));
    }
    let mut inputs = Vec::new();
    for (module, text) in &owned {
        let id = sources.add(ply_std::pseudo_path(module), (*text).to_string());
        inputs.push((id, module.clone(), *text));
    }
    let mut ast = ply_syntax::parse_program(inputs).expect("the corpus parses");
    let expanded = ply_derive::expand_program(&mut ast);
    assert!(expanded.is_empty(), "{expanded:?}");
    let resolved = ply_syntax::resolve::resolve(&mut ast).expect("the corpus resolves");
    let check = ply_core::check_program(&ast, &resolved).expect("the corpus checks");
    Loaded {
        program: Box::leak(Box::new(ast)),
        resolved: Box::leak(Box::new(resolved)),
        check: Box::leak(Box::new(check)),
    }
}

#[test]
fn the_census_over_the_parser_spike() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../spikes/ply-parser");
    let loaded = load_dir(dir);
    let unit = ply_codegen::Cranelift::over(loaded.program, loaded.resolved, loaded.check)
        .expect("this host has a cranelift backend");
    let functions = ply_codegen::Source::new(loaded.program, loaded.resolved, loaded.check)
        .functions()
        .len();

    let mut by_construct: std::collections::BTreeMap<&str, usize> = Default::default();
    for (_, construct) in unit.refusals() {
        *by_construct.entry(construct.as_str()).or_default() += 1;
    }
    let mut ranked: Vec<(&str, usize)> = by_construct.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));

    println!(
        "{functions} functions offered, {} survived the fixpoint, {} enterable",
        unit.compiled().len(),
        unit.len()
    );
    println!("refused, by construct, most first:");
    for (construct, count) in &ranked {
        println!("  {count:5}  {construct}");
    }
    println!("the enterable set: {:?}", unit.compiled());

    // The floor is the measurement, not a guess. This corpus reads **22**
    // enterable today, of 489 that survive the fixpoint; W1 -- the same six
    // modules plus `probe.ply` -- reads 6 through the shipping command, and the
    // two differ because `probe.ply` changes which definitions are reachable.
    // Set below the measured 22 so benign drift does not fail the build, and
    // far enough above 0 that a registry that stops registering is caught.
    // `items.parse` is in neither number: ADR 0032 §4 is what that costs.
    assert!(
        unit.len() >= 20,
        "the enterable fragment over the parser fell to {} (22 when measured)",
        unit.len()
    );
}
