//! What the compiled tiers would have to carry to compile effects, and what corpus there is to
//! test it against.
//!
//! Both compiled tiers refuse all four effect constructs today (`c/emit.rs`'s `describe`,
//! `jit.rs`'s arms beside it), so an effectful body runs interpreted. This is the census of what
//! that costs and of what any implementation has to be checked against.
//!
//! **The hard one is `handle`.** Ply's `resume` is multi-shot (`docs/GUIDE.md` §7.7, ADR 0034), so
//! a captured extent has to splice onto any stack at any height -- which a C function's frame
//! cannot do. Compiling it means a state-machine transform for every function with a non-empty
//! row, and that changes the tier's calling convention rather than adding a node to its emitter.
//! `perform` and `with cell` are separable from that and much cheaper: `perform` where the handler
//! is statically known and resumes in tail position is a call, and a cell is state, not control.
//!
//! What this test arms is the corpus, not the tier: if the last `handle` or the last `with cell`
//! left the shipped corpus, effect work in either tier would have nothing to be checked against
//! and nothing here would notice.
use std::collections::BTreeMap;

fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_path_buf()
}

#[test]
fn the_corpus_still_exercises_every_effect_construct_a_tier_would_have_to_carry() {
    let mut tally: BTreeMap<&str, usize> = BTreeMap::new();
    let (mut bodies, mut with_effect) = (0usize, 0usize);
    for dir in [
        repo_root().join("crates/ply-std/ply"),
        repo_root().join("examples"),
        repo_root().join("crates/ply-compiler-diff/fixtures"),
    ] {
        let mut files: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "ply"))
            .collect();
        files.sort();
        for f in files {
            let text = std::fs::read_to_string(&f).unwrap();
            let name = f.file_stem().unwrap().to_string_lossy().to_string();
            let dump = ply_compiler_diff::reference_lower_dump(&[(name, text)]);
            for body in dump.split("f:").skip(1) {
                bodies += 1;
                let mut hit = false;
                for tag in ["perform(", "handle(", "cell(", "sim("] {
                    if body.contains(tag) {
                        *tally.entry(tag).or_default() += 1;
                        hit = true;
                    }
                }
                with_effect += usize::from(hit);
            }
        }
    }
    println!("  {with_effect} of {bodies} bodies reach an effect construct");
    for (k, v) in &tally {
        println!("    {k} {v}");
    }
    for tag in ["perform(", "handle(", "cell("] {
        assert!(
            tally.get(tag).copied().unwrap_or(0) > 0,
            "no body in the shipped corpus reaches `{tag}` any more, so nothing here can check an \
             implementation of it in either compiled tier"
        );
    }
}
