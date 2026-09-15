//! The lowered form, dumped: the oracle a code generator written in Ply is checked against.
//!
//! Every stage of the front end has one of these, and this is the first for a stage *after* it.
//! `crates/ply-codegen`'s emitter reads `ply_eval::code::Code` -- windows numbered by `slots.rs`,
//! ownership marked by `rc.rs` -- so a code generator in Ply has to produce that form before it
//! can produce anything, and this is what says whether it has.
//!
//! Nothing here compares two implementations yet, because the second one is not written. What it
//! does is establish that the oracle is *usable*: total over the corpus, stable run to run, and
//! sensitive to the things a port would get wrong.

use ply_compiler_diff::reference_lower_dump;

fn corpus() -> Vec<(String, String)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("this crate sits at <root>/crates/ply-compiler-diff");
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
            // Named as the corpus names them, because `examples/` imports `std.net` and a module
            // called `net` resolves to nothing.
            let name = if dir.ends_with("ply-std/ply") {
                format!("std.{stem}")
            } else {
                stem.to_string()
            };
            let text = std::fs::read_to_string(&p).expect("readable");
            out.push((name, text));
        }
    }
    out
}

/// Every module in the shipped corpus lowers, and says something about itself when it does.
#[test]
fn the_oracle_is_total_over_the_shipped_corpus() {
    let inputs = corpus();
    assert!(inputs.len() > 15, "the corpus shrank: {}", inputs.len());
    // One program, not one module at a time: `examples/` imports the standard library, and a
    // module resolved alone resolves to nothing.
    let dump = reference_lower_dump(&inputs);
    assert!(
        dump.starts_with(&format!("L;{};", inputs.len())),
        "the corpus did not lower: {}",
        &dump[..dump.len().min(200)]
    );
    let functions = dump.matches(";f:").count()
        + usize::from(dump.contains("L;") && dump[..dump.len().min(40)].contains("f:"));
    // The corpus is thousands of definitions; a dump that walked none of them would still be
    // "total" by the assertions above.
    assert!(
        functions > 500,
        "only {functions} function bodies were lowered over {} modules",
        inputs.len()
    );
}

/// The same input twice is the same string. A dump that is not stable is not an oracle.
#[test]
fn the_oracle_is_stable() {
    let inputs = corpus();
    let once = reference_lower_dump(&inputs);
    let twice = reference_lower_dump(&inputs);
    assert_eq!(once, twice, "two dumps of one corpus differ");
    assert!(once.len() > 100_000, "the dump is suspiciously small");
}

/// It is sensitive to the two things a port is most likely to get wrong: which slot a name reads,
/// and whether a read is the last one.
///
/// Not a test of the lowering -- that is `ply-eval`'s own business -- but of whether *this dump*
/// would notice. A canonical form that erases the ownership marks would compare two ports equal
/// while one of them leaked and the other freed too early, which is exactly the class of defect
/// three separate C-tier bugs have been.
#[test]
fn the_oracle_notices_a_slot_and_an_ownership_mark() {
    let one = vec![(
        "m".to_string(),
        "fn f(a: Int, b: Int) -> Int = a + b\n".to_string(),
    )];
    let swapped = vec![(
        "m".to_string(),
        "fn f(a: Int, b: Int) -> Int = b + a\n".to_string(),
    )];
    assert_ne!(
        reference_lower_dump(&one),
        reference_lower_dump(&swapped),
        "swapping which slot is read first did not move the dump"
    );

    // `x` read once is its own last use; read twice, the first read is not.
    let once = vec![(
        "m".to_string(),
        "fn f(x: Bytes) -> Int = bytes_len(x)\n".to_string(),
    )];
    let twice = vec![(
        "m".to_string(),
        "fn f(x: Bytes) -> Int = bytes_len(x) + bytes_len(x)\n".to_string(),
    )];
    let a = reference_lower_dump(&once);
    let b = reference_lower_dump(&twice);
    assert!(
        a.contains("ovar(x"),
        "a single read of `x` is not marked as owning it: {a}"
    );
    assert!(
        b.contains("bvar(x"),
        "the first of two reads of `x` is not marked as borrowing it: {b}"
    );
}
