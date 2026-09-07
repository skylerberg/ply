//! The oracle for the stage the goal names: the C one body emits.
//!
//! `lower.rs` establishes the oracle for the *tree* a code generator reads. This is the one for
//! what it writes. Nothing compares two implementations yet — the second is not written — so what
//! these check is that the oracle is usable: total over the shipped corpus, stable, and sensitive
//! to a change in the program it is given.
//!
//! **One coupling is recorded here rather than left to be discovered.** `emit_body` runs
//! `opt::optimize` before it lowers, so `1 + 2` reaches the emitter as `3`. A Ply emitter reading
//! the tree `code.ply` builds — which is lowered from the *unoptimised* AST — would disagree on
//! every foldable constant, and the disagreement would be the inliner's rather than the emitter's.
//! Either the port takes `fold_literals` and `scalarize` with it, or this oracle grows a way to
//! emit without them. That is a decision for whoever writes the port, and it is written down
//! because a first attempt would otherwise spend a day rediscovering it.

use ply_parser_spike_harness::reference_emit_dump;

fn corpus() -> Vec<(String, String)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .and_then(std::path::Path::parent)
        .expect("the harness sits three levels under the repository root");
    let mut out = Vec::new();
    for dir in ["crates/ply-std/ply", "examples"] {
        let Ok(entries) = std::fs::read_dir(root.join(dir)) else {
            continue;
        };
        let mut paths: Vec<std::path::PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "ply"))
            .collect();
        paths.sort();
        for p in paths {
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("m");
            let name = if dir.ends_with("ply-std/ply") {
                format!("std.{stem}")
            } else {
                stem.to_string()
            };
            out.push((name, std::fs::read_to_string(&p).expect("readable")));
        }
    }
    out
}

/// The shipped corpus emits, and emits a program rather than a header.
#[test]
fn the_oracle_is_total_over_the_shipped_corpus() {
    let inputs = corpus();
    assert!(inputs.len() > 15, "the corpus shrank: {}", inputs.len());
    let dump = reference_emit_dump(&inputs);
    assert!(
        dump.starts_with(&format!("C;{};", inputs.len())),
        "the corpus did not emit: {}",
        &dump[..dump.len().min(200)]
    );
    let bodies = dump.matches(";f:").count();
    assert!(
        bodies > 500,
        "only {bodies} bodies emitted over {} modules",
        inputs.len()
    );
    // Emitted C, not a summary of it: every body carries the prologue the tier gives one.
    assert!(
        dump.matches("rt_no_fuel_p(ctx)").count() > 500,
        "the dump holds fewer prologues than bodies, so it is not the C"
    );
}

/// The same input twice is the same string.
#[test]
fn the_oracle_is_stable() {
    let inputs = corpus();
    assert_eq!(reference_emit_dump(&inputs), reference_emit_dump(&inputs));
}

/// It moves when the program does, including when only the *order* of two reads moves.
#[test]
fn the_oracle_notices_a_change_in_the_program() {
    let one = vec![(
        "m".to_string(),
        "fn f(a: Int, b: Int) -> Int = a - b\n".to_string(),
    )];
    let swapped = vec![(
        "m".to_string(),
        "fn f(a: Int, b: Int) -> Int = b - a\n".to_string(),
    )];
    assert_ne!(
        reference_emit_dump(&one),
        reference_emit_dump(&swapped),
        "swapping the operands emitted the same C"
    );
}
