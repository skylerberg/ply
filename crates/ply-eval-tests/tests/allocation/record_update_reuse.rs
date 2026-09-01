//! What a threaded record update costs the allocator once the machine is warm — ADR 0034's
//! decision 3: the base record's cells are reused rather than rebuilt.

use crate::counting::charge;
use ply_eval::Machine;
use ply_span::{SourceId, SourceMap};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::parse_program;
use ply_syntax::resolve::{Resolved, resolve};

fn load(src: &str) -> (Program, Resolved) {
    let mut map = SourceMap::new();
    let id: SourceId = map.add("reuse.ply", src.to_string());
    let mut program = match parse_program([(id, ModuleName::from_dotted("reuse"), src)]) {
        Ok(p) => p,
        Err(ds) => panic!("the probe must parse: {ds:#?}\n{src}"),
    };
    let resolved = resolve(&mut program).expect("the probe must resolve");
    (program, resolved)
}

/// Allocations charged to running the probe's one test, after a warm-up run of the same test on
/// the same machine so lowering and the free lists are not in the count.
fn allocations(src: &str) -> usize {
    let (program, resolved) = load(src);
    let mut machine = Machine::for_program(&program, &resolved);
    machine.eval_test(0).expect("the warm-up run passes");
    let (ok, allocs, _) = charge(|| machine.eval_test(0).is_ok());
    assert!(ok, "the probe must run, or its allocations measure nothing");
    allocs
}

const ROUNDS: usize = 1_000;

/// A state record threaded through a loop, one field appended and one incremented per round: the
/// record is updated in place and the list grows in place, so a round allocates nothing beyond
/// the list's own amortized growth.
#[test]
fn a_threaded_record_update_allocates_nothing_per_round() {
    let src = format!(
        "type S = {{k: Int, out: List<Int>}}\n\
         fn go(i: Int, s: S) -> S =\n\
         \x20 if i == {ROUNDS} {{ s }} else {{ go(i + 1, {{..s, k: s.k + 1, out: push(s.out, i)}}) }}\n\
         test \"threaded\" {{ assert_eq(len(go(0, {{k: 0, out: []}}).out), {ROUNDS}) }}\n"
    );
    let allocs = allocations(&src);
    let per_round = allocs as f64 / ROUNDS as f64;
    println!("  {ROUNDS} rounds: {allocs} allocations, {per_round:.3} per round");
    // A list past its first leaf allocates a node and a fresh tail per leaf of thirty-two, a
    // tenth of an allocation per round; anything near one per round would be the record or its
    // vector being rebuilt.
    assert!(
        allocs < ROUNDS / 4,
        "{allocs} allocations over {ROUNDS} rounds: the update is rebuilding the record"
    );
}
