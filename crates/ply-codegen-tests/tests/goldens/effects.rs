//! Whether the shipped corpus still reaches every effect construct a compiled tier would have to
//! carry. What this arms is the corpus, not the tier: if the last `handle` or the last `with cell`
//! left it, effect work in the tier would have nothing to be checked against.
//!
//! Counted on the port's lowering (`code.lower_dump`) rather than the surface text: `with cell`
//! reaches a `cell(` node its spelling does not contain.

use crate::harness::{fixtures, port, repo_root};
use std::collections::BTreeMap;

#[test]
fn the_corpus_still_exercises_every_effect_construct_a_tier_would_have_to_carry() {
    let mut tally: BTreeMap<&str, usize> = BTreeMap::new();
    let (mut bodies, mut with_effect) = (0usize, 0usize);
    for dir in [
        repo_root().join("crates/ply-std/ply"),
        repo_root().join("examples"),
        fixtures(),
    ] {
        let mut files: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "ply"))
            .collect();
        files.sort();
        for f in files {
            let text = std::fs::read_to_string(&f).unwrap();
            let dump = port::dump("code.lower_dump", text.as_bytes());
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
             implementation of it in the compiled tier"
        );
    }
}
