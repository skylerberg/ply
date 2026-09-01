//! Whether an accumulator is linear or quadratic, measured in bytes — the bounded worst case,
//! ADR 0034 Decision 2.
//!
//! `stdlib_accumulator_cost` asks how many appends copied, and `rc::Stats::elements_copied` how
//! much. Both survive the list representation because the list is the tree's own: a push knows
//! whether every array on its path was uniquely held, and how many slots it copied when one was
//! not. What this file measures is the quantity neither counter states — what the allocator saw,
//! in **bytes** — because that is where the *shape* of an accumulator shows. A whole-list copy is
//! one allocation, so a quadratic accumulator once made the same O(n) allocations a linear one
//! did while moving O(n²) bytes; a bounded copy moves O(n) bytes too, and the two shapes are told
//! apart by the ratio a doubling of the work produces.

use crate::counting::charge;
use ply_eval::Machine;
use ply_span::{SourceId, SourceMap};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::parse_program;
use ply_syntax::resolve::{Resolved, resolve};

fn load(src: &str) -> (Program, Resolved) {
    let mut map = SourceMap::new();
    let id: SourceId = map.add("shape.ply", src.to_string());
    let mut program = match parse_program([(id, ModuleName::from_dotted("shape"), src)]) {
        Ok(p) => p,
        Err(ds) => panic!("the probe must parse: {ds:#?}\n{src}"),
    };
    let resolved = resolve(&mut program).expect("the probe must resolve");
    (program, resolved)
}

/// Bytes charged to running every test in `src`.
///
/// The machine is built *outside* the measured region: lowering allocates, and a fixed cost on both
/// sides of a ratio does not cancel, it flattens the ratio toward one.
fn bytes(src: &str) -> usize {
    let (program, resolved) = load(src);
    let mut machine = Machine::for_program(&program, &resolved);
    let count = machine.test_count();
    assert!(count > 0, "the probe declares no test");
    let (out, _, bytes) = charge(|| {
        let mut ok = true;
        for i in 0..count {
            ok &= machine.eval_test(i).is_ok();
        }
        ok
    });
    assert!(out, "the probe must run, or its bytes measure nothing");
    bytes
}

fn linear(n: usize) -> String {
    format!(
        "fn grow(n: Int, xs: List<Int>) -> List<Int> =\n\
         \x20 if n == 0 {{ xs }} else {{ grow(n - 1, push(xs, n)) }}\n\
         test \"linear\" {{ assert_eq(len(grow({n}, [])), {n}) }}\n"
    )
}

fn quadratic(n: usize) -> String {
    // The binding is read again after the append, so every append copies. On the
    // chain machine the pessimal *argument position* produced this shape; under ADR 0034's slot
    // frames position decides nothing, and a genuine second owner is what quadratic costs.
    format!(
        "fn grow(xs: List<Int>, n: Int) -> List<Int> = {{\n\
         \x20 let ys = push(xs, n);\n\
         \x20 if len(xs) < 0 {{ xs }} else if n == 1 {{ ys }} else {{ grow(ys, n - 1) }}\n\
         }}\n\
         test \"quadratic\" {{ assert_eq(len(grow([], {n})), {n}) }}\n"
    )
}

/// Doubling `n` roughly doubles both accumulators now: a genuine second owner still costs a copy
/// on every append, but the copy is one leaf and the path above it rather than the whole list,
/// so the accumulator that used to quadruple grows linearly. The shape is what a reader should
/// expect of a language whose core operations have no cost the source does not show; if the
/// second ratio climbs back toward four, the representation is what to look at.
///
/// Bounds are loose because the measured region carries a fixed cost — lowering, the machine,
/// the test harness — that does not scale with `n`; what they have to separate is 2× from 4×.
#[test]
fn a_shared_accumulator_grows_like_a_linear_one_because_a_copy_is_bounded() {
    let (lin_1k, lin_2k) = (bytes(&linear(1000)), bytes(&linear(2000)));
    let (shared_1k, shared_2k) = (bytes(&quadratic(1000)), bytes(&quadratic(2000)));

    let lin_ratio = lin_2k as f64 / lin_1k as f64;
    let shared_ratio = shared_2k as f64 / shared_1k as f64;
    println!(
        "\n  linear {lin_1k:>9} -> {lin_2k:>9}  {lin_ratio:.2}x\n  \
         shared {shared_1k:>9} -> {shared_2k:>9}  {shared_ratio:.2}x"
    );

    assert!(
        lin_ratio < 3.0,
        "the linear accumulator grew {lin_ratio:.2}x for twice the work, which is not linear"
    );
    assert!(
        shared_ratio < 3.0,
        "the shared accumulator grew {shared_ratio:.2}x for twice the work: a copying append is \
         moving the whole list again, and the bound Decision 2 landed is gone"
    );
    assert!(
        shared_2k > lin_2k,
        "at n = 2000 the shared accumulator moved {shared_2k} bytes against the linear one's \
         {lin_2k}: the second owner is not being charged for its copies at all"
    );
}

// The stdlib accumulators are deliberately not measured here, and the bounded worst case records why: a probe
// that imports `std.json` charges roughly 11 MB of module-level and memoised work that does not
// scale with the probe, and two runs of it are not independent — the second is measured against a
// warm memo and a warm interner. Doubling the subject read 0.98x, and *fell* between k = 500 and
// k = 1000, which is not a quantity a ratio can be taken of. The synthetic pair above works because
// both sides pay the same near-zero fixed cost; the stdlib probes need their fixed part subtracted
// or their accumulator driven directly, and neither is written.
