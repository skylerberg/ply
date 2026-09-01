//! Whether an accumulator is linear or quadratic, measured in allocations — the bounded worst case, S5b.
//!
//! `stdlib_accumulator_cost` asks the same question of `rc::Stats`: how many appends copied. That
//! counter is computable because `push`'s copying arm knows the length it copied, and it stops
//! being computable the moment `Value::List` becomes a chunked vector — `imbl::Vector` exposes no
//! way to ask whether it is uniquely owned, so an append can report neither whether it rewrote nor
//! how much it moved. The seven assertions reading it are therefore not made *vacuous* by that
//! swap, they are made unmeasurable.
//!
//! What survives is what the allocator saw — in **bytes**, not in allocation count. A whole-list
//! copy is one `Vec::with_capacity`, so a quadratic accumulator makes the same O(n) allocations a
//! linear one does and moves O(n²) bytes doing it. Counting allocations separates the two by 2.29x
//! against 1.46x, which is not a shape; counting bytes separates them by n against n².

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
    // The binding is read again after the append, so every append clones the whole list. On the
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

/// Doubling `n` roughly doubles a linear accumulator and roughly quadruples a quadratic one.
///
/// The two programs differ only in whether the accumulator is read again after the append — the
/// second owner the semantics genuinely require a copy for. Bounds are loose because the measured
/// region carries a fixed cost — lowering, the machine, the test harness — that does not scale
/// with `n`; what they have to separate is 2× from 4×, and they do.
#[test]
fn a_quadratic_accumulator_grows_faster_than_a_linear_one() {
    let (lin_1k, lin_2k) = (bytes(&linear(1000)), bytes(&linear(2000)));
    let (quad_1k, quad_2k) = (bytes(&quadratic(1000)), bytes(&quadratic(2000)));

    let lin_ratio = lin_2k as f64 / lin_1k as f64;
    let quad_ratio = quad_2k as f64 / quad_1k as f64;
    println!(
        "\n  linear    {lin_1k:>9} -> {lin_2k:>9}  {lin_ratio:.2}x\n  \
         quadratic {quad_1k:>9} -> {quad_2k:>9}  {quad_ratio:.2}x"
    );

    assert!(
        lin_ratio < 3.0,
        "the linear accumulator grew {lin_ratio:.2}x for twice the work, which is not linear"
    );
    assert!(
        quad_ratio > 3.0,
        "the quadratic accumulator grew {quad_ratio:.2}x for twice the work; if this stops being \
         true the shape has changed and the representation is what to look at"
    );
    assert!(
        quad_2k > lin_2k * 4,
        "at n = 2000 the quadratic accumulator moved {quad_2k} bytes against the linear one's \
         {lin_2k}, which is too close together to be measuring the difference"
    );
}

// The stdlib accumulators are deliberately not measured here, and the bounded worst case records why: a probe
// that imports `std.json` charges roughly 11 MB of module-level and memoised work that does not
// scale with the probe, and two runs of it are not independent — the second is measured against a
// warm memo and a warm interner. Doubling the subject read 0.98x, and *fell* between k = 500 and
// k = 1000, which is not a quantity a ratio can be taken of. The synthetic pair above works because
// both sides pay the same near-zero fixed cost; the stdlib probes need their fixed part subtracted
// or their accumulator driven directly, and neither is written.
